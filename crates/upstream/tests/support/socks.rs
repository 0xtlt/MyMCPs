//! A minimal SOCKS5 server, standing in for the front of a Tailscale node: it
//! asks for a user name and a password, records the name it is asked to
//! connect to, and connects to `target` instead, whatever the name.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use url::Url;

pub struct Socks5 {
    address: SocketAddr,
    username: String,
    password: String,
    connects: Arc<Mutex<Vec<String>>>,
    server: JoinHandle<()>,
}

impl Socks5 {
    /// The proxy URL a client uses, with the right credentials.
    pub fn url(&self) -> Url {
        self.url_with(&self.username, &self.password)
    }

    pub fn url_with(&self, username: &str, password: &str) -> Url {
        Url::parse(&format!("socks5h://{username}:{password}@{}", self.address)).unwrap()
    }

    /// The proxy URL without credentials.
    pub fn url_without_credentials(&self) -> Url {
        Url::parse(&format!("socks5h://{}", self.address)).unwrap()
    }

    /// The `host:port` of every connection a client asked for, once it was
    /// authenticated.
    pub fn connects(&self) -> Vec<String> {
        self.connects.lock().unwrap().clone()
    }
}

impl Drop for Socks5 {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Start the server on 127.0.0.1. It only accepts user name and password
/// authentication with these, and every connection it is asked for goes to `target`.
pub async fn socks5(username: &str, password: &str, target: Option<SocketAddr>) -> Socks5 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let connects: Arc<Mutex<Vec<String>>> = Arc::default();
    let server = tokio::spawn({
        let connects = Arc::clone(&connects);
        let credentials = (username.to_owned(), password.to_owned());
        async move {
            while let Ok((stream, _)) = listener.accept().await {
                let connects = Arc::clone(&connects);
                let credentials = credentials.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, credentials, target, connects).await;
                });
            }
        }
    });
    Socks5 {
        address,
        username: username.to_owned(),
        password: password.to_owned(),
        connects,
        server,
    }
}

async fn serve(
    mut stream: TcpStream,
    (username, password): (String, String),
    target: Option<SocketAddr>,
    connects: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    // Greeting: version, number of methods, methods.
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).await?;
    let mut methods = vec![0u8; usize::from(head[1])];
    stream.read_exact(&mut methods).await?;
    if head[0] != 5 || !methods.contains(&2) {
        // No acceptable method: an unauthenticated client is refused.
        return stream.write_all(&[5, 0xFF]).await;
    }
    stream.write_all(&[5, 2]).await?;

    // Username and password (RFC 1929).
    let mut version = [0u8; 2];
    stream.read_exact(&mut version).await?;
    let mut given_username = vec![0u8; usize::from(version[1])];
    stream.read_exact(&mut given_username).await?;
    let mut length = [0u8; 1];
    stream.read_exact(&mut length).await?;
    let mut given_password = vec![0u8; usize::from(length[0])];
    stream.read_exact(&mut given_password).await?;
    if given_username != username.as_bytes() || given_password != password.as_bytes() {
        return stream.write_all(&[1, 1]).await;
    }
    stream.write_all(&[1, 0]).await?;

    // Request: version, command, reserved, address type, address, port.
    let mut request = [0u8; 4];
    stream.read_exact(&mut request).await?;
    let host = match request[3] {
        1 => {
            let mut octets = [0u8; 4];
            stream.read_exact(&mut octets).await?;
            std::net::Ipv4Addr::from(octets).to_string()
        }
        3 => {
            let mut length = [0u8; 1];
            stream.read_exact(&mut length).await?;
            let mut name = vec![0u8; usize::from(length[0])];
            stream.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        _ => return stream.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await,
    };
    let mut port = [0u8; 2];
    stream.read_exact(&mut port).await?;
    connects
        .lock()
        .unwrap()
        .push(format!("{host}:{}", u16::from_be_bytes(port)));

    let Some(target) = target else {
        return stream.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
    };
    let mut upstream = TcpStream::connect(target).await?;
    stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    tokio::io::copy_bidirectional(&mut stream, &mut upstream)
        .await
        .map(|_| ())
}
