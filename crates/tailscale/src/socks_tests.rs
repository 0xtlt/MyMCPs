//! The SOCKS front end to end: a real HTTP client on one side, a fake
//! `tailscaled` and a local web server on the other.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::routing::get;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::NodeState;
use crate::testing::{DialPlan, Harness, config, eventually};

/// A web server standing in for the machine in the tailnet.
async fn web_server() -> (SocketAddr, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = Router::new().route(
        "/",
        get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                "hello from the tailnet"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, hits)
}

/// A connected node 1 whose dials go to `plan`.
async fn connected(harness: &Harness, plan: DialPlan) {
    let fake = harness.launcher.node(1);
    fake.connect();
    fake.set(|state| state.dial = plan);
    harness.tailscale.ensure(config(1, None)).await.unwrap();
    eventually("the node to connect", || {
        harness.tailscale.status(1).state == NodeState::Connected
    })
    .await;
}

fn client_through(proxy: &url::Url) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::all(proxy.as_str()).unwrap())
        .build()
        .unwrap()
}

#[tokio::test]
async fn an_http_client_reaches_a_tailnet_host_through_the_front() {
    let (web, hits) = web_server().await;
    let harness = Harness::new(5).await;
    connected(&harness, DialPlan::Tunnel(web)).await;
    let client = client_through(&harness.tailscale.proxy_url(1).unwrap());

    // A name goes to tailscaled as it is: it resolves tailnet names itself (socks5h).
    let body = client
        .get(format!("http://box.tail1234.ts.net:{}/", web.port()))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "hello from the tailnet");
    let dial = harness
        .launcher
        .node(1)
        .last("POST", "/localapi/v0/dial")
        .unwrap();
    assert_eq!(dial.header("dial-host"), Some("box.tail1234.ts.net"));
    assert_eq!(
        dial.header("dial-port"),
        Some(web.port().to_string().as_str())
    );
    assert_eq!(dial.header("dial-network"), Some("tcp"));

    // An IPv4 and an IPv6 address are sent as text, without brackets.
    for (url_host, dial_host) in [
        ("100.64.0.2", "100.64.0.2"),
        ("[fd7a:115c:a1e0::2]", "fd7a:115c:a1e0::2"),
    ] {
        let response = client
            .get(format!("http://{url_host}:{}/", web.port()))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let dial = harness
            .launcher
            .node(1)
            .last("POST", "/localapi/v0/dial")
            .unwrap();
        assert_eq!(dial.header("dial-host"), Some(dial_host));
    }
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn bytes_sent_with_the_request_and_with_the_101_are_not_lost() {
    let harness = Harness::new(5).await;
    connected(
        &harness,
        DialPlan::Echo {
            preface: b"welcome".to_vec(),
        },
    )
    .await;
    let url = harness.tailscale.proxy_url(1).unwrap();

    let mut stream = login(
        harness.tailscale.socks_port().unwrap(),
        "1",
        url.password().unwrap(),
    )
    .await
    .unwrap();
    // The request and the first payload in one write.
    let mut message = connect_request(3, b"box.test", 80);
    message.extend_from_slice(b"abc");
    stream.write_all(&message).await.unwrap();
    assert_eq!(read_reply(&mut stream).await, 0);
    let mut received = [0u8; 10];
    stream.read_exact(&mut received).await.unwrap();
    assert_eq!(&received, b"welcomeabc");
    stream.write_all(b"more").await.unwrap();
    let mut echoed = [0u8; 4];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"more");
}

#[tokio::test]
async fn a_wrong_password_a_wrong_user_and_another_nodes_password_are_refused() {
    let (web, hits) = web_server().await;
    let harness = Harness::new(5).await;
    connected(&harness, DialPlan::Tunnel(web)).await;
    harness.launcher.node(2).connect();
    harness.tailscale.ensure(config(2, None)).await.unwrap();
    eventually("node 2", || {
        harness.tailscale.status(2).state == NodeState::Connected
    })
    .await;
    let port = harness.tailscale.socks_port().unwrap();
    let own = harness.tailscale.proxy_url(1).unwrap();
    let other = harness.tailscale.proxy_url(2).unwrap();

    let secret = own.password().unwrap().to_owned();
    for (user, password) in [
        ("1", "not-the-secret".to_owned()),
        ("1", String::new()),
        ("1", format!("{secret}x")),
        ("1", secret[..secret.len() - 1].to_owned()),
        ("1", other.password().unwrap().to_owned()),
        ("99", secret.clone()),
        ("0", secret.clone()),
        ("one", secret.clone()),
        ("", secret.clone()),
        ("+1", secret.clone()),
    ] {
        match login(port, user, &password).await {
            Err(answer) => assert_eq!(answer, [1, 1], "{user:?}"),
            Ok(_) => panic!("{user:?} / {password:?} was accepted"),
        }
    }

    // With a real client the failure is an error, and nothing was dialed.
    let mut wrong = own.clone();
    wrong.set_password(Some("nope")).unwrap();
    let result = client_through(&wrong)
        .get(format!("http://box.tail1234.ts.net:{}/", web.port()))
        .send()
        .await;
    assert!(result.is_err());
    assert_eq!(
        harness.launcher.node(1).count("POST", "/localapi/v0/dial"),
        0
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    // The right one still works.
    assert!(login(port, "1", &secret).await.is_ok());
}

#[tokio::test]
async fn a_removed_node_no_longer_logs_in() {
    let harness = Harness::new(5).await;
    connected(
        &harness,
        DialPlan::Echo {
            preface: Vec::new(),
        },
    )
    .await;
    let url = harness.tailscale.proxy_url(1).unwrap();
    let port = harness.tailscale.socks_port().unwrap();
    assert!(login(port, "1", url.password().unwrap()).await.is_ok());
    harness.tailscale.remove(1).await.unwrap();
    assert!(login(port, "1", url.password().unwrap()).await.is_err());
}

#[tokio::test]
async fn no_authentication_and_other_commands_are_refused() {
    let harness = Harness::new(5).await;
    connected(
        &harness,
        DialPlan::Echo {
            preface: Vec::new(),
        },
    )
    .await;
    let url = harness.tailscale.proxy_url(1).unwrap();
    let port = harness.tailscale.socks_port().unwrap();

    // A client that offers only "no authentication" is told there is no acceptable method.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(&[5, 1, 0]).await.unwrap();
    let mut answer = [0u8; 2];
    stream.read_exact(&mut answer).await.unwrap();
    assert_eq!(answer, [5, 0xFF]);

    // BIND and UDP ASSOCIATE.
    for command in [2u8, 3] {
        let mut stream = login(port, "1", url.password().unwrap()).await.unwrap();
        let mut request = connect_request(1, &[10, 0, 0, 1], 80);
        request[1] = command;
        stream.write_all(&request).await.unwrap();
        assert_eq!(read_reply(&mut stream).await, 0x07, "command {command}");
    }
    // An address type that does not exist.
    let mut stream = login(port, "1", url.password().unwrap()).await.unwrap();
    stream
        .write_all(&connect_request(9, &[1, 2, 3, 4], 80))
        .await
        .unwrap();
    assert_eq!(read_reply(&mut stream).await, 0x08);
    assert_eq!(
        harness.launcher.node(1).count("POST", "/localapi/v0/dial"),
        0
    );
}

#[tokio::test]
async fn a_target_outside_the_tailnet_is_not_dialed_when_it_is_private() {
    let (web, hits) = web_server().await;
    let harness = Harness::new(5).await;
    connected(&harness, DialPlan::SelfDial(web.to_string())).await;
    let url = harness.tailscale.proxy_url(1).unwrap();
    let port = harness.tailscale.socks_port().unwrap();

    // tailscaled says "dial 127.0.0.1 yourself": the container's loopback is not for a proxy.
    let mut stream = login(port, "1", url.password().unwrap()).await.unwrap();
    stream
        .write_all(&connect_request(3, b"example.com", 80))
        .await
        .unwrap();
    assert_eq!(read_reply(&mut stream).await, 0x02);

    let result = client_through(&url)
        .get(format!("http://example.com:{}/", web.port()))
        .send()
        .await;
    assert!(result.is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    for private in [
        "10.1.2.3:80",
        "192.168.0.9:80",
        "169.254.169.254:80",
        "[::1]:80",
    ] {
        harness
            .launcher
            .node(1)
            .set(|state| state.dial = DialPlan::SelfDial(private.into()));
        let mut stream = login(port, "1", url.password().unwrap()).await.unwrap();
        stream
            .write_all(&connect_request(1, &[8, 8, 8, 8], 80))
            .await
            .unwrap();
        assert_eq!(read_reply(&mut stream).await, 0x02, "{private}");
    }
}

#[tokio::test]
async fn dial_failures_become_socks_replies() {
    let harness = Harness::new(5).await;
    connected(
        &harness,
        DialPlan::Fail(502, "dial failure: i/o timeout".into()),
    )
    .await;
    let url = harness.tailscale.proxy_url(1).unwrap();
    let port = harness.tailscale.socks_port().unwrap();

    for (plan, expected) in [
        (
            DialPlan::Fail(502, "resolve failure: no such host".into()),
            0x04,
        ),
        (DialPlan::Fail(500, "boom".into()), 0x01),
        (DialPlan::Fail(403, "access denied".into()), 0x01),
        (DialPlan::SelfDial("not an address".into()), 0x01),
    ] {
        harness.launcher.node(1).set(|state| state.dial = plan);
        let mut stream = login(port, "1", url.password().unwrap()).await.unwrap();
        stream
            .write_all(&connect_request(1, &[100, 64, 0, 2], 80))
            .await
            .unwrap();
        assert_eq!(read_reply(&mut stream).await, expected);
        // The connection ends after a failure.
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    }
}

// Helpers for a client written by hand.

/// Greeting and login. `Err` carries the two bytes of a refusal.
async fn login(port: u16, user: &str, password: &str) -> Result<TcpStream, [u8; 2]> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(&[5, 2, 0, 2]).await.unwrap();
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await.unwrap();
    assert_eq!(method, [5, 2]);
    let mut message = vec![1, user.len() as u8];
    message.extend_from_slice(user.as_bytes());
    message.push(password.len() as u8);
    message.extend_from_slice(password.as_bytes());
    stream.write_all(&message).await.unwrap();
    let mut answer = [0u8; 2];
    stream.read_exact(&mut answer).await.unwrap();
    if answer == [1, 0] {
        Ok(stream)
    } else {
        Err(answer)
    }
}

/// `VER CMD RSV ATYP ADDR PORT` for CONNECT. A domain (type 3) gets its length byte here.
fn connect_request(address_type: u8, address: &[u8], port: u16) -> Vec<u8> {
    let mut request = vec![5, 1, 0, address_type];
    if address_type == 3 {
        request.push(address.len() as u8);
    }
    request.extend_from_slice(address);
    request.extend_from_slice(&port.to_be_bytes());
    request
}

async fn read_reply(stream: &mut TcpStream) -> u8 {
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[0], 5);
    reply[1]
}
