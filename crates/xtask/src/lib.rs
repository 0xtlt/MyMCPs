//! Release tooling for MyMCPs: the version of the Cargo workspace, the nightly
//! version derived from it, and the files a stable release changes.
//!
//! The release workflows run it as `cargo run --locked -p xtask -- <command>`.
//! It uses the standard library only, so the job that holds the write token
//! compiles nothing but this crate.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

mod cargo_files;
mod changelog;
pub mod cli;
mod release;
mod version;

pub use cargo_files::{set_workspace_version, update_lockfile, workspace_version};
pub use changelog::{ChangelogRelease, update_changelog};
pub use release::{PreparedRelease, ReleaseOptions, prepare_release};
pub use version::{bump_version, nightly_version};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// An argument or a file the tool refuses to work with.
    Invalid(String),
    /// A command line that names no command, or leaves out one of its options.
    Usage(String),
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Usage(message) => formatter.write_str(message),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Invalid(_) | Self::Usage(_) => None,
        }
    }
}

/// The root of the Cargo workspace this crate was built from.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// True when `value` has exactly the shape of `shape`, where a `d` in the
/// shape stands for one ASCII digit and any other byte for itself.
fn has_shape(value: &str, shape: &str) -> bool {
    value.len() == shape.len()
        && shape
            .bytes()
            .zip(value.bytes())
            .all(|(expected, byte)| match expected {
                b'd' => byte.is_ascii_digit(),
                literal => literal == byte,
            })
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}
