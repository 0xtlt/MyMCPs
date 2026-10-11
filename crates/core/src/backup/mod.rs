//! Backups: one encrypted file that holds everything an instance stores.
//!
//! A backup is a consistent copy of the SQLite database and the `APP_KEY`
//! its secrets were encrypted with, sealed with a password. It holds no
//! file of the data directory: MCP sandboxes, the Deno cache and uploads
//! are caches or short-lived.
//!
//! - [`container`] is the file format, shared with the Node app.
//! - [`Export`] makes the copy and hands the file out block by block.
//! - [`import`] reads a file back, checks what it holds before trusting
//!   it, and replaces the rows of the instance with those of the backup.
//!
//! Both keep their files in a [`BackupDir`], a private directory under
//! `backup-tmp` in the data directory that is deleted when they are done.
//! [`clear_temporary_files`] deletes what a crash left there.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use chrono::{DateTime, SecondsFormat, Utc};
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::config::Config;
use crate::crypto::random_hex;

pub mod container;
mod export;
mod restore;

pub use container::{Header, Key};
pub use export::Export;
pub use restore::{Imported, import};

/// Where exports and imports keep their files, in the data directory.
pub const TEMPORARY_DIRECTORY: &str = "backup-tmp";

/// The extension of a backup file.
pub const FILE_EXTENSION: &str = "mymcps";

/// Why a backup was not read, or not made. The first five are what the
/// person importing a file is told, word for word.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    /// Shorter than a header, or it does not start with the magic.
    #[error("This file is not a MyMCPs backup")]
    NotABackup,
    /// A format version, a key derivation or a migration this version does
    /// not know.
    #[error(
        "This backup was made by a newer version of MyMCPs. Update this instance, then import it again."
    )]
    NewerVersion,
    /// The first chunk does not verify.
    #[error("The password is incorrect, or the backup file is damaged")]
    WrongPassword,
    /// A later chunk does not verify, the file ends early, or what it holds
    /// is not the metadata and the database of an instance.
    #[error("The backup file is damaged or incomplete")]
    Damaged,
    #[error("This backup holds no administrator account")]
    NoAdministrator,
    /// The instance got its first user while the backup was being read:
    /// nothing was imported.
    #[error("The instance was set up while the backup was being imported")]
    AlreadySetUp,
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The database of the instance failed, not the one of the backup.
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl BackupError {
    /// What to tell the person who sent the file. `None` for a failure that
    /// is not about the file.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::NotABackup
            | Self::NewerVersion
            | Self::WrongPassword
            | Self::Damaged
            | Self::NoAdministrator => Some(self.to_string()),
            Self::AlreadySetUp | Self::Io(_) | Self::Database(_) => None,
        }
    }
}

/// A CPU-bound or blocking step, off the async threads.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, BackupError> + Send + 'static,
) -> Result<T, BackupError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(io::Error::other)?
}

/// A path as SQLite takes it. Absolute, so that a name starting with
/// `file:` is never read as a URI.
pub(crate) fn sqlite_path(path: &Path) -> io::Result<String> {
    std::path::absolute(path)?
        .into_os_string()
        .into_string()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the data directory has a name that is not UTF-8",
            )
        })
}

/// What a backup says about itself, in the JSON object its plaintext
/// starts with.
#[derive(Clone)]
pub struct Metadata {
    /// When the backup was made: ISO 8601 in UTC, with milliseconds.
    pub created_at: String,
    /// The `APP_KEY` of the instance that made it, as that instance was
    /// configured with it.
    pub app_key: String,
    /// The app that made it. Informational.
    pub app: Option<Value>,
}

impl std::fmt::Debug for Metadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Metadata")
            .field("created_at", &self.created_at)
            .field("app", &self.app)
            .finish_non_exhaustive()
    }
}

/// The metadata as a reader takes it. Members it does not know are ignored.
static METADATA_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    // Read as it was written: an empty string is not a missing one.
    vine::Vine::new().create(vine::object! {
        "createdAt" => vine::string(),
        "appKey" => vine::string().min_length(16).max_length(512),
        "app" => vine::any().optional(),
    })
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetadataFields {
    created_at: String,
    app_key: String,
    app: Option<Value>,
}

impl Metadata {
    /// The metadata of a backup made now by this version.
    pub fn new(created_at: DateTime<Utc>, app_key: &str) -> Self {
        Self {
            created_at: created_at.to_rfc3339_opts(SecondsFormat::Millis, true),
            app_key: app_key.to_string(),
            app: Some(json!({ "runtime": "rust", "version": crate::VERSION })),
        }
    }

    /// The bytes a backup holds.
    pub fn to_json(&self) -> String {
        let mut members = serde_json::Map::new();
        members.insert("createdAt".into(), Value::from(self.created_at.as_str()));
        members.insert("appKey".into(), Value::from(self.app_key.as_str()));
        if let Some(app) = &self.app {
            members.insert("app".into(), app.clone());
        }
        Value::Object(members).to_string()
    }

    /// Read the metadata of a backup. Anything else than a JSON object with
    /// its two required members is a damaged file.
    pub fn parse(bytes: &[u8]) -> Result<Self, BackupError> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| BackupError::Damaged)?;
        if !value.is_object() {
            return Err(BackupError::Damaged);
        }
        let fields: MetadataFields = METADATA_VALIDATOR
            .validate_as(&value)
            .map_err(|_| BackupError::Damaged)?;
        Ok(Self {
            created_at: fields.created_at,
            app_key: fields.app_key,
            app: fields.app,
        })
    }
}

/// The name a browser saves a backup under: `mymcps-backup-YYYYMMDD-HHMMSS.mymcps`, in UTC.
pub fn file_name(created_at: DateTime<Utc>) -> String {
    format!(
        "mymcps-backup-{}.{FILE_EXTENSION}",
        created_at.format("%Y%m%d-%H%M%S")
    )
}

/// A private directory for the files of one export or one import:
/// `<data dir>/backup-tmp/<random>`, readable by the server alone. It is
/// deleted with everything in it when this is dropped.
#[derive(Debug)]
pub struct BackupDir {
    path: PathBuf,
}

fn private_directory(recursive: bool) -> std::fs::DirBuilder {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
}

/// A new file only its owner can read: the copy of a database, with the
/// password hashes and the credentials it holds.
pub fn private_file() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn temporary_root(config: &Config) -> io::Result<PathBuf> {
    Ok(std::path::absolute(&config.data_dir)?.join(TEMPORARY_DIRECTORY))
}

impl BackupDir {
    pub fn create(config: &Config) -> io::Result<Self> {
        let root = temporary_root(config)?;
        private_directory(true).create(&root)?;
        let path = root.join(random_hex(16));
        private_directory(false).create(&path)?;
        Ok(Self { path })
    }

    /// The directory, as an absolute path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for BackupDir {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            // What is left is deleted when the server starts.
            Err(error) => tracing::warn!(
                directory = %self.path.display(),
                %error,
                "The temporary files of a backup could not be deleted"
            ),
        }
    }
}

/// Delete what exports and imports left in the data directory. For the
/// start of the server, when none can be under way: a crash, or a stop in
/// the middle of one, leaves a copy of the database there.
pub fn clear_temporary_files(config: &Config) -> io::Result<()> {
    match std::fs::remove_dir_all(temporary_root(config)?) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Derives the keys of backups, one at a time.
///
/// A derivation takes 128 MiB for a new backup, and up to 256 MiB for one
/// that is imported: the cost is what makes a password slow to guess. Run
/// at will, exports and imports would add these up. One of these belongs
/// to a server, and is shared by everything that derives a key in it.
#[derive(Debug, Clone)]
pub struct KeyDerivations {
    turn: Arc<Semaphore>,
}

impl Default for KeyDerivations {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyDerivations {
    pub fn new() -> Self {
        Self {
            turn: Arc::new(Semaphore::new(1)),
        }
    }

    /// The key of a backup, derived off the async threads once the
    /// derivation before it is done. The password is not kept.
    pub async fn derive(&self, password: String, header: Header) -> io::Result<Key> {
        let turn = self
            .turn
            .clone()
            .acquire_owned()
            .await
            .map_err(io::Error::other)?;
        // The turn goes with the work: a caller that gives up does not let
        // the next derivation start while this one still holds its memory.
        tokio::task::spawn_blocking(move || {
            let key = Key::derive(&password, &header);
            drop(turn);
            key
        })
        .await
        .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_metadata_a_reader_expects() {
        let created_at = DateTime::parse_from_rfc3339("2026-10-08T15:30:00.123456Z")
            .unwrap()
            .with_timezone(&Utc);
        let metadata = Metadata::new(created_at, "the key of this test");
        let json = metadata.to_json();
        assert_eq!(
            json,
            format!(
                "{{\"createdAt\":\"2026-10-08T15:30:00.123Z\",\"appKey\":\"the key of this test\",\"app\":{{\"runtime\":\"rust\",\"version\":\"{}\"}}}}",
                crate::VERSION
            )
        );
        assert_eq!(
            file_name(created_at),
            "mymcps-backup-20261008-153000.mymcps"
        );

        let read = Metadata::parse(json.as_bytes()).unwrap();
        assert_eq!(read.created_at, "2026-10-08T15:30:00.123Z");
        assert_eq!(read.app_key, "the key of this test");
        // The key is never printed.
        assert!(!format!("{read:?}").contains("of this test"));
    }

    #[test]
    fn reads_metadata_with_unknown_members_and_refuses_the_rest() {
        let other = Metadata::parse(
            br#"{"future":[1,2],"appKey":"sixteen chars ok","createdAt":"2026-01-02T03:04:05.000Z","app":{"runtime":"node","version":"0.4.1"}}"#,
        )
        .unwrap();
        assert_eq!(other.app_key, "sixteen chars ok");
        assert_eq!(other.app.unwrap()["runtime"], "node");
        assert!(
            Metadata::parse(br#"{"appKey":"sixteen chars ok","createdAt":"x"}"#)
                .unwrap()
                .app
                .is_none()
        );

        let long_key = format!(r#"{{"createdAt":"x","appKey":"{}"}}"#, "k".repeat(513));
        for damaged in [
            &b""[..],
            b"not json",
            b"[]",
            b"\"text\"",
            b"{}",
            br#"{"createdAt":"2026-01-02T03:04:05.000Z"}"#,
            br#"{"appKey":"sixteen chars ok"}"#,
            br#"{"createdAt":"x","appKey":"fifteen chars o"}"#,
            br#"{"createdAt":1,"appKey":"sixteen chars ok"}"#,
            br#"{"createdAt":"x","appKey":["sixteen chars ok"]}"#,
            long_key.as_bytes(),
            b"{\"createdAt\":\"x\",\"appKey\":\"sixteen chars ok\"}\xff",
        ] {
            assert!(
                matches!(Metadata::parse(damaged), Err(BackupError::Damaged)),
                "{}",
                String::from_utf8_lossy(damaged)
            );
        }
    }

    #[test]
    fn keeps_its_files_in_a_private_directory_and_deletes_them() {
        let data = tempfile::tempdir().unwrap();
        let config = Config::for_tests(data.path());
        let root = data.path().join(TEMPORARY_DIRECTORY);

        let first = BackupDir::create(&config).unwrap();
        let second = BackupDir::create(&config).unwrap();
        assert_ne!(first.path(), second.path());
        assert!(first.path().is_absolute());
        assert_eq!(
            first.path().parent().unwrap(),
            std::path::absolute(&root).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for directory in [&root, first.path()] {
                let mode = std::fs::metadata(directory).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o700, "{}", directory.display());
            }
            let file = first.path().join("db.sqlite3");
            private_file().open(&file).unwrap();
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let kept = second.path().to_path_buf();
        drop(first);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);

        // What a crash left is deleted when the server starts.
        std::mem::forget(second);
        std::fs::write(kept.join("db.sqlite3"), b"left behind").unwrap();
        clear_temporary_files(&config).unwrap();
        assert!(!root.exists());
        clear_temporary_files(&config).unwrap();
    }
}
