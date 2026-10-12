//! The sandbox of the processes MyMCPs starts for other people's code: the
//! package of an npm MCP, and the `npm` that installs it.
//!
//! Such a process is started through a launcher, `mymcps sandbox:exec
//! <policy> <program> <argument>...`. The launcher applies the [`Policy`] to
//! itself and then becomes the program, so the program never runs a single
//! instruction outside the sandbox. Four features of the Linux kernel hold
//! it, none of which needs a privilege, so they work for an unprivileged user
//! in a container without capabilities:
//!
//! - **Landlock** decides which files it reaches. Everything is refused but
//!   the system files a program needs to start (`/usr`, the resolver
//!   configuration, the certificates...), the paths of [`Policy::read`], read
//!   only, and the paths of [`Policy::write`]. The database, the key, `.env`
//!   and the directories of other MCPs are in neither. Where the kernel has
//!   them (Linux 6.12), Landlock scopes also keep its signals and abstract
//!   Unix sockets inside the sandbox.
//! - **seccomp** refuses the system calls Landlock does not cover and that
//!   are no use to a package: starting another process, signalling a process
//!   other than itself, by a call or through a file that signals its owner,
//!   changing the mode or the owner of a file by its path, Unix and raw
//!   sockets, tracing, mounting, namespaces, and the rest of what
//!   administers a machine.
//! - **a session of its own** takes it out of the process group and off the
//!   terminal of the server.
//! - **resource limits** cap its memory, its threads and its open files.
//!
//! The network stays open: a package reaches whatever the server reaches.
//!
//! The sandbox fails closed. A kernel without Landlock or seccomp, or another
//! system than Linux, makes the launcher exit with an error instead of
//! starting the program.
//!
//! ```no_run
//! use std::process::Command;
//!
//! use mymcps_sandbox::{LAUNCH_COMMAND, Policy};
//!
//! let policy = Policy::default()
//!     .read("/usr/local/lib/node_modules/npm")
//!     .write("/app/tmp/mcp-sandboxes/7");
//! let status = Command::new("/usr/local/bin/mymcps")
//!     .arg(LAUNCH_COMMAND)
//!     .args(policy.launch_arguments("/usr/local/bin/node", ["server.js"]).unwrap())
//!     .status();
//! ```

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod seccomp;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

/// The command of `mymcps` that is the launcher.
pub const LAUNCH_COMMAND: &str = "sandbox:exec";

/// What the launcher exits with when it could not apply the policy.
pub const EXIT_NOT_SANDBOXED: u8 = 126;
/// What the launcher exits with when the sandboxed program could not be started.
pub const EXIT_NOT_STARTED: u8 = 127;

/// What a sandboxed process may reach beyond the system files every program
/// needs to start.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Policy {
    /// Files and directories it may read, list and execute. Must exist.
    pub read: Vec<PathBuf>,
    /// Directories it may read and change as it likes. Must exist.
    pub write: Vec<PathBuf>,
    pub limits: Limits,
}

/// The resource limits of a sandboxed process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// The memory the process may hold in its heap, its stacks and its
    /// private mappings (`RLIMIT_DATA`). Address space a runtime reserves
    /// without using it does not count.
    pub memory_bytes: u64,
    /// `RLIMIT_NOFILE`.
    pub open_files: u64,
    /// The threads and processes the user of the server may have in all
    /// (`RLIMIT_NPROC`): the kernel counts them per user, not per process.
    pub tasks: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 2 * 1024 * 1024 * 1024,
            open_files: 4096,
            tasks: 4096,
        }
    }
}

impl Policy {
    pub fn read(mut self, path: impl Into<PathBuf>) -> Self {
        self.read.push(path.into());
        self
    }

    pub fn write(mut self, path: impl Into<PathBuf>) -> Self {
        self.write.push(path.into());
        self
    }

    /// What follows [`LAUNCH_COMMAND`] on the command line of the launcher
    /// to run `program` under this policy.
    pub fn launch_arguments<P, I, S>(
        &self,
        program: P,
        arguments: I,
    ) -> Result<Vec<OsString>, SandboxError>
    where
        P: Into<OsString>,
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut launch = vec![OsString::from(self.to_argument()?), program.into()];
        launch.extend(arguments.into_iter().map(Into::into));
        Ok(launch)
    }

    /// What follows [`LAUNCH_COMMAND`] to apply this policy and start
    /// nothing: the launcher prints the [`Enforcement`] it got and exits.
    pub fn probe_arguments(&self) -> Result<Vec<OsString>, SandboxError> {
        Ok(vec![OsString::from(self.to_argument()?)])
    }

    fn to_argument(&self) -> Result<String, SandboxError> {
        serde_json::to_string(self).map_err(|error| SandboxError::Policy(error.to_string()))
    }
}

/// What the kernel enforces on a sandboxed process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enforcement {
    /// The version of Landlock in use: the older of the kernel's and of the
    /// one this crate was written for.
    pub landlock_abi: u8,
}

impl std::fmt::Display for Enforcement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Landlock ABI {}, seccomp, resource limits",
            self.landlock_abi
        )
    }
}

/// Why a process is not sandboxed. The launcher starts nothing then.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error(
        "the sandbox of npm MCPs needs Linux: this system has neither Landlock nor seccomp. See NPM_SANDBOX in the README"
    )]
    UnsupportedSystem,

    #[error("the policy cannot be read: {0}")]
    Policy(String),

    /// The kernel was built without Landlock, or starts without it.
    #[error(
        "the kernel does not enforce Landlock ({0}). The sandbox of npm MCPs needs Linux 5.13 or later with landlock in its list of security modules (lsm=), and a container runtime that lets the system calls of Landlock through. See NPM_SANDBOX in the README"
    )]
    LandlockUnavailable(String),

    #[error("Landlock refused the rules of the sandbox: {0}")]
    Landlock(String),

    #[error("\"{}\" cannot be given to the sandbox: {source}", .path.display())]
    Path {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The process that started the launcher handed it an open file.
    #[error(
        "\"{}\" is open in the server and would stay open in the sandboxed program",
        .0.display()
    )]
    InheritedFile(PathBuf),

    #[error("the program cannot leave the session of the server: {0}")]
    Session(#[source] std::io::Error),

    #[error("the seccomp filter cannot be installed: {0}")]
    Seccomp(String),

    #[error("the resource limits cannot be set: {0}")]
    Limits(#[source] std::io::Error),
}

/// The files and directories of the system every sandboxed process may read,
/// whatever its policy. Nothing the server keeps to itself may lie in one of
/// them: [`exposed_by_the_system`] tells.
pub fn system_paths() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    {
        linux::SYSTEM_PATHS
    }
    #[cfg(not(target_os = "linux"))]
    {
        &[]
    }
}

/// The path of `system_paths` that `directory` is, or lies in, links
/// followed: a sandboxed process would read `directory` through it.
pub fn exposed_by<'a>(system_paths: &[&'a str], directory: &Path) -> Option<&'a str> {
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let directory = real(directory);
    system_paths
        .iter()
        .copied()
        .find(|path| directory.starts_with(real(Path::new(path))))
}

/// [`exposed_by`] the paths of [`system_paths`].
pub fn exposed_by_the_system(directory: &Path) -> Option<&'static str> {
    exposed_by(system_paths(), directory)
}

/// Apply `policy` to this process, for good: it cannot be lifted, and every
/// process started from here on inherits it.
pub fn enforce(policy: &Policy) -> Result<Enforcement, SandboxError> {
    #[cfg(target_os = "linux")]
    {
        linux::enforce(policy)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = policy;
        Err(SandboxError::UnsupportedSystem)
    }
}

/// The launcher: `arguments` are a policy, then the program to become and its
/// own arguments. Returns only when the program was not started. Without a
/// program, the policy is applied and what the kernel enforces is printed.
///
/// Call it before the process has a second thread: the sandbox is applied to
/// the calling thread.
pub fn launch<I>(arguments: I) -> ExitCode
where
    I: IntoIterator<Item = OsString>,
{
    let mut arguments = arguments.into_iter();
    let policy = arguments
        .next()
        .ok_or_else(|| SandboxError::Policy("it is missing".to_owned()))
        .and_then(|policy| {
            let policy = policy.to_string_lossy();
            serde_json::from_str::<Policy>(&policy)
                .map_err(|error| SandboxError::Policy(error.to_string()))
        });
    let enforcement = match policy.and_then(|policy| enforce(&policy)) {
        Ok(enforcement) => enforcement,
        Err(error) => {
            eprintln!("sandbox: {error}");
            return ExitCode::from(EXIT_NOT_SANDBOXED);
        }
    };

    let Some(program) = arguments.next() else {
        println!("{enforcement}");
        return ExitCode::SUCCESS;
    };
    let error = become_program(program.clone(), arguments);
    eprintln!("sandbox: spawn {} {error}", program.to_string_lossy());
    ExitCode::from(EXIT_NOT_STARTED)
}

#[cfg(unix)]
fn become_program(program: OsString, arguments: impl Iterator<Item = OsString>) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(program).args(arguments).exec()
}

#[cfg(not(unix))]
fn become_program(
    _program: OsString,
    _arguments: impl Iterator<Item = OsString>,
) -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puts_the_policy_before_the_program_and_its_arguments() {
        let policy = Policy::default()
            .read("/opt/node")
            .write("/app/tmp/mcp-sandboxes/7");
        let launch = policy
            .launch_arguments("/usr/local/bin/node", ["server.js", "--flag"])
            .unwrap();

        assert_eq!(launch.len(), 4);
        assert_eq!(
            serde_json::from_str::<Policy>(launch[0].to_str().unwrap()).unwrap(),
            policy
        );
        assert_eq!(launch[1..], ["/usr/local/bin/node", "server.js", "--flag"]);
        assert_eq!(policy.probe_arguments().unwrap(), launch[..1]);
    }

    #[test]
    fn limits_memory_threads_and_open_files_by_default() {
        assert_eq!(
            Policy::default().limits,
            Limits {
                memory_bytes: 2 * 1024 * 1024 * 1024,
                open_files: 4096,
                tasks: 4096,
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn tells_a_directory_a_sandboxed_process_would_read_through_the_system_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        for name in ["usr/local/mymcps/tmp", "app/tmp", "usrdata"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        let usr = root.join("usr").display().to_string();
        let missing = root.join("lib64").display().to_string();
        let system = [missing.as_str(), usr.as_str()];
        let exposed = |name: &str| exposed_by(&system, &root.join(name));

        assert_eq!(exposed("usr/local/mymcps/tmp"), Some(usr.as_str()));
        assert_eq!(exposed("usr"), Some(usr.as_str()));
        // A directory that does not exist yet is where it will be.
        assert_eq!(exposed("usr/local/later"), Some(usr.as_str()));
        assert_eq!(exposed("app/tmp"), None);
        assert_eq!(exposed("usrdata"), None);

        // Reached through a link, it is where the link leads.
        std::os::unix::fs::symlink(root.join("usr/local/mymcps"), root.join("linked")).unwrap();
        assert_eq!(exposed("linked/tmp"), Some(usr.as_str()));
        std::os::unix::fs::symlink(root.join("app"), root.join("usr/app")).unwrap();
        assert_eq!(exposed("usr/app/tmp"), None);

        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                exposed_by_the_system(Path::new("/usr/local/app")),
                Some("/usr")
            );
            assert_eq!(exposed_by_the_system(Path::new("/app/tmp")), None);
            assert_eq!(exposed_by_the_system(Path::new("/srv/mymcps")), None);
        }
    }

    #[test]
    fn starts_nothing_without_a_policy_it_can_read() {
        for arguments in [vec![], vec!["{not json"], vec!["{}", "/bin/true"]] {
            let code = launch(arguments.into_iter().map(OsString::from));
            assert_eq!(code, ExitCode::from(EXIT_NOT_SANDBOXED));
        }
    }
}
