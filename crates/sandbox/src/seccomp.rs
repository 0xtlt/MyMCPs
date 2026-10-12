//! The system calls a sandboxed process is refused.
//!
//! Landlock decides which files a process opens, creates and removes. It
//! says nothing of the rest of what a process of the server's own user can
//! do to the server: signal it, change the mode of its files, trace it. This
//! filter refuses that, and what no package needs: other processes, Unix and
//! raw sockets, and the calls that administer a machine.
//!
//! It is a list of refusals, not of permissions: a runtime such as Node
//! calls a few hundred different system calls, and which ones changes with
//! each of its versions.

use std::collections::BTreeMap;

use libc::c_long;
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use crate::SandboxError;

type Rules = BTreeMap<i64, Vec<SeccompRule>>;

// Calls every architecture numbers alike, newer than some C libraries name.
const SYS_FCHMODAT2: c_long = 452;

// Commands of `fcntl` and `ioctl` that name the process a file signals, the
// same on the two architectures the filter is compiled for.
const F_SETOWN_EX: u64 = 15;
const FIOSETOWN: u64 = 0x8901;
const SIOCSPGRP: u64 = 0x8902;
// The bits of the type of a socket that are not flags.
const SOCK_TYPE_MASK: u64 = 0xf;

fn seccomp_error(error: impl std::fmt::Display) -> SandboxError {
    SandboxError::Seccomp(error.to_string())
}

/// Refused whatever their arguments.
fn refused_calls() -> Vec<c_long> {
    let mut calls = vec![
        // Other processes of the same user: the server, and the packages of
        // other MCPs.
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_process_madvise,
        libc::SYS_process_mrelease,
        libc::SYS_pidfd_getfd,
        // A signal through a descriptor of the process itself, the only one
        // it opens, also reaches its process group since Linux 6.9.
        libc::SYS_pidfd_send_signal,
        libc::SYS_kcmp,
        // A thread id cannot be told from the one of another process.
        libc::SYS_tkill,
        // What Landlock does not check: a file is named, never opened. The
        // server's files belong to the user the package runs as.
        libc::SYS_fchmodat,
        SYS_FCHMODAT2,
        libc::SYS_fchownat,
        libc::SYS_setxattr,
        libc::SYS_lsetxattr,
        libc::SYS_removexattr,
        libc::SYS_lremovexattr,
        // Before Linux 6.2 Landlock does not check truncation either.
        libc::SYS_truncate,
        // Files reached by another way than their path.
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
        // Namespaces and mounts.
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_mount_setattr,
        libc::SYS_move_mount,
        libc::SYS_open_tree,
        libc::SYS_fsopen,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_fspick,
        // The kernel and the machine.
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_userfaultfd,
        libc::SYS_kexec_load,
        libc::SYS_kexec_file_load,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_reboot,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_acct,
        libc::SYS_quotactl,
        libc::SYS_quotactl_fd,
        libc::SYS_syslog,
        libc::SYS_settimeofday,
        libc::SYS_clock_settime,
        libc::SYS_clock_adjtime,
        libc::SYS_adjtimex,
        libc::SYS_sethostname,
        libc::SYS_setdomainname,
        libc::SYS_vhangup,
        libc::SYS_personality,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
    ];
    calls.extend(older_calls());
    calls
}

/// The calls newer architectures left out, for which the list above names
/// the one that replaced them.
#[cfg(target_arch = "x86_64")]
fn older_calls() -> Vec<c_long> {
    vec![
        libc::SYS_fork,
        libc::SYS_vfork,
        libc::SYS_chmod,
        libc::SYS_chown,
        libc::SYS_lchown,
        libc::SYS_iopl,
        libc::SYS_ioperm,
        libc::SYS_modify_ldt,
    ]
}

#[cfg(not(target_arch = "x86_64"))]
fn older_calls() -> Vec<c_long> {
    Vec::new()
}

/// Answered as a kernel without them answers, so that a C library or an
/// event loop falls back on what it did before they existed: `clone` for a
/// thread, which the filter can tell from a process, and `epoll`.
fn missing_calls() -> Vec<c_long> {
    vec![
        libc::SYS_clone3,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ]
}

fn condition(
    argument: u8,
    length: SeccompCmpArgLen,
    operator: SeccompCmpOp,
    value: u64,
) -> Result<SeccompCondition, SandboxError> {
    SeccompCondition::new(argument, length, operator, value).map_err(seccomp_error)
}

/// One refusal: every condition holds.
fn rule(conditions: Vec<SeccompCondition>) -> Result<SeccompRule, SandboxError> {
    SeccompRule::new(conditions).map_err(seccomp_error)
}

fn refusals(own_pid: u32) -> Result<Rules, SandboxError> {
    use SeccompCmpArgLen::{Dword, Qword};
    use SeccompCmpOp::{Eq, MaskedEq, Ne};

    let mut rules: Rules = refused_calls()
        .into_iter()
        .map(|call| (call, Vec::new()))
        .collect();

    // A thread, never a process: nothing started by a package outlives the
    // request it was started for, and a package starts no program the
    // sandbox was not written for.
    let mut clone = vec![rule(vec![condition(
        0,
        Qword,
        MaskedEq(libc::CLONE_THREAD as u64),
        0,
    )?])?];
    for namespace in [
        libc::CLONE_NEWNS,
        libc::CLONE_NEWUSER,
        libc::CLONE_NEWPID,
        libc::CLONE_NEWNET,
        libc::CLONE_NEWIPC,
        libc::CLONE_NEWUTS,
        libc::CLONE_NEWCGROUP,
        libc::CLONE_NEWTIME,
    ] {
        let namespace = namespace as u64;
        clone.push(rule(vec![condition(
            0,
            Qword,
            MaskedEq(namespace),
            namespace,
        )?])?);
    }
    rules.insert(libc::SYS_clone, clone);

    // Signals for itself alone. The server is a process of the same user,
    // so the kernel would deliver it a SIGKILL from a package. With no other
    // process of its own, a package has nobody else to signal.
    let another_process = || rule(vec![condition(0, Dword, Ne, u64::from(own_pid))?]);
    for call in [
        libc::SYS_kill,
        libc::SYS_tgkill,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_rt_tgsigqueueinfo,
        libc::SYS_pidfd_open,
    ] {
        rules.insert(call, vec![another_process()?]);
    }

    // A file also signals: the process named its owner when it can be read
    // or written, with the signal of the owner's choice. A package's files
    // signal the package, which they do when they are given no owner.
    let command = |value: u64| rule(vec![condition(1, Dword, Eq, value)?]);
    rules.insert(
        libc::SYS_fcntl,
        vec![command(libc::F_SETOWN as u64)?, command(F_SETOWN_EX)?],
    );
    rules.insert(
        libc::SYS_ioctl,
        vec![command(FIOSETOWN)?, command(SIOCSPGRP)?],
    );

    // The Internet, and the kernel's answers about the network interfaces
    // and routes (netlink), which a resolver asks for. Not Unix sockets,
    // which reach the services of the machine by a path Landlock does not
    // check before Linux 7.1, nor raw and packet sockets, nor the rest of
    // netlink: the sockets of other processes, the firewall, the audit log.
    rules.insert(
        libc::SYS_socket,
        vec![
            rule(vec![
                condition(0, Dword, Ne, libc::AF_INET as u64)?,
                condition(0, Dword, Ne, libc::AF_INET6 as u64)?,
                condition(0, Dword, Ne, libc::AF_NETLINK as u64)?,
            ])?,
            rule(vec![
                condition(0, Dword, Eq, libc::AF_NETLINK as u64)?,
                condition(2, Dword, Ne, libc::NETLINK_ROUTE as u64)?,
            ])?,
        ],
    );
    // A pair of connected sockets, which is how a runtime talks to itself,
    // is of the Unix family and reaches nothing as long as it is a stream:
    // a datagram socket of a pair still sends to the socket of a service
    // that has a path.
    let mut socketpair = vec![rule(vec![condition(0, Dword, Ne, libc::AF_UNIX as u64)?])?];
    for kind in (0..=SOCK_TYPE_MASK).filter(|kind| *kind != libc::SOCK_STREAM as u64) {
        socketpair.push(rule(vec![condition(
            1,
            Dword,
            MaskedEq(SOCK_TYPE_MASK),
            kind,
        )?])?);
    }
    rules.insert(libc::SYS_socketpair, socketpair);

    Ok(rules)
}

fn compile(rules: Rules, errno: i32, architecture: TargetArch) -> Result<BpfProgram, SandboxError> {
    SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(errno as u32),
        architecture,
    )
    .and_then(BpfProgram::try_from)
    .map_err(seccomp_error)
}

/// The filters of a process whose id is `own_pid`. A system call is refused
/// when any of them refuses it.
pub(crate) fn filters(own_pid: u32) -> Result<Vec<BpfProgram>, SandboxError> {
    let architecture = TargetArch::try_from(std::env::consts::ARCH).map_err(seccomp_error)?;
    let missing: Rules = missing_calls()
        .into_iter()
        .map(|call| (call, Vec::new()))
        .collect();

    let mut filters = vec![
        compile(refusals(own_pid)?, libc::EPERM, architecture)?,
        compile(missing, libc::ENOSYS, architecture)?,
    ];
    filters.extend(second_numbering());
    Ok(filters)
}

/// An x86-64 kernel can take the calls of 32-bit pointers programs (x32)
/// under other numbers, with bit 30 set, and reports them as the same
/// architecture: a list of refused numbers would not see them. Nothing the
/// sandbox runs is such a program.
#[cfg(target_arch = "x86_64")]
fn second_numbering() -> Option<BpfProgram> {
    use seccompiler::sock_filter;

    const X32_SYSCALL_BIT: u32 = 0x4000_0000;
    // The classic BPF instructions: load a word of the call's description,
    // jump when it is greater or equal, return.
    const LOAD_WORD_AT: u16 = 0x20;
    const JUMP_IF_GREATER_OR_EQUAL: u16 = 0x35;
    const RETURN: u16 = 0x06;
    let instruction = |code, jt, jf, k| sock_filter { code, jt, jf, k };

    Some(vec![
        // The number of the system call is the first field.
        instruction(LOAD_WORD_AT, 0, 0, 0),
        instruction(JUMP_IF_GREATER_OR_EQUAL, 0, 1, X32_SYSCALL_BIT),
        instruction(RETURN, 0, 0, libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32),
        instruction(RETURN, 0, 0, libc::SECCOMP_RET_ALLOW),
    ])
}

#[cfg(not(target_arch = "x86_64"))]
fn second_numbering() -> Option<BpfProgram> {
    None
}

pub(crate) fn install(filters: &[BpfProgram]) -> Result<(), SandboxError> {
    for filter in filters {
        seccompiler::apply_filter(filter).map_err(seccomp_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_for_this_architecture() {
        let filters = filters(4242).unwrap();
        assert!(filters.len() >= 2);
        assert!(filters.iter().all(|filter| !filter.is_empty()));
    }

    #[test]
    fn refuses_processes_and_lets_threads_be_made() {
        let rules = refusals(4242).unwrap();
        // One rule for a clone that is not a thread, one for each namespace.
        assert_eq!(rules[&libc::SYS_clone].len(), 9);
        for call in [
            libc::SYS_ptrace,
            libc::SYS_fchmodat,
            libc::SYS_truncate,
            libc::SYS_pidfd_send_signal,
        ] {
            assert!(
                rules[&call].is_empty(),
                "{call} is refused whatever it is asked"
            );
        }
        // Sockets: a family that is not allowed, or netlink for more than routes.
        assert_eq!(rules[&libc::SYS_socket].len(), 2);
        // Pairs: another family, or any type that is not a stream.
        assert_eq!(rules[&libc::SYS_socketpair].len(), 16);
        // The two commands that name the process a file signals.
        assert_eq!(rules[&libc::SYS_fcntl].len(), 2);
        assert_eq!(rules[&libc::SYS_ioctl].len(), 2);
        assert!(!rules.contains_key(&libc::SYS_execve));
        assert!(!rules.contains_key(&libc::SYS_connect));
    }
}
