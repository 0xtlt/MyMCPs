//! The launcher, started as the server starts it, and what the kernel then
//! lets the program it became do. The program is a shell whose own commands
//! (`read`, `echo`, `kill`, `ulimit`) need no other process, or a tool the
//! shell becomes with `exec`.

use std::process::{Command, Output};

use mymcps_sandbox::{EXIT_NOT_SANDBOXED, Policy};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_mymcps-sandbox-exec");

fn launch(policy: &Policy, program: &str, arguments: &[&str]) -> Output {
    Command::new(LAUNCHER)
        .args(
            policy
                .launch_arguments(program, arguments.iter().copied())
                .unwrap(),
        )
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[cfg(not(target_os = "linux"))]
#[test]
fn starts_nothing_on_a_system_that_cannot_sandbox() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("ran");
    let policy = Policy::default().write(directory.path());

    let output = launch(
        &policy,
        "/bin/sh",
        &["-c", &format!("echo ran > '{}'", marker.display())],
    );

    assert_eq!(output.status.code(), Some(i32::from(EXIT_NOT_SANDBOXED)));
    assert_eq!(
        stderr(&output),
        "sandbox: the sandbox of npm MCPs needs Linux: this system has neither Landlock nor seccomp. See NPM_SANDBOX in the README\n"
    );
    assert!(!marker.exists());

    // Asked what it enforces, it says the same.
    let probe = Command::new(LAUNCHER)
        .args(policy.probe_arguments().unwrap())
        .output()
        .unwrap();
    assert_eq!(probe.status.code(), Some(i32::from(EXIT_NOT_SANDBOXED)));
    assert!(probe.stdout.is_empty());
}

#[cfg(target_os = "linux")]
mod on_linux {
    use std::path::Path;

    use mymcps_sandbox::{EXIT_NOT_STARTED, Limits};

    use super::*;

    /// A directory the program may write, one it may read, and one it is not
    /// given, which holds what the database is to an npm MCP.
    struct Machine {
        directory: tempfile::TempDir,
        policy: Policy,
    }

    impl Machine {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            for (name, content) in [
                ("sandbox/state", "of the package\n"),
                ("package/asset", "packaged asset\n"),
                ("server/database", "of the server\n"),
            ] {
                let path = directory.path().join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, content).unwrap();
            }
            let policy = Policy::default()
                .read(directory.path().join("package"))
                .write(directory.path().join("sandbox"));
            Self { directory, policy }
        }

        fn path(&self, name: &str) -> String {
            self.directory.path().join(name).display().to_string()
        }

        fn shell(&self, script: &str) -> Output {
            launch(&self.policy, "/bin/sh", &["-c", script])
        }

        /// Whether the shell, sandboxed, got through `script`.
        fn can(&self, script: &str) -> bool {
            self.shell(script).status.success()
        }
    }

    #[test]
    fn says_what_the_kernel_enforces() {
        let probe = Command::new(LAUNCHER)
            .args(Policy::default().probe_arguments().unwrap())
            .output()
            .unwrap();

        assert!(probe.status.success(), "{}", stderr(&probe));
        let enforced = String::from_utf8(probe.stdout).unwrap();
        assert!(enforced.starts_with("Landlock ABI "), "{enforced}");
        assert!(
            enforced.ends_with(", seccomp, resource limits\n"),
            "{enforced}"
        );
    }

    #[test]
    fn reads_what_it_is_given_and_the_system_and_nothing_else() {
        let machine = Machine::new();
        let read = |path: &str| {
            let output = machine.shell(&format!("read line < '{path}' && echo \"$line\""));
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };

        assert_eq!(
            read(&machine.path("package/asset")).as_deref(),
            Some("packaged asset")
        );
        assert_eq!(
            read(&machine.path("sandbox/state")).as_deref(),
            Some("of the package")
        );
        assert!(read("/etc/passwd").is_some());

        // The file of the server, which the user of the server can read.
        let database = machine.path("server/database");
        assert_eq!(
            std::fs::read_to_string(&database).unwrap(),
            "of the server\n"
        );
        let refused = machine.shell(&format!("read line < '{database}'"));
        assert!(!refused.status.success());
        assert!(
            stderr(&refused).contains("Permission denied"),
            "{}",
            stderr(&refused)
        );
        // Nor what it would learn the names of the other directories from.
        assert!(!machine.can(&format!("exec ls '{}'", machine.path(""))));
        assert!(!machine.can(&format!("exec ls '{}'", machine.path("server"))));
        assert!(machine.can(&format!("exec ls '{}'", machine.path("package"))));
        // Nor the server itself, through the files the kernel describes it with.
        assert!(std::fs::read("/proc/self/environ").is_ok());
        assert!(!machine.can("read line < /proc/$PPID/environ"));
        assert!(!machine.can("read line < /proc/$PPID/cmdline"));
        assert!(!machine.can("read line < /proc/self/status"));
    }

    #[test]
    fn writes_the_directory_it_is_given_and_no_other() {
        let machine = Machine::new();

        assert!(machine.can(&format!(
            "echo written > '{0}' && echo more > '{1}' && exec mkdir '{2}'",
            machine.path("sandbox/state"),
            machine.path("sandbox/new"),
            machine.path("sandbox/directory"),
        )));
        assert_eq!(
            std::fs::read_to_string(machine.path("sandbox/state")).unwrap(),
            "written\n"
        );
        assert!(Path::new(&machine.path("sandbox/directory")).is_dir());

        for refused in [
            "package/asset",
            "package/new",
            "server/database",
            "server/new",
            "new",
        ] {
            assert!(
                !machine.can(&format!("echo written > '{}'", machine.path(refused))),
                "{refused}"
            );
        }
        assert!(!machine.can(&format!("exec rm '{}'", machine.path("server/database"))));
        assert!(!machine.can(&format!("exec rm '{}'", machine.path("package/asset"))));
        assert!(!machine.can(&format!(
            "exec mv '{}' '{}'",
            machine.path("server/database"),
            machine.path("sandbox/taken")
        )));
        assert!(!machine.can(&format!(
            "exec ln -s '{}' '{}'",
            machine.path("sandbox/state"),
            machine.path("server/link")
        )));
        assert!(!machine.can(&format!(
            "exec truncate -s 0 '{}'",
            machine.path("server/database")
        )));
        assert_eq!(
            std::fs::read_to_string(machine.path("server/database")).unwrap(),
            "of the server\n"
        );
        assert_eq!(
            std::fs::read_to_string(machine.path("package/asset")).unwrap(),
            "packaged asset\n"
        );
        assert!(!Path::new(&machine.path("server/new")).exists());
    }

    /// Landlock checks a file when it is opened. The mode and the owner of a
    /// file are changed without opening it, and the files of the server
    /// belong to the user a package runs as.
    #[test]
    fn changes_the_mode_of_no_file_by_its_path() {
        use std::os::unix::fs::PermissionsExt;

        let machine = Machine::new();
        let mode = |name: &str| {
            std::fs::metadata(machine.path(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        let database = mode("server/database");

        for name in [
            "server/database",
            "server",
            "package/asset",
            "sandbox/state",
        ] {
            let before = mode(name);
            let refused = machine.shell(&format!("exec chmod 000 '{}'", machine.path(name)));
            assert!(!refused.status.success(), "{name}");
            assert!(
                stderr(&refused).contains("Operation not permitted"),
                "{name}: {}",
                stderr(&refused)
            );
            assert_eq!(mode(name), before, "{name}");
        }
        assert_eq!(mode("server/database"), database);
    }

    #[test]
    fn starts_no_other_process_and_signals_none() {
        let machine = Machine::new();

        // The shell becomes another program, which is still in the sandbox.
        let became = machine.shell("exec echo became");
        assert_eq!(String::from_utf8_lossy(&became.stdout), "became\n");
        assert!(!machine.can(&format!(
            "exec sh -c \"echo written > '{}'\"",
            machine.path("server/new")
        )));

        // It starts none beside itself.
        let forked = machine.shell("ls /; echo still here");
        assert!(
            !String::from_utf8_lossy(&forked.stdout).contains("usr"),
            "{forked:?}"
        );
        assert!(!machine.can("(echo in a subshell)"));
        assert!(!machine.can("echo piped | read line"));

        // The process that started it is the server.
        assert!(machine.can("kill -0 $$"));
        for other in ["$PPID", "1", "0", "-1"] {
            let refused = machine.shell(&format!("kill -0 {other}"));
            assert!(!refused.status.success(), "{other}");
            assert!(
                stderr(&refused).contains("not permitted"),
                "{other}: {}",
                stderr(&refused)
            );
        }
    }

    #[test]
    fn holds_the_limits_of_its_policy() {
        let mut machine = Machine::new();
        machine.policy.limits = Limits {
            memory_bytes: 512 * 1024 * 1024,
            open_files: 64,
            tasks: 2048,
        };

        let limits = machine.shell("ulimit -d; ulimit -n; ulimit -c");

        assert!(limits.status.success(), "{}", stderr(&limits));
        assert_eq!(
            String::from_utf8_lossy(&limits.stdout)
                .lines()
                .collect::<Vec<_>>(),
            ["524288", "64", "0"]
        );
        // They only come down.
        assert!(!machine.can("ulimit -d unlimited"));
        assert!(!machine.can("ulimit -n 4096"));
    }

    /// Landlock checks what a program opens. What it was handed open is
    /// outside of every rule.
    #[test]
    fn takes_no_open_file_from_the_process_that_starts_it() {
        let machine = Machine::new();
        let database = machine.path("server/database");
        // A shell opens descriptor 3 and becomes the launcher, as a server
        // that forgot the close-on-exec flag would start it.
        let handed = |redirection: &str, script: &str| {
            Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("exec \"$0\" \"$@\" {redirection}"))
                .arg(LAUNCHER)
                .args(
                    machine
                        .policy
                        .launch_arguments("/bin/sh", ["-c", script])
                        .unwrap(),
                )
                .env_clear()
                .output()
                .unwrap()
        };

        let refused = handed(
            &format!("3< '{database}'"),
            "read line <&3 && echo \"$line\"",
        );
        assert_eq!(refused.status.code(), Some(i32::from(EXIT_NOT_SANDBOXED)));
        assert!(refused.stdout.is_empty());
        assert_eq!(
            stderr(&refused),
            format!(
                "sandbox: \"{database}\" is open in the server and would stay open in the sandboxed program\n"
            )
        );

        // A pipe is how the server talks to the program.
        let piped = handed("3>&1", "echo through >&3");
        assert!(piped.status.success(), "{}", stderr(&piped));
        assert_eq!(String::from_utf8_lossy(&piped.stdout), "through\n");
    }

    #[test]
    fn starts_nothing_it_cannot_hold() {
        let machine = Machine::new();
        let marker = machine.path("sandbox/ran");
        let script = format!("echo ran > '{marker}'");

        // A directory of the policy that does not exist is a mistake of the
        // server, not a rule to leave out.
        for policy in [
            machine.policy.clone().write(machine.path("missing")),
            machine.policy.clone().read(machine.path("missing")),
        ] {
            let output = launch(&policy, "/bin/sh", &["-c", &script]);
            assert_eq!(output.status.code(), Some(i32::from(EXIT_NOT_SANDBOXED)));
            assert!(
                stderr(&output).starts_with(&format!(
                    "sandbox: \"{}\" cannot be given to the sandbox: ",
                    machine.path("missing")
                )),
                "{}",
                stderr(&output)
            );
        }
        let unreadable = Command::new(LAUNCHER)
            .arg("{not a policy")
            .output()
            .unwrap();
        assert_eq!(
            unreadable.status.code(),
            Some(i32::from(EXIT_NOT_SANDBOXED))
        );
        assert!(stderr(&unreadable).starts_with("sandbox: the policy cannot be read: "));
        assert!(!Path::new(&marker).exists());

        // A program that is not there, or that the sandbox may not run.
        let missing = launch(&machine.policy, "/nonexistent/node", &[]);
        assert_eq!(missing.status.code(), Some(i32::from(EXIT_NOT_STARTED)));
        assert!(
            stderr(&missing).starts_with("sandbox: spawn /nonexistent/node "),
            "{}",
            stderr(&missing)
        );
        let outside = machine.path("server/program");
        std::fs::copy("/bin/sh", &outside).unwrap();
        let refused = launch(&machine.policy, &outside, &["-c", &script]);
        assert_eq!(refused.status.code(), Some(i32::from(EXIT_NOT_STARTED)));
        assert!(!Path::new(&marker).exists());
    }
}
