//! What the tests of the file links share: two built-in MCPs written for
//! them, the links their tools would hand out, and clients that send what
//! the test client of the pages cannot (a body that stays open, a body that
//! breaks off, a connection that stops reading).
//!
//! The mailbox stands in for iCloud Mail: a password sign-in whose
//! permissions MyMCPs enforces, one attachment to download, and uploads of
//! up to 20 MB. The image library stands in for Google Ads: an OAuth sign-in
//! with one switch for write access, and uploads of up to 5,242,880 bytes.
//! Neither reaches the network.
#![allow(dead_code)]

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::ServiceExt;
use axum::body::Body;
use axum::extract::ConnectInfo;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use http::{HeaderMap, Method, Request, Response, StatusCode};
use http_body_util::BodyExt;
use mymcps_builtin::arguments::{
    TOOL_VINE, VineArgument, argument, integer, line, media_type, pattern, uploaded_file_name,
};
use mymcps_builtin::file_link::{builtin_file_url, builtin_upload_url};
use mymcps_builtin::tool_input::tool_input;
use mymcps_builtin::upload_store::is_builtin_upload_id;
use mymcps_builtin::{
    BuiltinError, BuiltinFile, BuiltinMcpDefinition, BuiltinOauthConfig, BuiltinPasswordConfig,
    BuiltinPasswordContext, BuiltinProvider, BuiltinRegistry, BuiltinToolContext,
    BuiltinUploadTarget, UploadStore,
};
use mymcps_core::Config;
use mymcps_core::crypto::{random_hex, sign_path};
use mymcps_core::models::{Mcp, McpStatus, McpTransport, User};
use mymcps_upstream::Upstream;
use mymcps_vine as vine;
use mymcps_web::AppState;
use mymcps_web::routes::builtin_files::FileTraffic;
use mymcps_web::testing::TestApp;
use mymcps_web::testing::factories::{create_admin, create_mcp};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tower::ServiceExt as _;

/// The bytes of the attachment of message 11.
pub const ATTACHMENT: &str = "%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n";

pub const PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n";

/// What the mailbox takes for one attachment, and serves for one.
pub const MAILBOX_UPLOAD_BYTES: u64 = 20_000_000;
const MAILBOX_DOWNLOAD_BYTES: usize = 30_000_000;
/// What the image library takes for one image.
pub const IMAGE_BYTES: u64 = 5_242_880;

pub const INVALID_LINK: &str = "This link is invalid or has expired.";
pub const UNAVAILABLE: &str = "This file is no longer available.";
pub const UPLOAD_UNAVAILABLE: &str = "This upload link can no longer be used.";
pub const HOW_TO_UPLOAD: &str = "Send the file itself as the body of the PUT request, for example with: curl -T <file> \"<link>\"";

struct Attachment {
    filename: String,
    content_type: String,
    pieces: Vec<Bytes>,
}

/// The account behind the mailbox MCP: the attachment it serves, and what
/// was asked of it.
pub struct Mailbox {
    sign_ins: Mutex<HashMap<i64, usize>>,
    sign_outs: AtomicUsize,
    attachment: Mutex<Attachment>,
    held: Mutex<Option<(i64, watch::Receiver<bool>)>>,
}

impl Default for Mailbox {
    fn default() -> Self {
        Self {
            sign_ins: Mutex::default(),
            sign_outs: AtomicUsize::new(0),
            attachment: Mutex::new(Attachment {
                filename: "Menu été.pdf".to_owned(),
                content_type: "application/pdf".to_owned(),
                pieces: vec![Bytes::from_static(ATTACHMENT.as_bytes())],
            }),
            held: Mutex::default(),
        }
    }
}

/// Lets the downloads held by [`Mailbox::hold_downloads`] go on.
pub struct HeldDownloads(watch::Sender<bool>);

impl HeldDownloads {
    pub fn open(&self) {
        let _ = self.0.send(true);
    }
}

impl Mailbox {
    /// How many downloads reached the account.
    pub fn sign_ins(&self) -> usize {
        self.sign_ins.lock().unwrap().values().sum()
    }

    /// How many downloads of one MCP reached the account.
    pub fn sign_ins_of(&self, mcp_id: i64) -> usize {
        self.sign_ins
            .lock()
            .unwrap()
            .get(&mcp_id)
            .copied()
            .unwrap_or(0)
    }

    /// How many downloads left the account, served or not.
    pub fn sign_outs(&self) -> usize {
        self.sign_outs.load(Ordering::SeqCst)
    }

    /// Hold every download of one MCP at the account until `open` is called,
    /// the way a large attachment keeps its connection busy.
    pub fn hold_downloads(&self, mcp_id: i64) -> HeldDownloads {
        let (open, opened) = watch::channel(false);
        *self.held.lock().unwrap() = Some((mcp_id, opened));
        HeldDownloads(open)
    }

    /// Deliver the content of every download in the given pieces instead.
    pub fn serve_in_pieces(&self, pieces: Vec<Bytes>) {
        self.attachment.lock().unwrap().pieces = pieces;
    }

    pub fn name_attachment(&self, filename: &str) {
        self.attachment.lock().unwrap().filename = filename.to_owned();
    }

    pub fn label_attachment(&self, content_type: &str) {
        self.attachment.lock().unwrap().content_type = content_type.to_owned();
    }

    fn held_for(&self, mcp_id: i64) -> Option<watch::Receiver<bool>> {
        self.held
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(held, _)| *held == mcp_id)
            .map(|(_, opened)| opened.clone())
    }
}

/// One session at the account. It ends when the download does, served or not.
struct Session(Arc<Mailbox>);

impl Drop for Session {
    fn drop(&mut self) {
        self.0.sign_outs.fetch_add(1, Ordering::SeqCst);
    }
}

/// What the mailbox puts in a download link.
static ATTACHMENT_REFERENCE: LazyLock<vine::Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "mailbox" => line(200).optional(),
        "uid" => integer(1..=4_294_967_295),
        "part" => pattern(r"^\d{1,3}(\.\d{1,3}){0,9}$", "the part of an attachment, such as 2"),
    })
});

#[derive(Deserialize)]
struct AttachmentReference {
    uid: u32,
    part: String,
}

fn upload_id() -> VineArgument {
    argument(vine::rule(|value, field| {
        if !value.as_str().is_some_and(is_builtin_upload_id) {
            field.report("{{ field }} must be an upload ID", "uploadId");
        }
    }))
}

/// What both MCPs put in an upload link.
static UPLOAD_REFERENCE: LazyLock<vine::Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "upload" => upload_id(),
        "filename" => uploaded_file_name(255),
        "content_type" => media_type().optional(),
    })
});

#[derive(Deserialize)]
struct UploadReference {
    upload: String,
    filename: String,
    content_type: Option<String>,
}

fn allows(context: &BuiltinPasswordContext, permission: &str) -> bool {
    context
        .permissions
        .iter()
        .any(|allowed| allowed == permission)
}

async fn download_attachment(
    mailbox: Arc<Mailbox>,
    reference: Value,
    context: Arc<BuiltinPasswordContext>,
) -> Result<BuiltinFile, BuiltinError> {
    // The link outlives the call that made it, so the permission is checked again.
    if !allows(&context, "read") {
        return Err(BuiltinError::tool(
            "The \"read\" permission is no longer allowed for this MCP",
        ));
    }
    let AttachmentReference { uid, part } = tool_input(&ATTACHMENT_REFERENCE, &reference)?;

    *mailbox
        .sign_ins
        .lock()
        .unwrap()
        .entry(context.mcp_id)
        .or_insert(0) += 1;
    let _session = Session(mailbox.clone());
    if let Some(mut opened) = mailbox.held_for(context.mcp_id) {
        let _ = opened.wait_for(|opened| *opened).await;
    }

    // The message text is not a file to hand out.
    if uid != 11 || part != "2" {
        return Err(BuiltinError::tool(format!(
            "Message {uid} has no attachment at part \"{part}\"."
        )));
    }
    let attachment = mailbox.attachment.lock().unwrap();
    if attachment.pieces.iter().map(Bytes::len).sum::<usize>() > MAILBOX_DOWNLOAD_BYTES {
        return Err(BuiltinError::tool(format!(
            "Attachment \"{}\" cannot be downloaded",
            attachment.filename
        )));
    }
    Ok(BuiltinFile {
        filename: attachment.filename.clone(),
        content_type: attachment.content_type.clone(),
        content: attachment.pieces.clone(),
    })
}

pub const MAILBOX_KEY: &str = "mailbox";
pub const IMAGES_KEY: &str = "images";

fn mailbox_definition(mailbox: Arc<Mailbox>) -> BuiltinMcpDefinition {
    let provider = BuiltinProvider::new(
        MAILBOX_KEY,
        "Mailbox",
        Vec::new(),
        |_: Arc<BuiltinPasswordContext>| async { Ok(()) },
    )
    .download(
        move |reference: Value, context: Arc<BuiltinPasswordContext>| {
            download_attachment(mailbox.clone(), reference, context)
        },
    )
    .upload(
        |reference: Value, context: Arc<BuiltinPasswordContext>| async move {
            if !allows(&context, "draft") && !allows(&context, "send") {
                return Err(BuiltinError::tool(
                    "Neither the \"draft\" nor the \"send\" permission is allowed for this MCP any more",
                ));
            }
            let UploadReference {
                upload,
                filename,
                content_type,
            } = tool_input(&UPLOAD_REFERENCE, &reference)?;
            Ok(BuiltinUploadTarget {
                id: upload,
                filename,
                content_type,
                max_bytes: MAILBOX_UPLOAD_BYTES,
            })
        },
    );

    let pattern = |source: &str| {
        vine::js::regex(source, "")
            .expect("a static pattern")
            .as_regex()
            .clone()
    };
    BuiltinMcpDefinition::Password {
        provider,
        password: BuiltinPasswordConfig {
            username_pattern: pattern(r"^[^\s@]+@[^\s@]+$"),
            username_hint: "The address of the mailbox",
            password_pattern: pattern(r"^[a-z-]+$"),
            password_hint: "An app password",
            permissions: vec!["read", "organize", "draft", "send"],
            alias_hint: "Other addresses of the mailbox",
        },
    }
}

fn images_definition() -> BuiltinMcpDefinition {
    let provider = BuiltinProvider::new(
        IMAGES_KEY,
        "Images",
        Vec::new(),
        |_: Arc<BuiltinToolContext>| async { Ok(()) },
    )
    .upload(|reference: Value, _: Arc<BuiltinToolContext>| async move {
        let UploadReference {
            upload, filename, ..
        } = tool_input(&UPLOAD_REFERENCE, &reference)?;
        Ok(BuiltinUploadTarget {
            id: upload,
            filename,
            content_type: None,
            max_bytes: IMAGE_BYTES,
        })
    });
    BuiltinMcpDefinition::Oauth {
        provider,
        oauth: BuiltinOauthConfig {
            issuer: "https://images.example.test",
            authorize_url: "https://images.example.test/oauth/authorize",
            token_url: "https://images.example.test/oauth/token",
            scopes: vec!["images"],
            write_scopes: Vec::new(),
            scope_separator: " ",
            authorize_params: Vec::new(),
            sends_redirect_uri_with_code: true,
            client_id_pattern: None,
            client_id_hint: None,
        },
    }
}

/// The app with the two MCPs of these tests in place of the real ones.
pub struct Files {
    pub app: TestApp,
    pub mailbox: Arc<Mailbox>,
    admin: User,
}

pub async fn files() -> Files {
    files_with(|_| {}, FileTraffic::new()).await
}

/// The app for clients given less time than a real server gives them.
pub async fn impatient_files(stalled_client: Duration, whole_upload: Duration) -> Files {
    files_with(
        |_| {},
        FileTraffic::with_patience(stalled_client, whole_upload),
    )
    .await
}

pub async fn files_with(adjust: impl FnOnce(&mut Config), traffic: FileTraffic) -> Files {
    let mailbox = Arc::new(Mailbox::default());
    let registry = BuiltinRegistry::new(vec![
        mailbox_definition(mailbox.clone()),
        images_definition(),
    ]);
    let app = TestApp::with_state(adjust, |core| {
        let upstream = Upstream::new(core.clone(), registry);
        let mut state = AppState::with_upstream(core, upstream);
        state.file_traffic = traffic;
        state
    })
    .await;
    let admin = create_admin(&app).await;
    Files {
        app,
        mailbox,
        admin,
    }
}

impl Files {
    pub fn state(&self) -> AppState {
        self.app.state.clone()
    }

    /// The administrator the MCPs belong to.
    pub fn admin(&self) -> &User {
        &self.admin
    }

    /// A mailbox MCP whose sign-in may do what `permissions` say.
    pub async fn mailbox_mcp(&self, permissions: &[&str]) -> Mcp {
        create_mcp(&self.app, self.admin.id, |mcp| {
            mcp.transport = McpTransport::Builtin;
            mcp.builtin_key = Some(MAILBOX_KEY.to_owned());
            mcp.builtin_username = Some("thomas@example.com".to_owned());
            mcp.builtin_password = self.app.core.encrypt_secret(Some("abcd-efgh-ijkl-mnop"));
            mcp.builtin_permissions = Some(permissions.join(" "));
        })
        .await
    }

    /// A connected image library MCP.
    pub async fn images_mcp(&self, write_enabled: bool) -> Mcp {
        create_mcp(&self.app, self.admin.id, |mcp| {
            mcp.transport = McpTransport::Builtin;
            mcp.builtin_key = Some(IMAGES_KEY.to_owned());
            mcp.status = McpStatus::Ready;
            mcp.builtin_write_enabled = write_enabled;
            mcp.oauth_access_token = self.app.core.encrypt_secret(Some("images-access-token"));
            mcp.oauth_token_type = Some("Bearer".to_owned());
        })
        .await
    }

    /// The row as the database holds it now.
    pub async fn reload(&self, mcp: &Mcp) -> Mcp {
        Mcp::find(&*self.app.core.db, mcp.id)
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn save(&self, mcp: &mut Mcp) {
        mcp.save(&*self.app.core.db).await.unwrap();
    }

    /// A link like the ones a tool hands out for a download.
    pub fn file_link(&self, mcp_id: i64, reference: &Value) -> String {
        self.file_link_for(mcp_id, reference, Duration::from_secs(60))
    }

    pub fn file_link_for(&self, mcp_id: i64, reference: &Value, expires_in: Duration) -> String {
        on_this_server(&builtin_file_url(&self.app.core, mcp_id, reference, expires_in).unwrap())
    }

    /// A link to the attachment the mailbox serves.
    pub fn attachment_link(&self, mcp: &Mcp) -> String {
        self.file_link(mcp.id, &attachment())
    }

    /// A link like the ones a tool hands out for an upload, and the ID the
    /// file will be known by.
    pub fn upload_link(&self, mcp: &Mcp) -> UploadLink {
        self.upload_link_named(mcp, "report.pdf")
    }

    pub fn upload_link_named(&self, mcp: &Mcp, filename: &str) -> UploadLink {
        let upload = new_upload_id();
        let link = self.upload_link_to(
            mcp.id,
            &json!({ "upload": upload, "filename": filename }),
            Duration::from_secs(60),
        );
        UploadLink { upload, link }
    }

    pub fn upload_link_to(&self, mcp_id: i64, reference: &Value, expires_in: Duration) -> String {
        on_this_server(&builtin_upload_url(&self.app.core, mcp_id, reference, expires_in).unwrap())
    }

    /// A link with our signature, to a path no tool would write.
    pub fn signed(&self, path: &str, purpose: &str) -> String {
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(60);
        let signature = sign_path(&self.app.core.encryption, path, purpose, expires_at);
        format!("{path}?signature={signature}")
    }

    pub fn uploads(&self) -> UploadStore {
        self.app.state.upstream.builtin_env().uploads.clone()
    }

    /// The uploaded files of an MCP on disk, arrived whole or not.
    pub fn stored_files(&self, mcp: &Mcp) -> Vec<String> {
        let directory = self.uploads().root().join(mcp.id.to_string());
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| !name.ends_with(".json"))
            .collect()
    }

    pub async fn uploaded(&self, mcp: &Mcp, upload: &str) -> Option<Vec<u8>> {
        self.uploads().read(mcp.id, upload).await.unwrap()
    }

    /// Rolled-back tests of the Node app reused MCP ids; here it empties the
    /// disk the way an hour does.
    pub fn clear_uploads(&self) {
        let _ = std::fs::remove_dir_all(self.uploads().root());
    }

    pub async fn get(&self, link: &str) -> Answer {
        send(
            &self.state(),
            request(Method::GET, link).body(Body::empty()).unwrap(),
        )
        .await
    }

    /// Send a file as most clients do: saying how long it is, and nothing else.
    pub async fn put(&self, link: &str, file: impl Into<Bytes>) -> Answer {
        send(&self.state(), put(link, file, &[])).await
    }

    pub async fn put_labelled(
        &self,
        link: &str,
        file: impl Into<Bytes>,
        content_type: &str,
    ) -> Answer {
        send(
            &self.state(),
            put(link, file, &[("content-type", content_type)]),
        )
        .await
    }
}

pub struct UploadLink {
    pub upload: String,
    pub link: String,
}

pub fn attachment() -> Value {
    json!({ "mailbox": "INBOX", "uid": 11, "part": "2" })
}

pub fn encoded(reference: &Value) -> String {
    URL_SAFE_NO_PAD.encode(reference.to_string())
}

/// An ID like the ones upload links are made with.
pub fn new_upload_id() -> String {
    let hex = random_hex(16);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Links name APP_URL, which is not where a test sends its requests. The
/// signature covers the path, so the same link works on both.
pub fn on_this_server(link: &str) -> String {
    link.strip_prefix("http://localhost:3333")
        .expect("a link to the public address of the instance")
        .to_owned()
}

/// The same link with another signature.
pub fn forged(link: &str) -> String {
    let (path, _) = link.split_once('?').unwrap();
    format!("{path}?signature=forged")
}

pub fn unsigned(link: &str) -> String {
    link.split_once('?').unwrap().0.to_owned()
}

pub fn path_of(link: &str) -> &str {
    link.split_once('?').unwrap().0
}

pub fn signature_of(link: &str) -> &str {
    link.split_once("?signature=").unwrap().1
}

/// What a request was answered.
#[derive(Debug)]
pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The status and the text, which is all most refusals are.
    pub fn told(&self) -> (u16, String) {
        (self.status.as_u16(), self.text())
    }
}

/// A request with no cookie, no CSRF token and no access token: the
/// signature is the credential.
pub fn request(method: Method, link: &str) -> http::request::Builder {
    Request::builder().method(method).uri(link)
}

/// A PUT of `file` that says how long it is.
pub fn put(link: &str, file: impl Into<Bytes>, headers: &[(&str, &str)]) -> Request<Body> {
    let file: Bytes = file.into();
    let mut builder = request(Method::PUT, link).header("content-length", file.len());
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(file)).unwrap()
}

/// A PUT whose body does not say how long it is, like a file that is piped.
pub fn put_stream(link: &str, body: Body) -> Request<Body> {
    request(Method::PUT, link).body(body).unwrap()
}

/// Send a request straight to the router, as coming from 127.0.0.1, and hand
/// the answer over with its body still to read.
pub fn open(
    state: &AppState,
    mut request: Request<Body>,
) -> impl Future<Output = Response<Body>> + Send + 'static + use<> {
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 49152))));
    let service = mymcps_web::app::service(state.clone());
    async move {
        service
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {})
    }
}

/// Send a request straight to the router and read the whole answer.
pub fn send(
    state: &AppState,
    request: Request<Body>,
) -> impl Future<Output = Answer> + Send + 'static + use<> {
    let response = open(state, request);
    async move {
        let (parts, body) = response.await.into_parts();
        let body = body
            .collect()
            .await
            .expect("an answer whose body arrives whole")
            .to_bytes();
        Answer {
            status: parts.status,
            headers: parts.headers,
            body,
        }
    }
}

/// The sending end of a body that stays open, like a large file on a slow line.
pub struct OpenBody(mpsc::UnboundedSender<Result<Bytes, std::io::Error>>);

impl OpenBody {
    pub fn send(&self, bytes: impl Into<Bytes>) {
        let _ = self.0.send(Ok(bytes.into()));
    }

    /// End the body: the file has arrived whole.
    pub fn finish(self) {}

    /// Break the connection off, as a client that hangs up does.
    pub fn hang_up(self) {
        let _ = self.0.send(Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset",
        )));
    }
}

/// A body that has sent `first` and stays open until it is finished.
pub fn open_body(first: &'static str) -> (OpenBody, Body) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let body = OpenBody(sender);
    body.send(first);
    let chunks = futures::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    (body, Body::from_stream(chunks))
}

/// Wait, for a few seconds at most, until `condition` holds.
pub async fn eventually(condition: impl Fn() -> bool) {
    for _ in 0..1000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("condition did not hold in time");
}

/// The app behind a real socket on 127.0.0.1, for what only a connection
/// shows: an answer that arrives while the request is still being sent, a
/// client that hangs up, a client that stops reading.
pub struct Server {
    pub address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn serve(state: &AppState) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let service = mymcps_web::app::service(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<SocketAddr>(
                service,
            ),
        )
        .await
        .unwrap();
    });
    Server { address, task }
}

/// One connection to a [`Server`], written to and read from by hand.
pub struct Connection {
    stream: TcpStream,
    received: Vec<u8>,
}

impl Connection {
    pub async fn open(server: &Server) -> Self {
        Self {
            stream: TcpStream::connect(server.address).await.unwrap(),
            received: Vec::new(),
        }
    }

    pub async fn write(&mut self, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
        self.stream.write_all(bytes.as_ref()).await
    }

    /// The first lines of a request, up to the empty one.
    pub async fn write_head(&mut self, method: &str, link: &str, headers: &[(&str, &str)]) {
        let mut head = format!("{method} {link} HTTP/1.1\r\nHost: localhost\r\n");
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        self.write(head).await.unwrap();
    }

    async fn receive(&mut self) -> usize {
        let mut buffer = vec![0; 64 * 1024];
        let read = self.stream.read(&mut buffer).await.unwrap_or(0);
        self.received.extend_from_slice(&buffer[..read]);
        read
    }

    /// The status line and the headers of the next answer.
    pub async fn head(&mut self) -> Answer {
        let end = loop {
            if let Some(end) = self
                .received
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
            {
                break end;
            }
            assert!(
                self.receive().await > 0,
                "the connection closed before an answer"
            );
        };
        let head: Vec<u8> = self.received.drain(..end + 4).collect();
        let head = String::from_utf8_lossy(&head[..end]).into_owned();
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap().split(' ').nth(1).unwrap();
        let mut headers = HeaderMap::new();
        for line in lines {
            let (name, value) = line.split_once(':').unwrap();
            headers.append(
                http::HeaderName::try_from(name.trim()).unwrap(),
                value.trim().parse().unwrap(),
            );
        }
        Answer {
            status: StatusCode::from_bytes(status.as_bytes()).unwrap(),
            headers,
            body: Bytes::new(),
        }
    }

    /// The next answer, whole.
    pub async fn answer(&mut self) -> Answer {
        let mut answer = self.head().await;
        let length: usize = answer
            .header("content-length")
            .map_or(0, |length| length.parse().unwrap());
        while self.received.len() < length {
            assert!(
                self.receive().await > 0,
                "the connection closed before the end of an answer"
            );
        }
        answer.body = self.received.drain(..length).collect::<Vec<u8>>().into();
        answer
    }

    /// Read until the server closes the connection, and say how much came.
    pub async fn read_to_end(&mut self) -> usize {
        let mut total = self.received.len();
        self.received.clear();
        loop {
            let read = self.receive().await;
            self.received.clear();
            if read == 0 {
                return total;
            }
            total += read;
        }
    }
}
