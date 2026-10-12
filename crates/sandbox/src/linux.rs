//! The policy, applied with Landlock, seccomp and resource limits.

use std::io;
use std::path::Path;

use landlock::{
    ABI, Access, AccessFs, BitFlags, LandlockStatus, PathBeneath, PathFd, PathFdError, Ruleset,
    RulesetAttr, RulesetCreated, RulesetCreatedAttr, RulesetStatus, Scope,
};
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit, setsid};

use crate::{Enforcement, Limits, Policy, SandboxError, seccomp};

/// The version of Landlock these rules were written for and tested on. On an
/// older kernel the rights it lacks are left out, which only matters where
/// seccomp does not cover them: see the module documentation of the crate.
const LANDLOCK_ABI: ABI = ABI::V8;

/// What a dynamically linked program reads to start, resolve a host name and
/// verify a certificate. Read only; what a system does not have is skipped.
pub(crate) const SYSTEM_PATHS: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/etc/alternatives",
    "/etc/ld.so.cache",
    "/etc/ld.so.conf",
    "/etc/ld.so.conf.d",
    "/etc/ssl",
    "/etc/ca-certificates",
    "/etc/pki",
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/host.conf",
    "/etc/nsswitch.conf",
    "/etc/gai.conf",
    "/etc/services",
    "/etc/protocols",
    "/etc/passwd",
    "/etc/group",
    "/etc/localtime",
    "/etc/timezone",
    // What a runtime sizes its heap and its thread pools from. Nothing of
    // `/proc/<pid>` is here: that is where the server itself can be read.
    "/proc/cpuinfo",
    "/proc/meminfo",
    "/proc/stat",
    "/proc/loadavg",
    "/proc/uptime",
    "/proc/version",
    "/sys/devices/system/cpu",
];

/// Read and written: they hold nothing.
const DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
];

pub(crate) fn enforce(policy: &Policy) -> Result<Enforcement, SandboxError> {
    refuse_inherited_files()?;
    leave_the_session()?;
    limit_resources(&policy.limits)?;
    // Compiled before the files are closed off, installed after: Landlock
    // itself needs system calls the filter has no reason to refuse, but the
    // order keeps that true whatever the filter becomes.
    let filters = seccomp::filters(std::process::id())?;
    let landlock_abi = restrict_files(policy)?;
    seccomp::install(&filters)?;
    Ok(Enforcement { landlock_abi })
}

/// Landlock decides what a process opens, not what it holds open already: a
/// file the server had open without the close-on-exec flag would be the
/// program's to read and write, whatever the policy. The server opens none
/// that way; one that reaches here is refused rather than handed over.
/// Pipes and sockets are not files of the server and are let through: the
/// standard streams are pipes to the server.
fn refuse_inherited_files() -> Result<(), SandboxError> {
    // Without /proc there is nothing to look at, and no /proc for the
    // program to learn anything from either.
    let Ok(descriptors) = std::fs::read_dir("/proc/self/fd") else {
        return Ok(());
    };
    for descriptor in descriptors.flatten() {
        let number = descriptor.file_name().to_string_lossy().parse::<u32>();
        let Ok(path) = std::fs::read_link(descriptor.path()) else {
            continue;
        };
        // The standard streams, this listing itself, and devices such as a
        // terminal. What is not a path (`pipe:[7]`, `socket:[8]`) is no file.
        let passed_on = number.is_ok_and(|number| number > 2)
            && path.is_absolute()
            && !path.starts_with("/proc")
            && !path.starts_with("/dev");
        if passed_on {
            return Err(SandboxError::InheritedFile(path));
        }
    }
    Ok(())
}

/// A session and a process group of its own: what is sent to the group of
/// the server, or typed into its terminal, is not the program's to send.
fn leave_the_session() -> Result<(), SandboxError> {
    match setsid() {
        // Refused to the leader of a group, which is in the server's no more.
        Ok(_) | Err(rustix::io::Errno::PERM) => Ok(()),
        Err(error) => Err(SandboxError::Session(error.into())),
    }
}

fn limit_resources(limits: &Limits) -> Result<(), SandboxError> {
    for (resource, limit) in [
        (Resource::Core, 0),
        (Resource::Data, limits.memory_bytes),
        (Resource::Nofile, limits.open_files),
        (Resource::Nproc, limits.tasks),
    ] {
        // A limit can only come down: one the server already runs under is kept.
        let limit = match getrlimit(resource).maximum {
            Some(maximum) => limit.min(maximum),
            None => limit,
        };
        setrlimit(
            resource,
            Rlimit {
                current: Some(limit),
                maximum: Some(limit),
            },
        )
        .map_err(|error| SandboxError::Limits(error.into()))?;
    }
    Ok(())
}

fn landlock_error(error: impl std::fmt::Display) -> SandboxError {
    SandboxError::Landlock(error.to_string())
}

fn restrict_files(policy: &Policy) -> Result<u8, SandboxError> {
    let read = AccessFs::from_read(LANDLOCK_ABI);
    let devices = read | AccessFs::WriteFile;
    // Device nodes are of no use in a directory of an unprivileged user.
    let write = AccessFs::from_all(LANDLOCK_ABI) & !(AccessFs::MakeChar | AccessFs::MakeBlock);

    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(LANDLOCK_ABI))
        .map_err(landlock_error)?
        .scope(Scope::from_all(LANDLOCK_ABI))
        .map_err(landlock_error)?
        .create()
        .map_err(landlock_error)?;

    for path in SYSTEM_PATHS {
        ruleset = allow(ruleset, Path::new(path), read, Missing::Skipped)?;
    }
    for path in DEVICES {
        ruleset = allow(ruleset, Path::new(path), devices, Missing::Skipped)?;
    }
    for path in &policy.read {
        ruleset = allow(ruleset, path, read, Missing::Refused)?;
    }
    for path in &policy.write {
        ruleset = allow(ruleset, path, write, Missing::Refused)?;
    }

    let status = ruleset.restrict_self().map_err(landlock_error)?;
    let abi = match status.landlock {
        LandlockStatus::Available { effective_abi, .. } => abi_number(effective_abi),
        LandlockStatus::NotEnabled => {
            return Err(SandboxError::LandlockUnavailable(
                "it is built in but not enabled".to_owned(),
            ));
        }
        LandlockStatus::NotImplemented => {
            return Err(SandboxError::LandlockUnavailable(
                "it is not built in".to_owned(),
            ));
        }
    };
    if status.ruleset == RulesetStatus::NotEnforced || !status.no_new_privs {
        return Err(SandboxError::LandlockUnavailable(format!(
            "the rules were not applied on ABI {abi}"
        )));
    }
    Ok(abi)
}

/// The number the kernel documentation gives a version of Landlock.
fn abi_number(abi: ABI) -> u8 {
    [
        ABI::V1,
        ABI::V2,
        ABI::V3,
        ABI::V4,
        ABI::V5,
        ABI::V6,
        ABI::V7,
        ABI::V8,
    ]
    .iter()
    .filter(|known| **known <= abi)
    .count() as u8
}

enum Missing {
    Skipped,
    Refused,
}

fn allow(
    ruleset: RulesetCreated,
    path: &Path,
    access: BitFlags<AccessFs>,
    missing: Missing,
) -> Result<RulesetCreated, SandboxError> {
    let opened = match PathFd::new(path) {
        Ok(opened) => opened,
        Err(PathFdError::OpenCall { source, .. })
            if matches!(missing, Missing::Skipped) && source.kind() == io::ErrorKind::NotFound =>
        {
            return Ok(ruleset);
        }
        Err(PathFdError::OpenCall { source, .. }) => {
            return Err(SandboxError::Path {
                path: path.to_owned(),
                source,
            });
        }
        Err(error) => return Err(landlock_error(error)),
    };
    // The kernel refuses a rule that gives a file the rights of a directory.
    let access = if path.is_dir() {
        access
    } else {
        access & AccessFs::from_file(LANDLOCK_ABI)
    };
    ruleset
        .add_rule(PathBeneath::new(opened, access))
        .map_err(landlock_error)
}
