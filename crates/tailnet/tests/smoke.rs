//! The real `tailscaled`, not a fake one. It needs a Linux machine, the binary,
//! and the Internet (the coordination server hands out the login URL), so it is
//! ignored: `crates/tailnet/tests/smoke.sh` runs it in a container, with the
//! `tailscaled` of the official `tailscale/tailscale` image.
//!
//! `TAILSCALED_BIN` is the path of that binary.
#![cfg(unix)]

use std::time::{Duration, Instant};

use mymcps_tailnet::{ConnectOptions, TailnetState, Tailnets, TailnetsConfig};

#[tokio::test]
#[ignore = "starts the real tailscaled and reaches Tailscale: run tests/smoke.sh"]
async fn the_real_tailscaled_asks_for_a_login() {
    let binary = std::env::var_os("TAILSCALED_BIN")
        .expect("TAILSCALED_BIN is the path of a tailscaled executable");
    let data = tempfile::Builder::new()
        .prefix("ts")
        .tempdir_in("/tmp")
        .unwrap();
    let tailnets = Tailnets::new(TailnetsConfig {
        binary: binary.into(),
        data_dir: data.path().to_owned(),
    });

    tailnets.ensure_running(1).await.unwrap();
    tailnets
        .connect(
            1,
            ConnectOptions {
                hostname: "mymcps-smoke".to_owned(),
                control_url: None,
                auth_key: None,
            },
        )
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(90);
    let mut state = tailnets.state(1).await;
    loop {
        println!("state: {state:?}");
        if matches!(
            state,
            TailnetState::NeedsLogin { auth_url: Some(_) } | TailnetState::Failed(_)
        ) || Instant::now() >= deadline
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        state = tailnets.state(1).await;
    }
    let TailnetState::NeedsLogin {
        auth_url: Some(url),
    } = state
    else {
        panic!("expected NeedsLogin with an auth URL, got {state:?}");
    };
    assert_eq!(url.scheme(), "https");
    println!("auth url host: {:?}", url.host_str());

    // Nobody logged in: a peer does not exist, and asking is an error, not a
    // route.
    let handle = tailnets.handle(1).unwrap();
    assert!(handle.route("nas", 80).await.is_err());

    tailnets.remove(1).await.unwrap();
    assert!(!data.path().join("tailnets/1").exists());
}
