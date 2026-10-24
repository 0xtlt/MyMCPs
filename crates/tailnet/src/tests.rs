use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use url::Url;

use crate::supervisor::Backoff;
use crate::test_support::{
    FakeLocalApi, eventually, fake_tailscaled, running_status, spawn_echo_server, status_in,
};
use crate::{ConnectOptions, Error, TailnetState, Tailnets, TailnetsConfig};

// Fake keys are written in two pieces so that no line of the source reads as a
// Tailscale API key to a secret scanner. They still start with `tskey-`.
const KEY: &str = concat!("tskey", "-auth-kABCDEF123456-secretsecretsecret");
const SHORT_KEY: &str = concat!("tskey", "-auth-x");
const SECRET: &str = concat!("tskey", "-auth-kSECRET123-0123456789abcdef");

const SECOND: Duration = Duration::from_secs(1);
const PATIENCE: Duration = Duration::from_secs(10);

const FAST: Backoff = Backoff {
    initial: Duration::from_millis(20),
    max: Duration::from_millis(100),
    healthy_after: Duration::from_secs(60),
};

struct Fixture {
    dir: TempDir,
    tailnets: Tailnets,
}

impl Fixture {
    /// A registry whose daemon is the fake script; `fail_runs` first runs fail.
    fn new(fail_runs: u32) -> Self {
        Self::with_backoff(fail_runs, FAST)
    }

    fn with_backoff(fail_runs: u32, backoff: Backoff) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_tailscaled(&dir.path().join("bin"), fail_runs);
        let tailnets = Tailnets::with_settings(
            TailnetsConfig {
                binary,
                data_dir: dir.path().join("data"),
            },
            backoff,
            Duration::from_secs(2),
        );
        Fixture { dir, tailnets }
    }

    fn bin(&self, file: &str) -> PathBuf {
        self.dir.path().join("bin").join(file)
    }

    fn tailnet_dir(&self, id: i64) -> PathBuf {
        self.dir.path().join("data/tailnets").join(id.to_string())
    }

    fn socket(&self, id: i64) -> PathBuf {
        self.tailnet_dir(id).join("ts.sock")
    }

    fn runs(&self) -> u32 {
        std::fs::read_to_string(self.bin("runs"))
            .ok()
            .and_then(|runs| runs.trim().parse().ok())
            .unwrap_or(0)
    }

    fn pid(&self) -> Option<String> {
        std::fs::read_to_string(self.bin("pid"))
            .ok()
            .map(|pid| pid.trim().to_owned())
            .filter(|pid| !pid.is_empty())
    }

    /// Starts tailnet `id` and puts a fake LocalAPI where its socket is.
    async fn with_api(&self, id: i64, status: serde_json::Value) -> FakeLocalApi {
        self.tailnets.ensure_running(id).await.unwrap();
        FakeLocalApi::bind(&self.socket(id), status)
    }

    /// Starts tailnet `id` and waits until its daemon runs: returns its pid.
    async fn start(&self, id: i64) -> String {
        self.tailnets.ensure_running(id).await.unwrap();
        assert!(eventually(PATIENCE, || self.pid().is_some()).await);
        self.pid().unwrap()
    }

    /// The state of a node that joined tailnet `id`, as a daemon leaves it.
    fn node_state(&self, id: i64) {
        std::fs::create_dir_all(self.tailnet_dir(id)).unwrap();
        std::fs::write(self.tailnet_dir(id).join("tailscaled.state"), "{}").unwrap();
    }

    /// A fake LocalAPI that appears once the (fake) daemon of `id` is up: it
    /// is the daemon that records its start, not the clock, that says so.
    fn api_once_started(&self, id: i64, status: serde_json::Value) -> JoinHandle<FakeLocalApi> {
        let socket = self.socket(id);
        let started = self.bin("runs");
        tokio::spawn(async move {
            assert!(eventually(PATIENCE, || has_run(&started)).await);
            FakeLocalApi::bind(&socket, status)
        })
    }
}

/// What a tailnet that cannot start says, once it does.
async fn failed_reason(tailnets: &Tailnets, id: i64) -> String {
    let mut state = TailnetState::Starting;
    for _ in 0..200 {
        state = tailnets.state(id).await;
        if matches!(state, TailnetState::Failed(_)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let TailnetState::Failed(reason) = state else {
        panic!("expected Failed, got {state:?}");
    };
    reason
}

fn alive(pid: &str) -> bool {
    // Signal 0 only checks that the process exists (`kill` is not in every image).
    let pid = pid
        .parse::<i32>()
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .expect("a pid");
    rustix::process::test_kill_process(pid).is_ok()
}

/// Whether the fake daemon has run, by the count it writes in `runs`.
fn has_run(runs: &std::path::Path) -> bool {
    std::fs::read_to_string(runs).is_ok_and(|runs| runs.trim().parse::<u32>().is_ok_and(|n| n > 0))
}

fn options(hostname: &str, key: Option<&str>) -> ConnectOptions {
    ConnectOptions {
        hostname: hostname.to_owned(),
        control_url: None,
        auth_key: key.map(str::to_owned),
    }
}

mod supervisor {
    use super::*;

    #[tokio::test]
    async fn it_starts_tailscaled_in_userspace_mode_with_a_private_directory_and_a_bare_environment()
     {
        let fixture = Fixture::new(0);
        fixture.start(7).await;

        let directory = fixture.tailnet_dir(7);
        let args = std::fs::read_to_string(fixture.bin("args.1")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "--tun=userspace-networking".to_owned(),
                format!("--statedir={}", directory.display()),
                format!("--socket={}", directory.join("ts.sock").display()),
                "--port=0".to_owned(),
                "--no-logs-no-support".to_owned(),
            ]
        );

        for path in [&directory, directory.parent().unwrap()] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{}", path.display());
        }

        let env = std::fs::read_to_string(fixture.bin("env.1")).unwrap();
        let names: Vec<&str> = env
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .collect();
        assert!(
            names.contains(&"PATH") && names.contains(&"HOME"),
            "{names:?}"
        );
        // The test runs under cargo, which exports `CARGO_*` to it: none of
        // that may reach the daemon.
        let leaked: Vec<&&str> = names
            .iter()
            .filter(|name| !matches!(**name, "PATH" | "HOME" | "PWD" | "SHLVL" | "_" | "OLDPWD"))
            .collect();
        assert!(leaked.is_empty(), "{leaked:?}");
        assert!(env.contains(&format!("HOME={}", directory.display())));
    }

    #[tokio::test]
    async fn ensure_running_is_idempotent() {
        let fixture = Fixture::new(0);
        for _ in 0..3 {
            fixture.start(1).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(fixture.runs(), 1);
    }

    #[tokio::test]
    async fn it_starts_tailscaled_again_with_a_growing_wait_when_it_exits() {
        let fixture = Fixture::new(2);
        // Two runs end at once with status 3; the third one stays.
        fixture.start(1).await;
        assert_eq!(fixture.runs(), 3);
        assert_eq!(fixture.tailnets.state(1).await, TailnetState::Starting);
    }

    #[tokio::test]
    async fn the_state_is_failed_while_it_waits_to_restart() {
        let fixture = Fixture::with_backoff(
            u32::MAX,
            Backoff {
                initial: Duration::from_secs(60),
                max: Duration::from_secs(60),
                healthy_after: Duration::from_secs(60),
            },
        );
        fixture.tailnets.ensure_running(1).await.unwrap();
        let reason = failed_reason(&fixture.tailnets, 1).await;
        assert!(reason.contains("exited"), "{reason}");
        assert!(reason.contains('3'), "{reason}");
    }

    #[tokio::test]
    async fn a_missing_binary_is_a_failed_state_and_is_retried() {
        let dir = tempfile::tempdir().unwrap();
        let tailnets = Tailnets::with_settings(
            TailnetsConfig {
                binary: dir.path().join("no-such-tailscaled"),
                data_dir: dir.path().join("data"),
            },
            FAST,
            SECOND,
        );
        tailnets.ensure_running(1).await.unwrap();
        let reason = failed_reason(&tailnets, 1).await;
        assert!(reason.contains("could not start"), "{reason}");
        assert!(reason.contains("no-such-tailscaled"), "{reason}");
    }

    #[tokio::test]
    async fn stop_kills_the_process_and_keeps_the_state() {
        let fixture = Fixture::new(0);
        let pid = fixture.start(1).await;
        fixture.node_state(1);
        assert!(alive(&pid));

        fixture.tailnets.stop(1).await;

        assert!(
            eventually(PATIENCE, || !alive(&pid)).await,
            "pid {pid} is still running"
        );
        assert!(fixture.tailnets.handle(1).is_none());
        assert_eq!(fixture.tailnets.state(1).await, TailnetState::Stopped);
        assert!(fixture.tailnet_dir(1).join("tailscaled.state").exists());
        // It does not come back by itself.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(fixture.runs(), 1);
    }

    #[tokio::test]
    async fn dropping_the_last_registry_kills_the_process() {
        let fixture = Fixture::new(0);
        let clone = fixture.tailnets.clone();
        let pid = fixture.start(1).await;

        // A clone keeps the daemon.
        drop(clone);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(alive(&pid));

        let Fixture { dir, tailnets } = fixture;
        drop(tailnets);
        assert!(
            eventually(PATIENCE, || !alive(&pid)).await,
            "pid {pid} is still running"
        );
        drop(dir);
    }

    /// `target`, written relative to the working directory of the test.
    fn relative_to_the_working_directory(target: &Path) -> PathBuf {
        let depth = std::env::current_dir().unwrap().components().count() - 1;
        let mut relative: PathBuf = std::iter::repeat_n("..", depth).collect();
        relative.push(target.strip_prefix("/").unwrap());
        relative
    }

    #[test]
    fn a_path_is_made_absolute_and_its_dots_are_folded() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(crate::tailnets::absolute(Path::new("tmp")), cwd.join("tmp"));
        assert_eq!(
            crate::tailnets::absolute(Path::new("./a/../b/./c")),
            cwd.join("b/c")
        );
        assert_eq!(
            crate::tailnets::absolute(Path::new("/var/../data/./tmp")),
            Path::new("/data/tmp")
        );
    }

    #[tokio::test]
    async fn a_relative_data_directory_is_made_absolute_for_the_daemon() {
        // `DATA_DIR=tmp` is the default: the daemon runs in its own directory,
        // so a relative `--socket` or `--statedir` would not be where the
        // server looks for them.
        let dir = tempfile::tempdir().unwrap();
        let data = relative_to_the_working_directory(&dir.path().join("data"));
        assert!(data.is_relative());
        let binary = fake_tailscaled(&dir.path().join("bin"), 0);
        let tailnets = Tailnets::with_settings(
            TailnetsConfig {
                binary,
                data_dir: data,
            },
            FAST,
            Duration::from_secs(2),
        );

        tailnets.ensure_running(3).await.unwrap();
        assert!(eventually(PATIENCE, || has_run(&dir.path().join("bin/runs"))).await);
        let args = std::fs::read_to_string(dir.path().join("bin/args.1")).unwrap();
        let arguments: Vec<&str> = args.lines().collect();
        let statedir = arguments
            .iter()
            .find_map(|argument| argument.strip_prefix("--statedir="))
            .unwrap();
        let socket = arguments
            .iter()
            .find_map(|argument| argument.strip_prefix("--socket="))
            .unwrap();
        assert!(Path::new(statedir).is_absolute(), "{statedir}");
        assert!(Path::new(socket).is_absolute(), "{socket}");
        assert!(socket.ends_with("data/tailnets/3/ts.sock"), "{socket}");
        // The server finds the socket where the daemon is told to put it.
        let _api = FakeLocalApi::bind(Path::new(socket), running_status());
        assert!(matches!(
            tailnets.state(3).await,
            TailnetState::Running { .. }
        ));
    }

    #[tokio::test]
    async fn a_socket_path_too_long_for_a_unix_socket_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let tailnets = Tailnets::new(TailnetsConfig {
            binary: PathBuf::from("/bin/true"),
            data_dir: dir.path().join("a".repeat(100)),
        });
        let error = tailnets.ensure_running(1).await.unwrap_err();
        assert!(matches!(error, Error::SocketPathTooLong(_)), "{error}");
    }
}

mod state {
    use super::*;

    async fn state_of(status: serde_json::Value) -> TailnetState {
        let fixture = Fixture::new(0);
        let _api = fixture.with_api(1, status).await;
        fixture.tailnets.state(1).await
    }

    #[tokio::test]
    async fn is_read_from_the_status_of_the_daemon() {
        let login = "https://login.tailscale.com/a/0123456789abcd";
        let cases = [
            // Control gives its URL, or has none yet.
            (
                json!({"BackendState": "NeedsLogin", "AuthURL": login}),
                TailnetState::NeedsLogin {
                    auth_url: Some(Url::parse(login).unwrap()),
                },
            ),
            (
                json!({"BackendState": "NeedsLogin", "AuthURL": ""}),
                TailnetState::NeedsLogin { auth_url: None },
            ),
            // Running names the node and the tailnet.
            (
                running_status(),
                TailnetState::Running {
                    magic_dns_suffix: "tail1234.ts.net".to_owned(),
                    dns_name: "mymcps.tail1234.ts.net".to_owned(),
                    addresses: vec![
                        "100.64.0.1".parse().unwrap(),
                        "fd7a:115c:a1e0::1".parse().unwrap(),
                    ],
                },
            ),
            (
                status_in("NeedsMachineAuth"),
                TailnetState::NeedsMachineAuth,
            ),
            (status_in("Stopped"), TailnetState::Stopped),
            (status_in("Starting"), TailnetState::Starting),
            (status_in("NoState"), TailnetState::Starting),
        ];
        for (status, expected) in cases {
            assert_eq!(state_of(status.clone()).await, expected, "{status}");
        }
        assert!(matches!(
            state_of(status_in("Surprise")).await,
            TailnetState::Failed(reason) if reason.contains("Surprise")
        ));
    }

    #[tokio::test]
    async fn is_starting_stopped_or_failed_when_the_daemon_cannot_say() {
        let fixture = Fixture::new(0);
        // A daemon that does not answer yet is starting.
        fixture.tailnets.ensure_running(1).await.unwrap();
        assert_eq!(fixture.tailnets.state(1).await, TailnetState::Starting);
        // A tailnet nobody started is stopped.
        assert_eq!(fixture.tailnets.state(99).await, TailnetState::Stopped);
        // A refusal of the daemon is a failure.
        let api = fixture.with_api(2, status_in("Running")).await;
        api.force("/localapi/v0/status", 403, "status access denied");
        let TailnetState::Failed(reason) = fixture.tailnets.state(2).await else {
            panic!("expected Failed");
        };
        assert!(reason.contains("403"), "{reason}");
    }
}

mod connect {
    use super::*;

    #[tokio::test]
    async fn with_a_key_it_starts_the_node_authorised_and_starts_its_login() {
        let fixture = Fixture::new(0);
        let api = fixture.with_api(1, status_in("NeedsLogin")).await;
        let mut options = options("mymcps", Some(KEY));
        options.control_url = Some(Url::parse("https://headscale.example.com").unwrap());

        fixture.tailnets.connect(1, options).await.unwrap();

        let starts = api.requests_to("/localapi/v0/start");
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].method, "POST");
        assert_eq!(starts[0].header("content-type"), Some("application/json"));
        assert_eq!(starts[0].header("host"), Some("local-tailscaled.sock"));
        assert_eq!(
            starts[0].json(),
            json!({
                "UpdatePrefs": {
                    "ControlURL": "https://headscale.example.com/",
                    "RouteAll": true,
                    "WantRunning": true,
                    "Hostname": "mymcps",
                    "CorpDNS": true,
                    "ShieldsUp": true,
                    "AutoUpdate": {"Check": false}
                },
                "AuthKey": KEY
            })
        );
        // tailscaled does not contact the control server on `start` alone: the
        // login is what uses the key (seen against a real Headscale, see
        // tests/headscale-e2e.sh). It carries no key of its own.
        let logins = api.requests_to("/localapi/v0/login-interactive");
        assert_eq!(logins.len(), 1);
        assert!(logins[0].body.is_empty());
    }

    #[tokio::test]
    async fn a_node_that_is_already_logged_in_is_not_logged_in_again_with_or_without_a_key() {
        for key in [Some(SHORT_KEY), None] {
            let fixture = Fixture::new(0);
            let api = fixture.with_api(1, running_status()).await;

            fixture
                .tailnets
                .connect(1, options("mymcps", key))
                .await
                .unwrap();

            assert_eq!(api.requests_to("/localapi/v0/start").len(), 1);
            assert!(api.requests_to("/localapi/v0/login-interactive").is_empty());
        }
    }

    #[tokio::test]
    async fn without_a_key_it_starts_the_node_and_asks_for_a_login() {
        let fixture = Fixture::new(0);
        let api = fixture.with_api(1, status_in("NeedsLogin")).await;

        fixture
            .tailnets
            .connect(1, options("mymcps", None))
            .await
            .unwrap();

        let starts = api.requests_to("/localapi/v0/start");
        assert_eq!(starts.len(), 1);
        let body = starts[0].json();
        assert!(body.get("AuthKey").is_none(), "{body}");
        assert_eq!(body["UpdatePrefs"]["ControlURL"], "");
        assert_eq!(body["UpdatePrefs"]["WantRunning"], true);
        assert_eq!(body["UpdatePrefs"]["RouteAll"], true);
        assert_eq!(body["UpdatePrefs"]["Hostname"], "mymcps");

        let logins = api.requests_to("/localapi/v0/login-interactive");
        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].method, "POST");
        assert!(logins[0].body.is_empty());
        // Start comes first: the login belongs to the new preferences.
        let paths: Vec<String> = api
            .requests()
            .into_iter()
            .map(|request| request.path)
            .collect();
        let paths: Vec<&str> = paths
            .iter()
            .map(String::as_str)
            .filter(|path| !path.ends_with("/status"))
            .collect();
        assert_eq!(
            paths,
            ["/localapi/v0/start", "/localapi/v0/login-interactive"]
        );
    }

    #[tokio::test]
    async fn sign_in_again_asks_for_a_login_whatever_the_node_holds() {
        for state in [
            status_in("NeedsLogin"),
            status_in("Stopped"),
            running_status(),
        ] {
            let fixture = Fixture::new(0);
            let api = fixture.with_api(1, state).await;

            // A key is not a way back: it is not sent, nor kept.
            fixture
                .tailnets
                .sign_in_again(1, options("mymcps", Some(SHORT_KEY)))
                .await
                .unwrap();

            let starts = api.requests_to("/localapi/v0/start");
            assert_eq!(starts.len(), 1);
            let body = starts[0].json();
            assert!(body.get("AuthKey").is_none(), "{body}");
            assert_eq!(body["UpdatePrefs"]["WantRunning"], true);
            assert_eq!(body["UpdatePrefs"]["Hostname"], "mymcps");
            assert_eq!(api.requests_to("/localapi/v0/login-interactive").len(), 1);
            assert!(api.requests_to("/localapi/v0/logout").is_empty());
        }
    }

    #[tokio::test]
    async fn connect_and_sign_in_again_need_a_running_daemon() {
        let fixture = Fixture::new(0);
        let error = fixture
            .tailnets
            .connect(1, options("mymcps", None))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::NotRunning(1)), "{error}");
        let error = fixture
            .tailnets
            .sign_in_again(1, options("mymcps", None))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::NotRunning(1)), "{error}");
    }

    #[tokio::test]
    async fn it_waits_for_the_socket_of_a_daemon_that_is_still_starting() {
        let fixture = Fixture::new(0);
        fixture.tailnets.ensure_running(1).await.unwrap();
        // The socket appears once the daemon has started, not after a delay.
        let late = fixture.api_once_started(1, status_in("NeedsLogin"));

        fixture
            .tailnets
            .connect(1, options("mymcps", Some(SHORT_KEY)))
            .await
            .unwrap();

        let api = late.await.unwrap();
        assert_eq!(api.requests_to("/localapi/v0/start").len(), 1);
    }

    #[tokio::test]
    async fn options_are_validated_before_anything_is_sent() {
        let fixture = Fixture::new(0);
        let api = fixture.with_api(1, status_in("NeedsLogin")).await;
        for hostname in [
            "",
            "   ",
            "-bad",
            "bad-",
            "has space",
            "dots.are.not.labels",
            &"a".repeat(64),
        ] {
            let error = fixture
                .tailnets
                .connect(1, options(hostname, None))
                .await
                .unwrap_err();
            assert!(
                matches!(error, Error::InvalidOptions(_)),
                "{hostname:?}: {error}"
            );
        }
        let mut bad_url = options("mymcps", None);
        bad_url.control_url = Some(Url::parse("ftp://headscale.example.com").unwrap());
        assert!(matches!(
            fixture.tailnets.connect(1, bad_url).await.unwrap_err(),
            Error::InvalidOptions(_)
        ));
        // Signing in again checks them too.
        let error = fixture
            .tailnets
            .sign_in_again(1, options("not a hostname", None))
            .await
            .unwrap_err();
        assert!(matches!(error, Error::InvalidOptions(_)), "{error}");
        assert!(api.requests().is_empty());
    }

    #[tokio::test]
    async fn a_key_is_trimmed_and_an_empty_one_means_none() {
        let fixture = Fixture::new(0);
        let api = fixture.with_api(1, status_in("NeedsLogin")).await;

        fixture
            .tailnets
            .connect(1, options("mymcps", Some(concat!("  tskey", "-auth-x \n"))))
            .await
            .unwrap();
        assert_eq!(
            api.requests_to("/localapi/v0/start")[0].json()["AuthKey"],
            SHORT_KEY
        );

        fixture
            .tailnets
            .connect(1, options("mymcps", Some("")))
            .await
            .unwrap();
        assert!(
            api.requests_to("/localapi/v0/start")[1]
                .json()
                .get("AuthKey")
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_key_is_not_in_the_debug_output_nor_in_an_error() {
        let secret = SECRET;
        let debug = format!("{:?}", options("mymcps", Some(secret)));
        assert!(!debug.contains(secret), "{debug}");
        assert!(debug.contains("redacted"));

        let fixture = Fixture::new(0);
        let api = fixture.with_api(1, status_in("NeedsLogin")).await;
        // A daemon that repeats the request in its error.
        api.force("/localapi/v0/start", 500, &format!("invalid key {secret}"));
        let error = fixture
            .tailnets
            .connect(1, options("mymcps", Some(secret)))
            .await
            .unwrap_err();
        assert!(!format!("{error} {error:?}").contains(secret), "{error}");
        assert!(matches!(error, Error::LocalApi { status: 500, .. }));
    }
}

mod remove {
    use super::*;

    #[test]
    fn the_state_directory_is_the_one_the_registry_uses_and_is_absolute() {
        let fixture = Fixture::new(0);
        let data = fixture.dir.path().join("data");
        assert_eq!(crate::state_dir(&data, 1), fixture.tailnet_dir(1));

        // A relative data directory is made absolute, with its dots folded,
        // as the registry does it.
        let here = std::env::current_dir().unwrap();
        let relative = crate::state_dir(std::path::Path::new("tmp/../data/."), 7);
        assert!(relative.is_absolute(), "{}", relative.display());
        assert_eq!(relative, here.join("data").join("tailnets").join("7"));
    }

    #[tokio::test]
    async fn the_state_is_deleted_without_a_registry_or_a_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state_dir(dir.path(), 4);
        std::fs::create_dir_all(state.join("nested")).unwrap();
        std::fs::write(state.join("tailscaled.state"), "{}").unwrap();
        let other = crate::state_dir(dir.path(), 5);
        std::fs::create_dir_all(&other).unwrap();

        assert!(crate::delete_state(dir.path(), 4).await.unwrap());
        assert!(!state.exists());
        // Another tailnet's state is not touched, and nothing is left to
        // delete the second time.
        assert!(other.exists());
        assert!(!crate::delete_state(dir.path(), 4).await.unwrap());
        assert!(
            !crate::delete_state(&dir.path().join("missing"), 4)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn it_logs_out_stops_the_daemon_and_deletes_the_directory() {
        let fixture = Fixture::new(0);
        let api = fixture.with_api(1, running_status()).await;
        let pid = fixture.start(1).await;
        fixture.node_state(1);

        fixture.tailnets.remove(1).await.unwrap();

        let logouts = api.requests_to("/localapi/v0/logout");
        assert_eq!(logouts.len(), 1);
        assert_eq!(logouts[0].method, "POST");
        assert!(logouts[0].body.is_empty());
        assert!(eventually(PATIENCE, || !alive(&pid)).await);
        assert!(!fixture.tailnet_dir(1).exists());
        assert!(fixture.tailnets.handle(1).is_none());
        assert_eq!(fixture.tailnets.state(1).await, TailnetState::Stopped);

        // A failed logout does not keep the tailnet.
        let api = fixture.with_api(2, running_status()).await;
        api.force("/localapi/v0/logout", 500, "control is down");
        fixture.tailnets.remove(2).await.unwrap();
        assert!(!fixture.tailnet_dir(2).exists());
    }

    #[tokio::test]
    async fn a_tailnet_nobody_started_is_just_deleted() {
        let fixture = Fixture::new(0);
        fixture.tailnets.remove(5).await.unwrap();
        // A directory without the state of a node: nothing to log out.
        let leftovers = fixture.tailnet_dir(6);
        std::fs::create_dir_all(&leftovers).unwrap();
        std::fs::write(leftovers.join("junk"), "x").unwrap();
        fixture.tailnets.remove(6).await.unwrap();
        assert!(!leftovers.exists());
        assert_eq!(fixture.runs(), 0);
    }

    #[tokio::test]
    async fn a_node_joined_before_a_restart_is_logged_out_by_a_daemon_started_for_it() {
        let fixture = Fixture::new(0);
        let directory = fixture.tailnet_dir(1);
        fixture.node_state(1);
        assert!(fixture.tailnets.handle(1).is_none());
        let late = fixture.api_once_started(1, running_status());

        fixture.tailnets.remove(1).await.unwrap();

        let api = late.await.unwrap();
        assert_eq!(api.requests_to("/localapi/v0/logout").len(), 1);
        assert!(!directory.exists());
        assert_eq!(fixture.runs(), 1);
    }

    #[tokio::test]
    async fn discard_deletes_the_tailnet_without_a_logout_and_forever() {
        let fixture = Fixture::new(0);
        // A node that joined before a restart: no daemon is started for it.
        fixture.node_state(1);

        fixture.tailnets.discard(1).await.unwrap();

        assert_eq!(fixture.runs(), 0);
        assert!(!fixture.tailnet_dir(1).exists());
        assert!(fixture.tailnets.handle(1).is_none());
        // Removed for good: nothing starts it again.
        assert!(matches!(
            fixture.tailnets.ensure_running(1).await,
            Err(Error::Removed(1))
        ));
        assert_eq!(fixture.runs(), 0);

        // A running daemon is stopped, and not logged out.
        let api = fixture.with_api(2, running_status()).await;
        let pid = fixture.start(2).await;
        fixture.node_state(2);

        fixture.tailnets.discard(2).await.unwrap();

        assert!(api.requests_to("/localapi/v0/logout").is_empty());
        assert!(eventually(PATIENCE, || !alive(&pid)).await);
        assert!(!fixture.tailnet_dir(2).exists());
        assert!(fixture.tailnets.handle(2).is_none());
    }
}

mod lifecycle {
    use super::*;

    /// A fixture whose daemon takes a second to stop once asked to, as a real
    /// one does while it saves its state.
    fn slow_to_stop() -> Fixture {
        let mut fixture = Fixture::new(0);
        let script = std::fs::read_to_string(fixture.bin("tailscaled")).unwrap();
        let script = script.replace(
            "exec sleep 600",
            "trap 'kill $child; sleep 1; exit 0' TERM\nsleep 600 &\nchild=$!\nwait $child",
        );
        let binary = fixture.bin("slow-tailscaled");
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        crate::test_support::wait_until_executable(&binary);
        fixture.tailnets = Tailnets::with_settings(
            TailnetsConfig {
                binary,
                data_dir: fixture.dir.path().join("data"),
            },
            FAST,
            Duration::from_secs(2),
        );
        fixture
    }

    #[tokio::test]
    async fn a_request_that_comes_during_a_removal_does_not_start_the_tailnet_again() {
        let fixture = slow_to_stop();
        let _api = fixture.with_api(1, running_status()).await;
        let pid = fixture.start(1).await;

        let removal = tokio::spawn({
            let tailnets = fixture.tailnets.clone();
            async move { tailnets.remove(1).await }
        });
        // The list of a page, asking again and again while the daemon is
        // being stopped: every answer is a refusal, none a new daemon.
        let mut refused = 0;
        while !removal.is_finished() {
            match fixture.tailnets.ensure_running(1).await {
                Err(Error::Removed(1)) => refused += 1,
                Ok(()) => assert!(!alive_runs_again(&fixture), "a second daemon started"),
                Err(other) => panic!("{other}"),
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        removal.await.unwrap().unwrap();

        assert!(refused > 0, "no request came in while the removal ran");
        assert!(eventually(PATIENCE, || !alive(&pid)).await);
        assert_eq!(fixture.runs(), 1, "the daemon was started again");
        assert!(fixture.tailnets.handle(1).is_none());
        assert!(!fixture.tailnet_dir(1).exists());
        // And later requests find it removed too.
        assert!(matches!(
            fixture.tailnets.ensure_running(1).await,
            Err(Error::Removed(1))
        ));
        assert!(!fixture.tailnet_dir(1).exists());
        assert_eq!(fixture.tailnets.state(1).await, TailnetState::Stopped);
    }

    fn alive_runs_again(fixture: &Fixture) -> bool {
        fixture.runs() > 1
    }

    #[tokio::test]
    async fn the_id_of_a_removed_tailnet_can_be_started_once_it_is_forgotten() {
        let fixture = Fixture::new(0);
        let _api = fixture.with_api(1, running_status()).await;
        assert!(eventually(PATIENCE, || fixture.runs() == 1).await);
        fixture.tailnets.remove(1).await.unwrap();
        assert!(matches!(
            fixture.tailnets.ensure_running(1).await,
            Err(Error::Removed(1))
        ));
        // Another id is not affected.
        fixture.tailnets.ensure_running(2).await.unwrap();
        // The fake counts its runs in a file: one run at a time.
        assert!(eventually(PATIENCE, || fixture.runs() == 2).await);

        fixture.tailnets.forget_removal(1);
        fixture.tailnets.ensure_running(1).await.unwrap();
        assert!(fixture.tailnets.handle(1).is_some());
        assert!(
            eventually(PATIENCE, || fixture.runs() >= 3).await,
            "runs {}",
            fixture.runs()
        );
    }

    #[tokio::test]
    async fn a_start_that_comes_during_a_stop_waits_for_it_and_runs_one_daemon() {
        let fixture = slow_to_stop();
        let first = fixture.start(1).await;

        let stop = tokio::spawn({
            let tailnets = fixture.tailnets.clone();
            async move { tailnets.stop(1).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        // A plain stop keeps the tailnet: the request starts it again, but
        // only once the first daemon is gone.
        fixture.tailnets.ensure_running(1).await.unwrap();
        assert!(stop.is_finished(), "the start did not wait for the stop");
        stop.await.unwrap();
        assert!(!alive(&first));
        assert!(eventually(PATIENCE, || fixture.runs() == 2).await);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(fixture.runs(), 2);
        assert!(alive(&fixture.pid().unwrap()));
    }

    #[tokio::test]
    async fn two_removals_at_once_leave_nothing_behind() {
        let fixture = Fixture::new(0);
        let _api = fixture.with_api(1, running_status()).await;
        let (first, second) = tokio::join!(fixture.tailnets.remove(1), fixture.tailnets.remove(1));
        first.unwrap();
        second.unwrap();
        assert!(fixture.tailnets.handle(1).is_none());
        assert!(!fixture.tailnet_dir(1).exists());
    }
}

mod route_and_dial {
    use super::*;

    async fn running(fixture: &Fixture) -> (FakeLocalApi, crate::TailnetHandle) {
        let api = fixture.with_api(1, running_status()).await;
        let handle = fixture.tailnets.handle(1).expect("a supervised tailnet");
        (api, handle)
    }

    /// [`running`], with the dials bridged to an echo server.
    async fn bridged(fixture: &Fixture) -> (FakeLocalApi, crate::TailnetHandle, JoinHandle<()>) {
        let (api, handle) = running(fixture).await;
        let (echo, server) = spawn_echo_server().await;
        api.bridge_dials_to(echo);
        (api, handle, server)
    }

    /// The error of a dial that the client refuses.
    async fn dial_error(handle: &crate::TailnetHandle, address: &str) -> std::io::Error {
        handle.dial(address.parse().unwrap()).await.err().unwrap()
    }

    #[tokio::test]
    async fn route_reads_the_status_of_the_daemon() {
        let fixture = Fixture::new(0);
        let (api, handle) = running(&fixture).await;

        let expected = |address: &str, port: u16| {
            Some(std::net::SocketAddr::new(address.parse().unwrap(), port))
        };
        assert_eq!(
            handle.route("nas", 8080).await.unwrap(),
            expected("100.64.0.2", 8080)
        );
        assert_eq!(
            handle.route("NAS.tail1234.ts.net", 443).await.unwrap(),
            expected("100.64.0.2", 443)
        );
        assert_eq!(
            handle.route("100.64.0.3", 80).await.unwrap(),
            expected("100.64.0.3", 80)
        );
        assert_eq!(
            handle.route("192.168.1.20", 22).await.unwrap(),
            expected("192.168.1.20", 22)
        );
        // Another DNS name is not the tailnet's: the caller may send it elsewhere.
        assert_eq!(handle.route("example.com", 443).await.unwrap(), None);
        assert_eq!(handle.route("nas.example.com", 443).await.unwrap(), None);
        // Everything that is the tailnet's or nobody's fails instead, so that
        // it is never left to another network.
        for host in [
            "192.168.2.20",
            "127.0.0.1",
            "169.254.169.254",
            "8.8.8.8",
            "::1",
            "[fd00::1]",
            "localhost",
            "printer",
            "gone.tail1234.ts.net",
            "mymcps.tail1234.ts.net",
            "someone.else.ts.net",
        ] {
            let error = handle.route(host, 22).await.unwrap_err();
            assert!(crate::is_not_routed(&error), "{host}: {error}");
        }

        let status_requests = api.requests_to("/localapi/v0/status");
        assert_eq!(status_requests.len(), 1, "the status is cached");
        assert_eq!(status_requests[0].method, "GET");
        assert_eq!(
            status_requests[0].header("host"),
            Some("local-tailscaled.sock")
        );
        assert!(status_requests[0].header("origin").is_none());
        assert!(status_requests[0].header("referer").is_none());
    }

    #[tokio::test]
    async fn a_host_of_the_tailnet_that_no_peer_carries_is_an_error_not_none() {
        let fixture = Fixture::new(0);
        let (api, handle) = running(&fixture).await;
        assert!(handle.route("192.168.1.20", 22).await.unwrap().is_some());
        let mut status = running_status();
        status["Peer"]["nodekey:bbbb"]["PrimaryRoutes"] = json!([]);
        status["Peer"]["nodekey:aaaa"]["TailscaleIPs"] = json!(["100.64.0.2/32"]);
        api.set_status(status);
        tokio::time::sleep(Duration::from_millis(2100)).await;

        // The subnet route was withdrawn: the address is still the tailnet's.
        let error = handle.route("192.168.1.20", 22).await.unwrap_err();
        assert!(crate::is_not_routed(&error), "{error}");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("192.168.1.20"), "{error}");
        // A tailnet that cannot be asked is another error.
        let down = fixture.with_api(2, status_in("NeedsLogin")).await;
        let error = fixture
            .tailnets
            .handle(2)
            .unwrap()
            .route("192.168.1.20", 22)
            .await
            .unwrap_err();
        assert!(!crate::is_not_routed(&error), "{error}");
        drop(down);
    }

    #[tokio::test]
    async fn the_status_is_cached_for_two_seconds() {
        let fixture = Fixture::new(0);
        let (api, handle) = running(&fixture).await;

        handle.route("nas", 80).await.unwrap();
        assert_eq!(api.requests_to("/localapi/v0/status").len(), 1);

        // A peer that joined is seen once the cache expires, not before.
        let mut status = running_status();
        status["Peer"]["nodekey:cccc"] = json!({
            "DNSName": "printer.tail1234.ts.net.",
            "TailscaleIPs": ["100.64.0.9"]
        });
        api.set_status(status);
        assert!(crate::is_not_routed(
            &handle.route("printer", 80).await.unwrap_err()
        ));
        assert_eq!(api.requests_to("/localapi/v0/status").len(), 1);

        tokio::time::sleep(Duration::from_millis(2100)).await;
        assert!(handle.route("printer", 80).await.unwrap().is_some());
        assert_eq!(api.requests_to("/localapi/v0/status").len(), 2);
    }

    #[tokio::test]
    async fn route_fails_when_the_tailnet_is_not_up() {
        let fixture = Fixture::new(0);
        let api = fixture.with_api(1, status_in("NeedsLogin")).await;
        let handle = fixture.tailnets.handle(1).unwrap();
        let error = handle.route("nas", 80).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);
        assert!(error.to_string().contains("NeedsLogin"), "{error}");

        drop(api);
        let error = handle.route("nas", 80).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);
    }

    #[tokio::test]
    async fn dial_bridges_the_connection_to_the_peer() {
        let fixture = Fixture::new(0);
        let (api, handle, _server) = bridged(&fixture).await;

        let address = handle.route("nas", 7000).await.unwrap().unwrap();
        let mut connection = handle.dial(address).await.unwrap();
        connection
            .write_all(b"hello through the tailnet")
            .await
            .unwrap();
        let mut answer = [0u8; 25];
        connection.read_exact(&mut answer).await.unwrap();
        assert_eq!(&answer, b"hello through the tailnet");
        // And again, and in bulk: the relay is a plain byte stream.
        let payload = vec![0xA5u8; 300_000];
        let writer = {
            let to_send = payload.clone();
            let (mut read, mut write) = tokio::io::split(connection);
            let task = tokio::spawn(async move {
                write.write_all(&to_send).await.unwrap();
                write
            });
            let mut received = vec![0u8; payload.len()];
            read.read_exact(&mut received).await.unwrap();
            assert!(received == payload);
            task.await.unwrap()
        };
        drop(writer);

        let dials = api.requests_to("/localapi/v0/dial");
        assert_eq!(dials.len(), 1);
        assert_eq!(dials[0].method, "POST");
        assert_eq!(dials[0].header("host"), Some("local-tailscaled.sock"));
        assert_eq!(dials[0].header("connection"), Some("upgrade"));
        assert_eq!(dials[0].header("upgrade"), Some("ts-dial"));
        assert_eq!(dials[0].header("dial-host"), Some("100.64.0.2"));
        assert_eq!(dials[0].header("dial-port"), Some("7000"));
        assert_eq!(dials[0].header("dial-network"), Some("tcp"));

        // An IPv6 peer is passed without brackets.
        let address = handle
            .route("fd7a:115c:a1e0::2", 80)
            .await
            .unwrap()
            .unwrap();
        let mut connection = handle.dial(address).await.unwrap();
        connection.write_all(b"v6").await.unwrap();
        let mut answer = [0u8; 2];
        connection.read_exact(&mut answer).await.unwrap();
        assert_eq!(
            api.requests_to("/localapi/v0/dial")[1].header("dial-host"),
            Some("fd7a:115c:a1e0::2")
        );

        // The connection ends when the peer closes.
        let mut connection = handle.dial("100.64.0.2:80".parse().unwrap()).await.unwrap();
        connection.write_all(b"bye").await.unwrap();
        connection.shutdown().await.unwrap();
        let mut rest = Vec::new();
        connection.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"bye");
    }

    #[tokio::test]
    async fn dial_refuses_an_address_a_tailnet_never_routes() {
        let fixture = Fixture::new(0);
        let (api, handle) = running(&fixture).await;
        for address in [
            "127.0.0.1:80",
            "[::1]:80",
            "169.254.169.254:80",
            "0.0.0.0:80",
            "224.0.0.1:80",
            // No peer has it.
            "100.64.0.99:80",
        ] {
            let error = dial_error(&handle, address).await;
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied,
                "{address}"
            );
        }
        assert!(api.requests_to("/localapi/v0/dial").is_empty());
    }

    #[tokio::test]
    async fn an_address_of_this_node_is_neither_routed_nor_dialled_even_inside_a_subnet() {
        let fixture = Fixture::new(0);
        let mut status = running_status();
        status["Peer"]["nodekey:bbbb"]["PrimaryRoutes"] =
            json!(["100.64.0.0/10", "192.168.1.0/24"]);
        let api = fixture.with_api(1, status).await;
        let handle = fixture.tailnets.handle(1).unwrap();

        for own in ["100.64.0.1", "fd7a:115c:a1e0::1"] {
            let error = handle.route(own, 8080).await.unwrap_err();
            assert!(crate::is_not_routed(&error), "{own}: {error}");
        }
        assert!(handle.route("100.64.0.50", 8080).await.unwrap().is_some());
        for address in ["100.64.0.1:8080", "[fd7a:115c:a1e0::1]:8080"] {
            let error = dial_error(&handle, address).await;
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied,
                "{address}"
            );
        }
        assert!(api.requests_to("/localapi/v0/dial").is_empty());
    }

    #[tokio::test]
    async fn dial_reads_a_fresh_status_not_the_cached_one() {
        let fixture = Fixture::new(0);
        let (api, handle, _server) = bridged(&fixture).await;

        // The route fills the cache: the address is a peer's, not ours.
        let address = handle.route("nas", 80).await.unwrap().unwrap();
        assert_eq!(address.ip().to_string(), "100.64.0.2");
        assert_eq!(api.requests_to("/localapi/v0/status").len(), 1);

        // The daemon now gives this very address to this node, well inside the
        // two seconds of the cache.
        let mut status = running_status();
        status["Self"]["TailscaleIPs"] = json!(["100.64.0.1", "fd7a:115c:a1e0::1", "100.64.0.2"]);
        status["TailscaleIPs"] = status["Self"]["TailscaleIPs"].clone();
        api.set_status(status);

        let error = handle.dial(address).await.err().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(api.requests_to("/localapi/v0/dial").is_empty());
        assert_eq!(
            api.requests_to("/localapi/v0/status").len(),
            2,
            "the dial read the status again"
        );
    }

    #[tokio::test]
    async fn dial_reports_what_the_daemon_refused() {
        let fixture = Fixture::new(0);
        let (api, handle) = running(&fixture).await;
        // No bridge: the fake answers `Dial-Self: true`, as for a host that is
        // not on the tailnet.
        let error = dial_error(&handle, "100.64.0.2:80").await;
        assert!(error.to_string().contains("does not route"), "{error}");
        assert_eq!(api.requests_to("/localapi/v0/dial").len(), 1);

        api.force("/localapi/v0/dial", 502, "dial failure: connection refused");
        let error = dial_error(&handle, "100.64.0.2:80").await;
        assert!(error.to_string().contains("502"), "{error}");
        assert!(error.to_string().contains("connection refused"), "{error}");

        api.force("/localapi/v0/dial", 403, "access denied");
        let error = dial_error(&handle, "100.64.0.2:80").await;
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn the_fake_daemon_refuses_a_request_with_another_host() {
        // Guards the fake itself: the client tests above only mean something
        // if a wrong Host would have failed them.
        use http_body_util::Empty;
        use hyper_util::rt::TokioIo;

        let dir = tempfile::tempdir().unwrap();
        let socket: &Path = &dir.path().join("s.sock");
        let _api = FakeLocalApi::bind(socket, running_status());
        let stream = tokio::net::UnixStream::connect(socket).await.unwrap();
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake::<_, Empty<bytes::Bytes>>(TokioIo::new(stream))
                .await
                .unwrap();
        tokio::spawn(connection);
        let request = http::Request::builder()
            .uri("/localapi/v0/status")
            .header("host", "127.0.0.1:41112")
            .body(Empty::new())
            .unwrap();
        assert_eq!(sender.send_request(request).await.unwrap().status(), 403);
    }
}
