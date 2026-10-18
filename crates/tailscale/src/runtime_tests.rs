//! The supervisor and the runtime against a fake `tailscaled`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::testing::{Harness, Launch, config, eventually};
use crate::{Availability, NodeState, Tailscale, TailscaleError, TailscaleOptions};

fn state(harness: &Harness, id: i64) -> NodeState {
    harness.tailscale.status(id).state
}

async fn wait_for_state(harness: &Harness, id: i64, wanted: NodeState) {
    eventually(&format!("{wanted:?}"), || state(harness, id) == wanted).await;
}

#[tokio::test]
async fn a_node_starts_applies_its_prefs_and_connects() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.connect();

    harness.tailscale.ensure(config(1, None)).await.unwrap();
    wait_for_state(&harness, 1, NodeState::Connected).await;

    let status = harness.tailscale.status(1);
    assert_eq!(status.tailnet.as_deref(), Some("me@example.com"));
    assert_eq!(status.dns_name.as_deref(), Some("mymcps.tail1234.ts.net"));
    assert_eq!(status.addresses.len(), 2);
    assert_eq!(fake.count("PATCH", "/localapi/v0/prefs"), 1);
    let patch = fake.last("PATCH", "/localapi/v0/prefs").unwrap().json();
    assert_eq!(patch["Hostname"], "mymcps");
    assert_eq!(patch["ShieldsUp"], true);
    assert_eq!(patch["RouteAll"], true);
    assert_eq!(patch["WantRunning"], true);
    // Connected and logged in: nothing else was asked of the node.
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 0);
    assert_eq!(fake.count("POST", "/localapi/v0/login-interactive"), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn node_directories_are_private() {
    use std::os::unix::fs::PermissionsExt;

    let harness = Harness::new(5).await;
    harness.tailscale.ensure(config(4, None)).await.unwrap();
    for dir in ["", "state", "logs"] {
        let path = harness.root().join("4").join(dir);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", path.display());
    }
}

#[tokio::test]
async fn backend_states_map_to_node_states() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    harness.tailscale.ensure(config(1, None)).await.unwrap();

    for (backend, expected) in [
        ("NoState", NodeState::Starting),
        ("Starting", NodeState::Starting),
        ("NeedsMachineAuth", NodeState::NeedsApproval),
        ("Running", NodeState::Connected),
        ("NeedsLogin", NodeState::NeedsLogin { auth_url: None }),
        ("SomethingNew", NodeState::Starting),
    ] {
        fake.set(|state| state.backend_state = backend.into());
        wait_for_state(&harness, 1, expected).await;
    }
}

#[tokio::test]
async fn a_stopped_node_is_told_to_run_again() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.set(|state| state.backend_state = "Stopped".into());
    harness.tailscale.ensure(config(1, None)).await.unwrap();

    wait_for_state(&harness, 1, NodeState::Starting).await;
    eventually("WantRunning applied again", || {
        fake.count("PATCH", "/localapi/v0/prefs") >= 3
    })
    .await;
}

#[tokio::test]
async fn the_key_is_used_once_per_process() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.set(|state| {
        state.backend_state = "NeedsLogin".into();
        state.on_start = Some(Box::new(|state, options| {
            assert_eq!(options["AuthKey"], "tskey-auth-one");
            state.backend_state = "Running".into();
            state.have_node_key = true;
        }));
    });

    harness
        .tailscale
        .ensure(config(1, Some("tskey-auth-one")))
        .await
        .unwrap();
    wait_for_state(&harness, 1, NodeState::Connected).await;
    // Logged in without a node key before: the login is asked for, like `tailscale up` does.
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 1);
    assert_eq!(fake.count("POST", "/localapi/v0/login-interactive"), 1);

    // Idempotent: the same call changes nothing.
    harness
        .tailscale
        .ensure(config(1, Some("tskey-auth-one")))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 1);
    assert_eq!(harness.launcher.launches(), 1);
}

#[tokio::test]
async fn a_key_that_changes_is_tried_and_one_that_stays_is_not_repeated() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.set(|state| {
        state.backend_state = "NeedsLogin".into();
        // Control refuses every key but the second one.
        state.on_start = Some(Box::new(|state, options| {
            if options["AuthKey"] == "tskey-auth-good" {
                state.backend_state = "Running".into();
                state.health.clear();
            } else {
                state.health = vec![
                    "You are logged out. The last login error was: invalid key: unable to validate API key"
                        .into(),
                ];
            }
        }));
    });

    harness
        .tailscale
        .ensure(config(1, Some("tskey-auth-bad")))
        .await
        .unwrap();
    wait_for_state(
        &harness,
        1,
        NodeState::Failed {
            error: "Tailscale refused the auth key: invalid key: unable to validate API key".into(),
        },
    )
    .await;
    // The refused key is not sent again and again.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 1);
    assert_eq!(
        harness.tailscale.proxy_url(1).unwrap_err(),
        "Tailscale refused the auth key: invalid key: unable to validate API key"
    );

    // A new key is tried at once, without restarting the process.
    harness
        .tailscale
        .ensure(config(1, Some("tskey-auth-good")))
        .await
        .unwrap();
    wait_for_state(&harness, 1, NodeState::Connected).await;
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 2);
    assert_eq!(harness.launcher.launches(), 1);
}

#[tokio::test]
async fn a_key_that_gets_no_answer_fails_after_the_grace_period() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.set(|state| state.backend_state = "NeedsLogin".into());
    harness
        .tailscale
        .ensure(config(1, Some("tskey-auth-x")))
        .await
        .unwrap();
    eventually("login to fail", || {
        matches!(state(&harness, 1), NodeState::Failed { .. })
    })
    .await;
    let NodeState::Failed { error } = state(&harness, 1) else {
        unreachable!()
    };
    assert!(
        error.starts_with("Tailscale did not log in with the auth key"),
        "{error}"
    );
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 1);
}

#[tokio::test]
async fn a_control_server_that_cannot_be_reached_is_not_blamed_on_the_key() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    let unreachable = "register request: Post \"https://controlplane.tailscale.com/machine/register\": all connection attempts failed";
    fake.set(|state| {
        state.backend_state = "NeedsLogin".into();
        state.health = vec![format!(
            "You are logged out. The last login error was: {unreachable}"
        )];
    });
    harness
        .tailscale
        .ensure(config(1, Some("tskey-auth-fine")))
        .await
        .unwrap();
    // Within the grace period it is still starting, and never "refused".
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(state(&harness, 1), NodeState::Starting);
    wait_for_state(
        &harness,
        1,
        NodeState::Failed {
            error: format!("Tailscale could not reach its control server to log in: {unreachable}"),
        },
    )
    .await;
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 1);

    // When the network is back, the node connects without another start.
    fake.set(|state| {
        state.backend_state = "Running".into();
        state.health.clear();
    });
    wait_for_state(&harness, 1, NodeState::Connected).await;
}

#[tokio::test]
async fn without_a_key_a_control_server_that_cannot_be_reached_fails_the_node_after_the_grace_period()
 {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    let unreachable = "register request: Post \"https://controlplane.tailscale.com/machine/register\": all connection attempts failed";
    fake.set(|state| {
        state.backend_state = "NeedsLogin".into();
        state.health = vec![format!(
            "You are logged out. The last login error was: {unreachable}"
        )];
    });
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    // Within the grace period the node still waits for a link.
    wait_for_state(&harness, 1, NodeState::NeedsLogin { auth_url: None }).await;
    wait_for_state(
        &harness,
        1,
        NodeState::Failed {
            error: format!("Tailscale could not reach its control server to log in: {unreachable}"),
        },
    )
    .await;
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 0);
    assert!(
        harness
            .tailscale
            .proxy_url(1)
            .unwrap_err()
            .starts_with("Tailscale could not reach its control server to log in")
    );

    // The network is back: Tailscale gives a link, and the node shows it.
    fake.set(|state| {
        state.health.clear();
        state.auth_url = "https://login.tailscale.com/a/abc123".into();
    });
    wait_for_state(
        &harness,
        1,
        NodeState::NeedsLogin {
            auth_url: Some("https://login.tailscale.com/a/abc123".into()),
        },
    )
    .await;
    fake.set(|state| {
        state.backend_state = "Running".into();
        state.auth_url.clear();
    });
    wait_for_state(&harness, 1, NodeState::Connected).await;
}

#[tokio::test]
async fn a_start_that_tailscaled_rejects_fails_the_node_until_the_key_changes() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.set(|state| {
        state.backend_state = "NeedsLogin".into();
        state.start_error = Some((
            500,
            "cannot start backend when state store is unhealthy\n".into(),
        ));
    });
    harness
        .tailscale
        .ensure(config(1, Some("tskey-auth-a")))
        .await
        .unwrap();
    wait_for_state(
        &harness,
        1,
        NodeState::Failed {
            error: "Tailscale could not start the login: cannot start backend when state store is unhealthy"
                .into(),
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 1);
}

#[tokio::test]
async fn without_a_key_the_login_link_is_asked_for_once_and_shown() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.set(|state| {
        state.backend_state = "NeedsLogin".into();
        state.on_login_interactive = Some(Box::new(|state, _| {
            state.auth_url = "https://login.tailscale.com/a/abc123".into();
        }));
    });
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    wait_for_state(
        &harness,
        1,
        NodeState::NeedsLogin {
            auth_url: Some("https://login.tailscale.com/a/abc123".into()),
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fake.count("POST", "/localapi/v0/login-interactive"), 1);
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 0);
    assert_eq!(
        harness.tailscale.proxy_url(1).unwrap_err(),
        "it needs a Tailscale login"
    );

    // The admin visits the link: the node connects.
    fake.set(|state| {
        state.backend_state = "Running".into();
        state.auth_url.clear();
    });
    wait_for_state(&harness, 1, NodeState::Connected).await;
}

#[tokio::test]
async fn a_key_added_to_a_node_waiting_for_a_link_is_used() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.set(|state| {
        state.backend_state = "NeedsLogin".into();
        state.on_start = Some(Box::new(|state, _| state.backend_state = "Running".into()));
    });
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    wait_for_state(&harness, 1, NodeState::NeedsLogin { auth_url: None }).await;

    harness
        .tailscale
        .ensure(config(1, Some("tskey-auth-late")))
        .await
        .unwrap();
    wait_for_state(&harness, 1, NodeState::Connected).await;
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 1);
}

#[tokio::test]
async fn a_changed_hostname_is_applied_without_a_restart() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.connect();
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    wait_for_state(&harness, 1, NodeState::Connected).await;

    let mut renamed = config(1, None);
    renamed.hostname = "office".into();
    harness.tailscale.ensure(renamed).await.unwrap();
    eventually("the new hostname", || {
        fake.last("PATCH", "/localapi/v0/prefs")
            .is_some_and(|request| request.json()["Hostname"] == "office")
    })
    .await;
    assert_eq!(harness.launcher.launches(), 1);
}

#[tokio::test]
async fn a_crashed_process_is_restarted() {
    let harness = Harness::new(5).await;
    harness.launcher.node(1).connect();
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    wait_for_state(&harness, 1, NodeState::Connected).await;

    harness.launcher.crash(1);
    eventually("a second launch", || harness.launcher.launches() == 2).await;
    wait_for_state(&harness, 1, NodeState::Connected).await;
}

#[tokio::test]
async fn quick_failures_back_off_and_end_in_failed_with_the_last_line() {
    let harness = Harness::new(5).await;
    let line = Some("flag provided but not defined: -x".to_owned());
    harness
        .launcher
        .script(1, (0..4).map(|_| Launch::ExitAt { line: line.clone() }));
    harness.launcher.node(1).connect();
    harness.tailscale.ensure(config(1, None)).await.unwrap();

    wait_for_state(
        &harness,
        1,
        NodeState::Failed {
            error: "tailscaled keeps stopping: flag provided but not defined: -x".into(),
        },
    )
    .await;
    // It does not give up: the fifth launch runs, and the node recovers.
    wait_for_state(&harness, 1, NodeState::Connected).await;
    assert_eq!(harness.launcher.launches(), 5);

    // The delay doubles: 20, 40, 80, 160 ms in the test timing.
    let times = harness.launcher.launch_times();
    let gaps: Vec<_> = times.windows(2).map(|pair| pair[1] - pair[0]).collect();
    for (gap, minimum) in gaps.iter().zip([20, 40, 80, 160]) {
        assert!(*gap >= Duration::from_millis(minimum), "{gaps:?}");
    }
}

#[tokio::test]
async fn a_binary_that_cannot_be_started_is_reported() {
    let harness = Harness::new(5).await;
    harness
        .launcher
        .script(1, (0..3).map(|_| Launch::SpawnError));
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    eventually("a failure", || {
        matches!(state(&harness, 1), NodeState::Failed { .. })
    })
    .await;
    let NodeState::Failed { error } = state(&harness, 1) else {
        unreachable!()
    };
    assert!(
        error.contains("it could not be started (no such file)"),
        "{error}"
    );
}

#[tokio::test]
async fn reconnect_restarts_the_process_and_tries_the_key_again() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.set(|state| {
        state.backend_state = "NeedsLogin".into();
        state.on_start = Some(Box::new(|state, _| state.backend_state = "Running".into()));
    });
    harness
        .tailscale
        .ensure(config(1, Some("tskey-auth-k")))
        .await
        .unwrap();
    wait_for_state(&harness, 1, NodeState::Connected).await;

    fake.set(|state| state.backend_state = "NeedsLogin".into());
    harness
        .tailscale
        .reconnect(config(1, Some("tskey-auth-k")))
        .await
        .unwrap();
    eventually("a second launch", || harness.launcher.launches() == 2).await;
    assert_eq!(harness.launcher.stops(), 1);
    wait_for_state(&harness, 1, NodeState::Connected).await;
    assert_eq!(fake.count("POST", "/localapi/v0/start"), 2);
}

#[tokio::test]
async fn the_number_of_nodes_is_limited() {
    let harness = Harness::new(1).await;
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    let error = harness.tailscale.ensure(config(2, None)).await.unwrap_err();
    assert!(
        matches!(error, TailscaleError::TooManyNodes { max: 1 }),
        "{error}"
    );
    assert!(!harness.root().join("2").exists());
    // The node that is there can be ensured again.
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    // And a place frees up when it is removed.
    harness.tailscale.remove(1).await.unwrap();
    harness.tailscale.ensure(config(2, None)).await.unwrap();
}

#[tokio::test]
async fn a_socket_path_that_is_too_long_is_refused_with_a_sentence() {
    let root = PathBuf::from("/tmp").join("x".repeat(110));
    let tailscale = Tailscale::build(
        root,
        5,
        Availability::Available {
            version: "t".into(),
        },
        Some(Arc::new(Arc::new(crate::testing::FakeLauncher::default()))),
        crate::testing::fast(),
    )
    .await
    .unwrap();
    let error = tailscale.ensure(config(1, None)).await.unwrap_err();
    assert!(matches!(error, TailscaleError::SocketPathTooLong { .. }));
    let message = error.to_string();
    assert!(
        message.starts_with("The Tailscale socket path is too long ("),
        "{message}"
    );
    assert!(
        message.ends_with("Use a shorter data directory"),
        "{message}"
    );
}

#[tokio::test]
async fn a_node_that_could_not_start_is_failed_with_the_reason_until_it_is_removed() {
    let root = PathBuf::from("/tmp").join("x".repeat(110));
    let tailscale = Tailscale::build(
        root,
        5,
        Availability::Available {
            version: "t".into(),
        },
        Some(Arc::new(Arc::new(crate::testing::FakeLauncher::default()))),
        crate::testing::fast(),
    )
    .await
    .unwrap();
    // Nothing was tried yet: the node is simply not running.
    assert_eq!(tailscale.status(1).state, NodeState::Stopped);
    assert_eq!(tailscale.proxy_url(1).unwrap_err(), "it is not running");

    let sentence = tailscale
        .ensure(config(1, None))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        sentence.ends_with("Use a shorter data directory"),
        "{sentence}"
    );
    assert_eq!(
        tailscale.status(1).state,
        NodeState::Failed {
            error: sentence.clone()
        }
    );
    assert_eq!(tailscale.proxy_url(1).unwrap_err(), sentence);
    // Only that node: another id has nothing to say.
    assert_eq!(tailscale.status(2).state, NodeState::Stopped);

    tailscale.remove(1).await.unwrap();
    assert_eq!(tailscale.status(1).state, NodeState::Stopped);
    assert_eq!(tailscale.proxy_url(1).unwrap_err(), "it is not running");
}

#[tokio::test]
async fn a_node_that_did_not_fit_is_failed_until_a_place_frees_up() {
    let harness = Harness::new(1).await;
    harness.launcher.node(2).connect();
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    let sentence = harness
        .tailscale
        .ensure(config(2, None))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        sentence,
        "This server runs at most 1 Tailscale proxies: raise TAILSCALE_MAX_PROXIES"
    );
    assert_eq!(
        harness.tailscale.status(2).state,
        NodeState::Failed {
            error: sentence.clone()
        }
    );
    assert_eq!(harness.tailscale.proxy_url(2).unwrap_err(), sentence);
    // The node that is running is not touched.
    assert_ne!(
        harness.tailscale.status(1).state,
        NodeState::Failed { error: sentence }
    );

    harness.tailscale.remove(1).await.unwrap();
    // Still failed: nobody asked again yet.
    assert!(matches!(
        harness.tailscale.status(2).state,
        NodeState::Failed { .. }
    ));
    harness.tailscale.ensure(config(2, None)).await.unwrap();
    wait_for_state(&harness, 2, NodeState::Connected).await;
    assert!(harness.tailscale.proxy_url(2).is_ok());
}

#[tokio::test]
async fn reconcile_records_the_nodes_it_could_not_start_and_forgets_the_dropped_ones() {
    let harness = Harness::new(1).await;
    harness
        .tailscale
        .reconcile(vec![config(1, None), config(2, None)])
        .await;
    assert!(matches!(
        harness.tailscale.status(1).state,
        NodeState::Starting | NodeState::Connected | NodeState::NeedsLogin { .. }
    ));
    let NodeState::Failed { error } = harness.tailscale.status(2).state else {
        panic!("node 2 should have failed");
    };
    assert!(error.contains("at most 1 Tailscale proxies"), "{error}");
    assert_eq!(harness.tailscale.proxy_url(2).unwrap_err(), error);

    // Node 2 is no longer listed: its failure goes, and node 1 keeps running.
    harness.tailscale.reconcile(vec![config(1, None)]).await;
    assert_eq!(harness.tailscale.status(2).state, NodeState::Stopped);
    assert!(!matches!(
        harness.tailscale.status(1).state,
        NodeState::Stopped | NodeState::Failed { .. }
    ));
}

#[tokio::test]
async fn remove_logs_out_stops_and_deletes_the_state() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    fake.connect();
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    wait_for_state(&harness, 1, NodeState::Connected).await;
    let first = harness.tailscale.proxy_url(1).unwrap();
    assert!(harness.root().join("1").exists());

    harness.tailscale.remove(1).await.unwrap();
    assert_eq!(fake.count("POST", "/localapi/v0/logout"), 1);
    assert_eq!(harness.launcher.stops(), 1);
    assert!(!harness.root().join("1").exists());
    assert_eq!(harness.tailscale.status(1).state, NodeState::Stopped);
    assert_eq!(
        harness.tailscale.proxy_url(1).unwrap_err(),
        "it is not running"
    );

    // Removing what is not there is fine.
    harness.tailscale.remove(1).await.unwrap();
    harness.tailscale.remove(99).await.unwrap();

    // A node created again is a new one, with a secret of its own.
    fake.connect();
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    wait_for_state(&harness, 1, NodeState::Connected).await;
    assert_ne!(
        harness.tailscale.proxy_url(1).unwrap().password(),
        first.password()
    );
}

#[tokio::test]
async fn reconcile_keeps_the_listed_nodes_and_deletes_the_others() {
    let harness = Harness::new(2).await;
    harness.launcher.node(1).connect();
    harness.launcher.node(2).connect();
    harness.launcher.node(3).connect();

    // A node that runs but is not listed, one that is only on disk, and something that is not a node.
    harness.tailscale.ensure(config(3, None)).await.unwrap();
    wait_for_state(&harness, 3, NodeState::Connected).await;
    std::fs::create_dir_all(harness.root().join("7").join("state")).unwrap();
    std::fs::write(
        harness
            .root()
            .join("7")
            .join("state")
            .join("tailscaled.state"),
        "{}",
    )
    .unwrap();
    std::fs::create_dir_all(harness.root().join("notes")).unwrap();
    std::fs::write(harness.root().join("README"), "keep").unwrap();

    harness
        .tailscale
        .reconcile(vec![config(1, None), config(2, None)])
        .await;

    assert!(!harness.root().join("3").exists());
    assert!(!harness.root().join("7").exists());
    assert!(harness.root().join("notes").exists());
    assert!(harness.root().join("README").exists());
    assert_eq!(harness.tailscale.status(3).state, NodeState::Stopped);
    wait_for_state(&harness, 1, NodeState::Connected).await;
    wait_for_state(&harness, 2, NodeState::Connected).await;
    assert_eq!(
        harness
            .launcher
            .node(3)
            .count("POST", "/localapi/v0/logout"),
        1
    );

    // Again with the same list: nothing restarts.
    let launches = harness.launcher.launches();
    harness
        .tailscale
        .reconcile(vec![config(1, None), config(2, None)])
        .await;
    assert_eq!(harness.launcher.launches(), launches);
}

#[tokio::test]
async fn shutdown_stops_every_process_and_keeps_the_state() {
    let harness = Harness::new(5).await;
    for id in [1, 2] {
        harness.launcher.node(id).connect();
        harness.tailscale.ensure(config(id, None)).await.unwrap();
    }
    wait_for_state(&harness, 1, NodeState::Connected).await;
    wait_for_state(&harness, 2, NodeState::Connected).await;

    harness.tailscale.shutdown().await;
    assert_eq!(harness.launcher.stops(), 2);
    assert_eq!(state(&harness, 1), NodeState::Stopped);
    assert_eq!(state(&harness, 2), NodeState::Stopped);
    assert!(harness.root().join("1").join("state").exists());
    assert!(matches!(
        harness.tailscale.ensure(config(3, None)).await,
        Err(TailscaleError::ShuttingDown)
    ));
}

#[tokio::test]
async fn the_proxy_url_is_given_only_for_a_connected_node() {
    let harness = Harness::new(5).await;
    let fake = harness.launcher.node(1);
    harness.launcher.node(2).connect();
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    harness.tailscale.ensure(config(2, None)).await.unwrap();

    fake.set(|state| state.backend_state = "Starting".into());
    wait_for_state(&harness, 1, NodeState::Starting).await;
    assert_eq!(
        harness.tailscale.proxy_url(1).unwrap_err(),
        "it is starting"
    );
    fake.set(|state| state.backend_state = "NeedsMachineAuth".into());
    wait_for_state(&harness, 1, NodeState::NeedsApproval).await;
    assert_eq!(
        harness.tailscale.proxy_url(1).unwrap_err(),
        "it is waiting for approval in the Tailscale admin console"
    );

    wait_for_state(&harness, 2, NodeState::Connected).await;
    fake.connect();
    wait_for_state(&harness, 1, NodeState::Connected).await;
    let first = harness.tailscale.proxy_url(1).unwrap();
    let second = harness.tailscale.proxy_url(2).unwrap();
    assert_eq!(first.scheme(), "socks5h");
    assert_eq!(first.username(), "1");
    assert_eq!(second.username(), "2");
    assert_eq!(first.host_str(), Some("127.0.0.1"));
    assert_eq!(first.port(), harness.tailscale.socks_port());
    // 32 random bytes in base64url: 43 characters, a different one per node.
    assert_eq!(first.password().unwrap().len(), 43);
    assert_ne!(first.password(), second.password());
    // The password is stable for the life of the node.
    assert_eq!(harness.tailscale.proxy_url(1).unwrap(), first);
}

#[tokio::test]
async fn a_runtime_that_is_off_refuses_and_remembers_nothing() {
    let tailscale = Tailscale::disabled();
    assert_eq!(tailscale.availability(), &Availability::Disabled);
    assert!(matches!(
        tailscale.ensure(config(1, None)).await,
        Err(TailscaleError::Disabled)
    ));
    assert!(matches!(
        tailscale.reconnect(config(1, None)).await,
        Err(TailscaleError::Disabled)
    ));
    assert_eq!(tailscale.status(1).state, NodeState::Stopped);
    assert!(tailscale.proxy_url(1).is_err());
    tailscale.remove(1).await.unwrap();
    tailscale.reconcile(vec![config(1, None)]).await;
    tailscale.shutdown().await;
    assert_eq!(tailscale.status(1).state, NodeState::Stopped);
}

#[tokio::test]
async fn start_reports_why_nodes_cannot_run() {
    let dir = tempfile::tempdir().unwrap();
    let options = |binary: Option<PathBuf>, max_nodes| TailscaleOptions {
        binary,
        root: dir.path().join("tailscale"),
        max_nodes,
    };

    let off = Tailscale::start(options(None, 0)).await;
    assert_eq!(off.availability(), &Availability::Disabled);
    assert!(off.socks_port().is_none());

    let missing = Tailscale::start(options(Some(dir.path().join("no/tailscaled")), 3)).await;
    let Availability::Unavailable { reason } = missing.availability() else {
        panic!("{:?}", missing.availability());
    };
    assert!(
        reason.starts_with("tailscaled was not found at "),
        "{reason}"
    );
    assert!(matches!(
        missing.ensure(config(1, None)).await,
        Err(TailscaleError::Unavailable(_))
    ));
    // Not available is not the same as gone: what is on disk stays.
    std::fs::create_dir_all(dir.path().join("tailscale/5")).unwrap();
    missing.reconcile(Vec::new()).await;
    assert!(dir.path().join("tailscale/5").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn start_reads_the_version_of_the_binary() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("tailscaled");
    std::fs::write(
        &script,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf '1.104.1\\n  tailscale commit: abc\\n'; exit 0; fi\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut tailscale = None;
    // A script that was just written can be reported busy while another test forks.
    for _ in 0..20 {
        let started = Tailscale::start(TailscaleOptions {
            binary: Some(script.clone()),
            root: dir.path().join("data/tailscale"),
            max_nodes: 2,
        })
        .await;
        if matches!(started.availability(), Availability::Available { .. }) {
            tailscale = Some(started);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let tailscale = tailscale.expect("the binary did not run");
    assert_eq!(
        tailscale.availability(),
        &Availability::Available {
            version: "1.104.1".into()
        }
    );
    assert!(tailscale.socks_port().is_some());
    let mode = std::fs::metadata(dir.path().join("data/tailscale"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);

    let failing = dir.path().join("failing");
    std::fs::write(&failing, "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..20 {
        let started = Tailscale::start(TailscaleOptions {
            binary: Some(failing.clone()),
            root: dir.path().join("data/tailscale"),
            max_nodes: 2,
        })
        .await;
        let Availability::Unavailable { reason } = started.availability() else {
            panic!("{:?}", started.availability());
        };
        if reason.starts_with("tailscaled --version failed") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the failing binary was not reported");
}

#[test]
fn a_node_config_never_prints_its_key() {
    let printed = format!("{:?}", config(1, Some("tskey-auth-secret")));
    assert!(!printed.contains("tskey-auth-secret"), "{printed}");
    assert!(printed.contains("[redacted]"));
    let _: Option<Value> = None;
}
