//! Files that agents send to upload links, kept on disk until a tool uses
//! them or an hour has passed.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::{Stream, StreamExt};
use mymcps_core::Core;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// How long an uploaded file can be used, counted from its upload.
pub const BUILTIN_UPLOAD_MINUTES: i64 = 60;
const KEPT_MS: i64 = BUILTIN_UPLOAD_MINUTES * 60_000;

/// What the files waiting for one MCP may take on the instance's disk.
const MAX_STORED_BYTES: u64 = 100_000_000;
const MAX_STORED_FILES: usize = 50;

const METADATA: &str = ".json";

/// Upload ids name files on disk, so nothing but a UUID is ever one.
pub fn is_builtin_upload_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
        })
}

/// Where to keep a file sent to an upload link, as the tool that made the
/// link described it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinUploadTarget {
    /// Names the stored file, and is how tools refer to it afterwards. A UUID.
    pub id: String,
    pub filename: String,
    /// Left out, it follows from the filename.
    pub content_type: Option<String>,
    pub max_bytes: u64,
}

/// A file an agent sent to an upload link, kept until `expires_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinUpload {
    pub id: String,
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub size: u64,
    /// In milliseconds since the epoch.
    pub expires_at: i64,
}

/// Why a file sent to a valid link was not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadRefusal {
    /// The link already took a file.
    Taken,
    /// The request had no body.
    Empty,
    /// The file is larger than the provider accepts.
    TooLarge,
    /// Too much is already waiting for this MCP.
    Full,
}

impl UploadRefusal {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Taken => "taken",
            Self::Empty => "empty",
            Self::TooLarge => "too_large",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("Upload refused: {}", .0.as_str())]
    Refused(UploadRefusal),
    #[error("Uploads belong to a saved MCP")]
    UnsavedMcp,
    #[error("Not an upload id")]
    NotAnUploadId,
    /// The body broke off before the file was whole.
    #[error("{0}")]
    Body(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn is_missing(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::NotFound
}

fn private_open_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    // Created only if nothing has the name yet, readable by the server alone.
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
}

#[derive(Debug, Default, Clone, Copy)]
struct Kept {
    files: usize,
    bytes: u64,
}

/// The uploads under way, which the files on disk do not tell the size of
/// yet, so that uploads sent at the same time for one MCP share its room.
#[derive(Debug, Default)]
struct Ledger {
    /// Taken to count what an MCP keeps and reserve a place in it, and to
    /// settle a finished upload: a count never misses a file in between.
    counting: tokio::sync::Mutex<()>,
    usage: Mutex<HashMap<i64, Usage>>,
}

#[derive(Debug, Default)]
struct Usage {
    /// The files that were whole, or stale, at the last count, and the uploads
    /// that finished since.
    kept: Kept,
    /// The bytes received so far by each upload under way, by its id.
    arriving: HashMap<String, u64>,
}

impl Usage {
    fn bytes(&self) -> u64 {
        self.kept.bytes + self.arriving.values().sum::<u64>()
    }
}

impl Ledger {
    fn usage(&self) -> MutexGuard<'_, HashMap<i64, Usage>> {
        self.usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The ids of the uploads under way for one MCP.
    fn arriving(&self, mcp_id: i64) -> Vec<String> {
        self.usage()
            .get(&mcp_id)
            .map(|usage| usage.arriving.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Whether one more upload fits in what one MCP keeps, `kept` counted
    /// from its directory, and the uploads under way.
    fn check_room(&self, mcp_id: i64, kept: Kept, id: &str) -> Result<(), UploadRefusal> {
        let usage = self.usage();
        let (files, bytes) = match usage.get(&mcp_id) {
            Some(entry) if entry.arriving.contains_key(id) => return Err(UploadRefusal::Taken),
            Some(entry) => (entry.arriving.len(), entry.arriving.values().sum::<u64>()),
            None => (0, 0),
        };
        if kept.files + files >= MAX_STORED_FILES || kept.bytes + bytes >= MAX_STORED_BYTES {
            return Err(UploadRefusal::Full);
        }
        Ok(())
    }

    /// Count an upload as under way, with nothing received yet. Called with
    /// `counting` held, so `kept` is still what the MCP keeps.
    fn begin(&self, mcp_id: i64, kept: Kept, id: &str) {
        let mut usage = self.usage();
        let entry = usage.entry(mcp_id).or_default();
        entry.kept = kept;
        entry.arriving.insert(id.to_owned(), 0);
    }

    /// Forget an upload under way; `size` is what it now keeps on disk.
    fn settle(&self, mcp_id: i64, id: &str, size: Option<u64>) {
        let mut usage = self.usage();
        let Some(entry) = usage.get_mut(&mcp_id) else {
            return;
        };
        entry.arriving.remove(id);
        if let Some(size) = size {
            entry.kept.files += 1;
            entry.kept.bytes += size;
        }
        if entry.arriving.is_empty() {
            // Counted again from the disk by the next upload.
            usage.remove(&mcp_id);
        }
    }
}

/// An upload under way: its file and its place in the ledger. Dropped
/// before [`Reservation::commit`], by a failure or a request that was
/// abandoned halfway, it deletes what it wrote and gives its place back.
struct Reservation<'a> {
    ledger: &'a Ledger,
    mcp_id: i64,
    id: String,
    path: PathBuf,
    is_committed: bool,
}

impl Reservation<'_> {
    /// Count `bytes` more for this upload. `false` when the MCP has no room left for them.
    fn grow(&self, bytes: u64) -> bool {
        let mut usage = self.ledger.usage();
        let Some(entry) = usage.get_mut(&self.mcp_id) else {
            return false;
        };
        let total = entry.bytes() + bytes;
        if total > MAX_STORED_BYTES {
            return false;
        }
        if let Some(received) = entry.arriving.get_mut(&self.id) {
            *received += bytes;
        }
        true
    }

    async fn commit(mut self, size: u64) {
        let _counting = self.ledger.counting.lock().await;
        self.ledger.settle(self.mcp_id, &self.id, Some(size));
        self.is_committed = true;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if self.is_committed {
            return;
        }
        // Nothing is kept of a failed upload, so its link can be tried again.
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(UploadStore::metadata_path(&self.path));
        self.ledger.settle(self.mcp_id, &self.id, None);
    }
}

/// The directory uploads are kept in, one sub-directory for each MCP.
/// Clones share what they know of the uploads under way: make one store for
/// a directory and clone it.
#[derive(Debug, Clone)]
pub struct UploadStore {
    root: PathBuf,
    ledger: Arc<Ledger>,
}

impl UploadStore {
    pub fn new(core: &Core) -> Self {
        Self::at(core.config.data_dir.join("builtin-uploads"))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ledger: Arc::default(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn directory(&self, mcp_id: i64) -> Result<PathBuf, UploadError> {
        if mcp_id <= 0 {
            return Err(UploadError::UnsavedMcp);
        }
        Ok(self.root.join(mcp_id.to_string()))
    }

    fn path(&self, mcp_id: i64, id: &str) -> Result<PathBuf, UploadError> {
        if !is_builtin_upload_id(id) {
            return Err(UploadError::NotAnUploadId);
        }
        Ok(self.directory(mcp_id)?.join(id))
    }

    fn metadata_path(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(METADATA);
        PathBuf::from(name)
    }

    /// What was written once the file had arrived whole. `None` until then.
    async fn read_metadata(path: &Path) -> Option<BuiltinUpload> {
        let content = fs::read(Self::metadata_path(path)).await.ok()?;
        serde_json::from_slice(&content).ok()
    }

    async fn discard(path: &Path) {
        let _ = fs::remove_file(path).await;
        let _ = fs::remove_file(Self::metadata_path(path)).await;
    }

    /// Delete the files of one MCP that have expired, and count what is
    /// left. A file without metadata is still arriving, or was left by a
    /// server that stopped halfway: it is given as long as a finished one.
    /// An upload under way is left alone, and not counted: the ledger counts it.
    async fn sweep(&self, mcp_id: i64, now: i64) -> Result<Kept, UploadError> {
        let directory = self.directory(mcp_id)?;
        let mut kept = Kept::default();
        let arriving = self.ledger.arriving(mcp_id);

        let mut entries = match fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if is_missing(&error) => return Ok(kept),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let Some(name) = name.to_str().filter(|name| is_builtin_upload_id(name)) else {
                continue;
            };
            if arriving.iter().any(|id| id == name) {
                continue;
            }
            let path = directory.join(name);
            let stat = match fs::metadata(&path).await {
                Ok(stat) => stat,
                // Removed in the meantime by a failed upload.
                Err(error) if is_missing(&error) => continue,
                Err(error) => return Err(error.into()),
            };
            let modified_ms = stat
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_millis() as i64);
            let expires_at = Self::read_metadata(&path)
                .await
                .map_or(modified_ms + KEPT_MS, |metadata| metadata.expires_at);
            if expires_at <= now {
                Self::discard(&path).await;
            } else {
                kept.files += 1;
                kept.bytes += stat.len();
            }
        }
        Ok(kept)
    }

    /// Keep the file sent to an upload link. A link takes one file: the name
    /// it is stored under is created only if nothing has it yet, which also
    /// turns away a second request sent while the first is still arriving.
    /// Fails with [`UploadError::Refused`] when the file is not kept, and
    /// leaves nothing behind.
    pub async fn save<S, E>(
        &self,
        mcp_id: i64,
        target: &BuiltinUploadTarget,
        body: S,
    ) -> Result<BuiltinUpload, UploadError>
    where
        S: Stream<Item = Result<Bytes, E>>,
        E: std::fmt::Display,
    {
        let path = self.path(mcp_id, &target.id)?;
        let directory = self.directory(mcp_id)?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(&directory).await?;

        // Counting and reserving are one step, so that uploads sent at the
        // same time cannot each take the room that is left.
        let (mut file, reservation) = {
            let _counting = self.ledger.counting.lock().await;
            let stored = self.sweep(mcp_id, now_ms()).await?;
            self.ledger
                .check_room(mcp_id, stored, &target.id)
                .map_err(UploadError::Refused)?;
            let file = match private_open_options().open(&path).await {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    return Err(UploadError::Refused(UploadRefusal::Taken));
                }
                Err(error) => return Err(error.into()),
            };
            self.ledger.begin(mcp_id, stored, &target.id);
            let reservation = Reservation {
                ledger: &self.ledger,
                mcp_id,
                id: target.id.clone(),
                path: path.clone(),
                is_committed: false,
            };
            (file, reservation)
        };

        let written = async {
            let mut size: u64 = 0;
            let mut body = std::pin::pin!(body);
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|error| UploadError::Body(error.to_string()))?;
                size += chunk.len() as u64;
                if size > target.max_bytes {
                    return Err(UploadError::Refused(UploadRefusal::TooLarge));
                }
                if !reservation.grow(chunk.len() as u64) {
                    return Err(UploadError::Refused(UploadRefusal::Full));
                }
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            if size == 0 {
                return Err(UploadError::Refused(UploadRefusal::Empty));
            }

            let upload = BuiltinUpload {
                id: target.id.clone(),
                filename: target.filename.clone(),
                content_type: target.content_type.clone(),
                size,
                expires_at: now_ms() + KEPT_MS,
            };
            let mut metadata = private_open_options()
                .open(Self::metadata_path(&path))
                .await?;
            metadata
                .write_all(
                    serde_json::to_string(&upload)
                        .map_err(std::io::Error::other)?
                        .as_bytes(),
                )
                .await?;
            metadata.flush().await?;
            Ok(upload)
        }
        .await;

        // Dropped without being committed, the reservation deletes the file.
        if let Ok(upload) = &written {
            reservation.commit(upload.size).await;
        }
        written
    }

    /// The file uploaded under `id`. `None` when none arrived whole, or it
    /// has expired.
    pub async fn find(&self, mcp_id: i64, id: &str) -> Result<Option<BuiltinUpload>, UploadError> {
        let path = self.path(mcp_id, id)?;
        let Some(upload) = Self::read_metadata(&path).await else {
            return Ok(None);
        };
        if upload.expires_at <= now_ms() {
            Self::discard(&path).await;
            return Ok(None);
        }
        Ok(Some(upload))
    }

    /// The bytes of an uploaded file. `None` when it was deleted in the meantime.
    pub async fn read(&self, mcp_id: i64, id: &str) -> Result<Option<Vec<u8>>, UploadError> {
        match fs::read(self.path(mcp_id, id)?).await {
            Ok(content) => Ok(Some(content)),
            Err(error) if is_missing(&error) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Delete every file uploaded for an MCP. Called when the MCP is deleted.
    pub async fn remove_all(&self, mcp_id: i64) -> Result<(), UploadError> {
        match fs::remove_dir_all(self.directory(mcp_id)?).await {
            Ok(()) => Ok(()),
            Err(error) if is_missing(&error) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Delete the expired files of every MCP. A file is never served past
    /// its expiry, but only this removes the ones nobody asks for again.
    pub async fn prune(&self) -> Result<(), UploadError> {
        self.prune_at(now_ms()).await
    }

    pub async fn prune_at(&self, now: i64) -> Result<(), UploadError> {
        let mut directories = match fs::read_dir(&self.root).await {
            Ok(directories) => directories,
            Err(error) if is_missing(&error) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = directories.next_entry().await? {
            let name = entry.file_name();
            let Some(mcp_id) = name
                .to_str()
                .filter(|name| {
                    !name.starts_with('0') && name.bytes().all(|byte| byte.is_ascii_digit())
                })
                .and_then(|name| name.parse::<i64>().ok())
            else {
                continue;
            };
            if self.sweep(mcp_id, now).await?.files == 0 {
                // Not empty when an upload has just started: it stays for the next pass.
                let _ = fs::remove_dir(self.root.join(mcp_id.to_string())).await;
            }
        }
        Ok(())
    }

    /// Prune now, for what a previous run left behind, then every few
    /// minutes, until the returned task is aborted.
    pub fn start_sweeper(self: &Arc<Self>, every: Duration) -> tokio::task::JoinHandle<()> {
        let store = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(every);
            loop {
                interval.tick().await;
                if let Err(error) = store.prune().await {
                    tracing::warn!(%error, "Could not delete the expired uploads of built-in MCPs");
                }
            }
        })
    }
}
