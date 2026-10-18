//! Runs the real `tailscaled`. It needs a binary and the network, so it is
//! ignored unless asked for:
//!
//! ```text
//! MYMCPS_TAILSCALED=/path/to/tailscaled cargo test -p mymcps-tailscale --test real_tailscaled -- --ignored --nocapture
//! ```
//!
//! Set `MYMCPS_TAILSCALE_AUTH_KEY` to log in with a key; without one the node
//! asks for a login link and the test waits for it.

use std::time::Duration;

use mymcps_tailscale::{Availability, NodeConfig, NodeState, Tailscale, TailscaleOptions};

#[tokio::test]
#[ignore = "needs a tailscaled binary and the network"]
async fn a_real_node_asks_for_a_login() {
    let Some(binary) = std::env::var_os("MYMCPS_TAILSCALED") else {
        eprintln!("MYMCPS_TAILSCALED is not set: nothing to run");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let tailscale = Tailscale::start(TailscaleOptions {
        binary: Some(binary.into()),
        root: dir.path().join("ts"),
        max_nodes: 1,
    })
    .await;
    let Availability::Available { version } = tailscale.availability() else {
        panic!("{:?}", tailscale.availability());
    };
    eprintln!("tailscaled {version}");

    let key = std::env::var("MYMCPS_TAILSCALE_AUTH_KEY").ok();
    tailscale
        .ensure(NodeConfig {
            id: 1,
            hostname: "mymcps-smoke".into(),
            auth_key: key.clone(),
        })
        .await
        .unwrap();

    let mut last = None;
    for _ in 0..90 {
        let status = tailscale.status(1);
        if last.as_ref() != Some(&status) {
            eprintln!("{status:#?}");
            last = Some(status.clone());
        }
        match status.state {
            NodeState::Connected => break,
            NodeState::NeedsLogin { auth_url: Some(_) } if key.is_none() => break,
            NodeState::Failed { .. } => break,
            _ => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
    tailscale.shutdown().await;
}
