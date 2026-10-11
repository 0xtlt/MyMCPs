//! Paths as Node's `path` and `fs` modules treat them: by name, without
//! reading links.

use std::io;
use std::path::{Component, Path, PathBuf};

/// `path.normalize`: `.` and `..` are resolved by name, and `..` stops at the root.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let at_root = normalized.has_root() && normalized.parent().is_none();
                let climbs = normalized
                    .components()
                    .next_back()
                    .is_none_or(|last| last == Component::ParentDir);
                if at_root {
                    continue;
                }
                if climbs {
                    normalized.push("..");
                } else {
                    normalized.pop();
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// `path.resolve(path)` in a process whose working directory is `current_dir`.
pub(crate) fn resolve(current_dir: &Path, path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&current_dir.join(path))
    }
}

/// Whether `path` is `directory` or lies below it. Both are absolute and normalized.
pub(crate) fn is_within(path: &Path, directory: &Path) -> bool {
    path.starts_with(directory)
}

/// A regular file the server may execute, links followed.
pub(crate) fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok() && path.is_file()
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// `fs.rm(path, { recursive: true, force: true })`: nothing to do when the
/// path is missing, and a link is removed, not followed.
pub(crate) async fn remove_recursively(path: &Path) -> io::Result<()> {
    let removed = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() => tokio::fs::remove_dir_all(path).await,
        Ok(_) => tokio::fs::remove_file(path).await,
        Err(error) => Err(error),
    };
    match removed {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// The code and description libuv gives the errors a full disk, a read-only
/// volume or a permission problem cause.
fn system_error(error: &io::Error) -> Option<(&'static str, &'static str)> {
    use io::ErrorKind;
    // EPERM and EACCES are one kind for Rust.
    if cfg!(unix) && error.raw_os_error() == Some(1) {
        return Some(("EPERM", "operation not permitted"));
    }
    Some(match error.kind() {
        ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        ErrorKind::AlreadyExists => ("EEXIST", "file already exists"),
        ErrorKind::NotADirectory => ("ENOTDIR", "not a directory"),
        ErrorKind::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        ErrorKind::DirectoryNotEmpty => ("ENOTEMPTY", "directory not empty"),
        ErrorKind::ReadOnlyFilesystem => ("EROFS", "read-only file system"),
        ErrorKind::StorageFull => ("ENOSPC", "no space left on device"),
        ErrorKind::QuotaExceeded => ("EDQUOT", "disk quota exceeded"),
        ErrorKind::ResourceBusy => ("EBUSY", "resource busy or locked"),
        ErrorKind::InvalidFilename => ("ENAMETOOLONG", "name too long"),
        _ => return None,
    })
}

/// A file system failure worded as Node words it: `EACCES: permission denied,
/// mkdir '/app/tmp/mcp-sandboxes/7'`.
pub(crate) fn file_system_message(error: &io::Error, operation: &str, path: &Path) -> String {
    match system_error(error) {
        Some((code, description)) => {
            format!("{code}: {description}, {operation} '{}'", path.display())
        }
        None => format!("{error}, {operation} '{}'", path.display()),
    }
}

/// The name Node gives a spawn failure, as in `spawn /usr/local/bin/deno ENOENT`.
pub(crate) fn spawn_error_name(error: &io::Error) -> String {
    use io::ErrorKind;
    let name = match error.kind() {
        ErrorKind::NotFound => "ENOENT",
        ErrorKind::PermissionDenied => "EACCES",
        ErrorKind::NotADirectory => "ENOTDIR",
        ErrorKind::IsADirectory => "EISDIR",
        ErrorKind::ArgumentListTooLong => "E2BIG",
        ErrorKind::OutOfMemory => "ENOMEM",
        ErrorKind::InvalidFilename => "ENAMETOOLONG",
        ErrorKind::ExecutableFileBusy => "ETXTBSY",
        ErrorKind::WouldBlock => "EAGAIN",
        _ => return error.to_string(),
    };
    name.to_owned()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn resolves_paths_by_name_as_node_does() {
        let cwd = Path::new("/srv/app");
        let resolved = |path: &str| resolve(cwd, path);
        assert_eq!(
            resolved("relative/deno-cache"),
            Path::new("/srv/app/relative/deno-cache")
        );
        assert_eq!(
            resolved("/var/cache/server/"),
            Path::new("/var/cache/server")
        );
        assert_eq!(resolved(""), Path::new("/srv/app"));
        assert_eq!(resolved("./a/../b//c/."), Path::new("/srv/app/b/c"));
        assert_eq!(resolved("../../../.."), Path::new("/"));
        assert_eq!(resolved("/a/../../b"), Path::new("/b"));
        assert_eq!(
            resolved("//usr//bin/../bin/deno"),
            Path::new("/usr/bin/deno")
        );
        assert_eq!(normalize(Path::new("a/../../b")), Path::new("../b"));
    }

    #[test]
    fn tells_a_directory_and_what_lies_below_it_from_its_neighbours() {
        let within = |path: &str, directory: &str| is_within(Path::new(path), Path::new(directory));
        assert!(within("/app", "/app"));
        assert!(within("/app/tmp/mcp-sandboxes/7", "/app/tmp/mcp-sandboxes"));
        assert!(within("/app", "/"));
        assert!(!within(
            "/app/tmp/mcp-sandboxes-old",
            "/app/tmp/mcp-sandboxes"
        ));
        assert!(!within("/app/tmp", "/app/tmp/mcp-sandboxes"));
        assert!(!within("/application", "/app"));
    }

    #[test]
    fn words_file_system_failures_as_node_does() {
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(
            file_system_message(&denied, "mkdir", Path::new("/app/tmp/mcp-sandboxes/7")),
            "EACCES: permission denied, mkdir '/app/tmp/mcp-sandboxes/7'"
        );
        let full = io::Error::from(io::ErrorKind::StorageFull);
        assert_eq!(
            file_system_message(&full, "mkdir", Path::new("/x")),
            "ENOSPC: no space left on device, mkdir '/x'"
        );
        assert_eq!(
            spawn_error_name(&io::Error::from(io::ErrorKind::NotFound)),
            "ENOENT"
        );
    }
}
