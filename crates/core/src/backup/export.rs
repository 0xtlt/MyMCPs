//! Making a backup: a consistent copy of the database, encrypted as it is
//! handed out.

use std::io;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::container::{self, CHUNK_BYTES, Header, Sealer};
use super::{BackupDir, BackupError, KeyDerivations, Metadata, sqlite_path};
use crate::context::Core;

/// The name of the copy in its [`BackupDir`].
const SNAPSHOT_FILE: &str = "db.sqlite3";

/// A backup being handed out: the header first, then one block for each
/// chunk. Neither the copy of the database nor the encrypted file is ever
/// held whole in memory.
///
/// The copy lives in a private directory that is deleted when the last
/// block was handed out, or when this is dropped before.
pub struct Export {
    /// The metadata, then the copy of the database.
    plaintext: Box<dyn AsyncRead + Send + Unpin>,
    sealer: Sealer,
    /// Handed out first.
    header: Option<Header>,
    dir: Option<BackupDir>,
    chunk: Vec<u8>,
    content_length: u64,
    created_at: DateTime<Utc>,
}

impl std::fmt::Debug for Export {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Export")
            .field("content_length", &self.content_length)
            .field("created_at", &self.created_at)
            .finish_non_exhaustive()
    }
}

impl Export {
    /// Copy the database as it is now and derive the key of the backup.
    ///
    /// The copy is made with `VACUUM INTO`: one file, read in one
    /// transaction, while the instance goes on writing. The work runs on a
    /// task of its own, so that a caller that gives up halfway leaves no
    /// copy of the database behind.
    pub async fn start(
        core: &Arc<Core>,
        password: String,
        derivations: &KeyDerivations,
    ) -> Result<Self, BackupError> {
        let core = core.clone();
        let derivations = derivations.clone();
        tokio::spawn(async move { Self::prepare(&core, password, &derivations).await })
            .await
            .map_err(io::Error::other)?
    }

    async fn prepare(
        core: &Core,
        password: String,
        derivations: &KeyDerivations,
    ) -> Result<Self, BackupError> {
        let created_at = Utc::now();
        let dir = BackupDir::create(&core.config)?;
        let snapshot = dir.path().join(SNAPSHOT_FILE);
        sqlx::query("vacuum into ?")
            .bind(sqlite_path(&snapshot)?)
            .execute(&*core.db)
            .await?;
        let database = File::open(&snapshot).await?;
        let database_len = database.metadata().await?.len();

        let metadata = Metadata::new(created_at, &core.config.app_key).to_json();
        // What this version writes, it must be able to read back.
        Metadata::parse(metadata.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "APP_KEY is longer than the 512 characters a backup can hold",
            )
        })?;
        let prefix = container::plaintext_prefix(metadata.as_bytes())?;
        let plaintext_len = prefix.len() as u64 + database_len;

        let header = Header::generate();
        let key = derivations.derive(password, header.clone()).await?;
        Ok(Self {
            plaintext: Box::new(io::Cursor::new(prefix).chain(database)),
            sealer: Sealer::new(&key, &header),
            header: Some(header),
            dir: Some(dir),
            chunk: Vec::with_capacity(CHUNK_BYTES),
            content_length: container::sealed_len(plaintext_len),
            created_at,
        })
    }

    /// The size of the whole file, known before its first byte is written.
    pub fn content_length(&self) -> u64 {
        self.content_length
    }

    /// When the copy was made: the `createdAt` of the backup.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// The name to save the file under.
    pub fn file_name(&self) -> String {
        super::file_name(self.created_at)
    }

    /// The next bytes of the file, or `None` after the final chunk.
    pub async fn next_block(&mut self) -> io::Result<Option<Vec<u8>>> {
        if let Some(header) = self.header.take() {
            return Ok(Some(header.to_bytes().to_vec()));
        }
        if self.dir.is_none() {
            return Ok(None);
        }

        self.chunk.resize(CHUNK_BYTES, 0);
        let mut filled = 0;
        while filled < CHUNK_BYTES {
            match self.plaintext.read(&mut self.chunk[filled..]).await? {
                0 => break,
                read => filled += read,
            }
        }
        let last = filled < CHUNK_BYTES;
        let block = self.sealer.seal(&self.chunk[..filled], last)?;
        if last {
            // The copy is not needed any longer: it does not wait for the
            // client to have received the end of the file.
            self.plaintext = Box::new(tokio::io::empty());
            self.dir = None;
        }
        Ok(Some(block))
    }
}
