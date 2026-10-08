//! The temporary links of the built-in MCPs: the files their tools hand out,
//! such as mail attachments, and the files sent to them, such as the
//! attachments of a mail to send. (`app/controllers/builtin_files_controller.ts`)
//!
//! Neither route has a session, a CSRF token or a parsed body. The signature
//! in the link is the only credential, so it is checked before anything is
//! counted or read, and the MCP is checked again in case it changed since
//! the link was made.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{OriginalUri, Path, Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use bytes::Bytes;
use chrono::{DateTime, SecondsFormat};
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    EXPECT, RETRY_AFTER, X_CONTENT_TYPE_OPTIONS,
};
use http::{HeaderMap, HeaderValue, StatusCode, Uri};
use mymcps_builtin::file_link::{self, BUILTIN_FILE_PURPOSE, BUILTIN_UPLOAD_PURPOSE};
use mymcps_builtin::places::{LimitedPlaces, Place};
use mymcps_builtin::upload_store::{UploadError, UploadRefusal};
use mymcps_builtin::{BuiltinUpload, BuiltinUploadTarget};
use mymcps_core::Core;
use mymcps_core::models::{Mcp, McpTransport};
use mymcps_core::redaction::sanitize_mcp_diagnostic;
use mymcps_vine as vine;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::json;
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::client_ip::ClientIp;
use crate::error::AppError;
use crate::routes::FeatureRoutes;
use crate::state::AppState;
use crate::validators::builtin_files::{FileLink, file_link as link_parameters};

static MEDIA_TYPE: LazyLock<vine::JsRegex> =
    LazyLock::new(|| vine::js::regex(r"^[\w.+-]+\/[\w.+-]+$", "").expect("a static pattern"));
const INVALID_LINK: &str = "This link is invalid or has expired.";
const UNAVAILABLE: &str = "This file is no longer available.";
const UPLOAD_UNAVAILABLE: &str = "This upload link can no longer be used.";
const HOW_TO_UPLOAD: &str = "Send the file itself as the body of the PUT request, for example with: curl -T <file> \"<link>\"";

/// A download signs in to the provider and keeps the whole file in memory
/// until the client has received it, so an MCP serves only a few at once.
const MAX_CONCURRENT_DOWNLOADS: usize = 3;
/// An upload is written to disk as it arrives, and keeps a connection open meanwhile.
const MAX_CONCURRENT_UPLOADS: usize = 3;
const BUSY_RETRY_SECONDS: u64 = 5;
/// A client that stops reading, or sending, must not keep its place for good.
const STALLED_CLIENT: Duration = Duration::from_secs(60);
/// What Node gave any request to arrive whole. Without it, a client sending
/// a few bytes a minute would keep its place for as long as it pleased.
const WHOLE_UPLOAD: Duration = Duration::from_secs(300);
/// A file is handed to the connection in pieces of this size at most, so
/// that the time since the last one says whether the client still reads,
/// and so that a connection never holds more of a file than a few of them.
const PIECE_BYTES: usize = 16 * 1024;

/// What `encodeURIComponent` escapes, and `'()*` with it.
const FILENAME_ESCAPED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~');

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        machine: Router::new()
            .route("/files/{id}/{reference}", get(show))
            .route("/uploads/{id}/{reference}", put(store)),
        ..Default::default()
    }
}

/// The downloads and uploads of built-in MCP files that are under way, and
/// how long a client may keep one waiting. The Node app counted them in the
/// variables of a module; here the count belongs to the server.
#[derive(Debug, Clone)]
pub struct FileTraffic {
    downloads: LimitedPlaces,
    uploads: LimitedPlaces,
    stalled_client: Duration,
    whole_upload: Duration,
}

impl FileTraffic {
    pub fn new() -> Self {
        Self::with_patience(STALLED_CLIENT, WHOLE_UPLOAD)
    }

    /// The same places, for clients given `stalled_client` between two
    /// pieces of a file and `whole_upload` to send one whole. For tests,
    /// which cannot wait a minute.
    pub fn with_patience(stalled_client: Duration, whole_upload: Duration) -> Self {
        Self {
            downloads: LimitedPlaces::new(MAX_CONCURRENT_DOWNLOADS),
            uploads: LimitedPlaces::new(MAX_CONCURRENT_UPLOADS),
            stalled_client,
            whole_upload,
        }
    }
}

impl Default for FileTraffic {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn text(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

fn try_again_in(seconds: u64, message: &'static str) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(RETRY_AFTER, HeaderValue::from(seconds))],
        message,
    )
        .into_response()
}

/// Whether the link carries our signature for this purpose. The signature
/// covers the path, and a link has no other parameter: one more, or the
/// signature twice, is not a link we made.
fn has_valid_signature(core: &Core, uri: &Uri, purpose: &str) -> bool {
    let mut signature = None;
    for (name, value) in url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if name != "signature" || signature.is_some() {
            return false;
        }
        signature = Some(value.into_owned());
    }
    file_link::has_valid_signature(
        core,
        uri.path(),
        purpose,
        signature
            .as_deref()
            .filter(|signature| !signature.is_empty()),
    )
}

/// The MCP and the reference a signed link names. `None` when they are not
/// what a tool puts in a link.
fn link_of(params: Result<Path<(String, String)>, PathRejection>) -> Option<FileLink> {
    let Path((id, reference)) = params.ok()?;
    link_parameters(&id, &reference)
}

/// The MCP of a link, when it still is one that serves files.
async fn linked_mcp(state: &AppState, id: i64) -> Result<Option<Mcp>, AppError> {
    let mcp = Mcp::find(&*state.core.db, id).await?;
    Ok(mcp.filter(|mcp| mcp.enabled && mcp.transport == McpTransport::Builtin))
}

/// Name the download after the file, in ASCII for old clients and in full for the others.
fn attachment_disposition(filename: &str) -> String {
    let name: String = filename
        .chars()
        .map(|character| match character {
            '"' | '\\' | '/' => '_',
            control if control.is_control() => '_',
            other => other,
        })
        .collect();
    let mut ascii = String::with_capacity(name.len());
    for character in name.chars() {
        if (' '..='~').contains(&character) {
            ascii.push(character);
        } else {
            // One for each UTF-16 unit, as JavaScript replaced them.
            ascii.extend(std::iter::repeat_n('_', character.len_utf16()));
        }
    }
    let encoded = utf8_percent_encode(&name, FILENAME_ESCAPED);
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

/// A file being sent to a client, and the place it holds meanwhile.
struct Leaving {
    pieces: VecDeque<Bytes>,
    taken_at: Instant,
    _place: Place,
}

/// The body of a download. It keeps the place of the download for as long
/// as the file is in memory: until the client has it, has gone, or has
/// stopped reading for too long.
struct FileBody {
    leaving: Arc<Mutex<Option<Leaving>>>,
}

#[derive(Debug, thiserror::Error)]
#[error("The client stopped reading the file")]
struct StalledClient;

impl FileBody {
    fn new(content: Vec<Bytes>, place: Place, patience: Duration) -> Self {
        let leaving = Arc::new(Mutex::new(Some(Leaving {
            pieces: content.into(),
            taken_at: Instant::now(),
            _place: place,
        })));
        tokio::spawn(give_up_on_a_stalled_client(
            Arc::downgrade(&leaving),
            patience,
        ));
        Self { leaving }
    }
}

/// Drop the file and give its place back once nothing was taken of it for
/// `patience`. Ends when the body is dropped, which does the same.
async fn give_up_on_a_stalled_client(leaving: Weak<Mutex<Option<Leaving>>>, patience: Duration) {
    loop {
        let wait_until = {
            let Some(leaving) = leaving.upgrade() else {
                return;
            };
            let mut leaving = lock(&leaving);
            match leaving.as_ref() {
                Some(file) if file.taken_at.elapsed() < patience => file.taken_at + patience,
                _ => {
                    *leaving = None;
                    return;
                }
            }
        };
        tokio::time::sleep_until(wait_until).await;
    }
}

impl Stream for FileBody {
    type Item = Result<Bytes, StalledClient>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut leaving = lock(&self.leaving);
        let Some(file) = leaving.as_mut() else {
            // Given up on: the connection must not look like it sent a whole file.
            return Poll::Ready(Some(Err(StalledClient)));
        };
        while let Some(mut piece) = file.pieces.pop_front() {
            if piece.is_empty() {
                continue;
            }
            if piece.len() > PIECE_BYTES {
                let rest = piece.split_off(PIECE_BYTES);
                file.pieces.push_front(rest);
            }
            file.taken_at = Instant::now();
            // A copy: the connection keeps what it was handed until the
            // client has read it, and a part of a larger piece would keep
            // the whole of it in memory after the file was given up on.
            return Poll::Ready(Some(Ok(Bytes::copy_from_slice(&piece))));
        }
        Poll::Ready(None)
    }
}

/// Serve the file behind a temporary link that a built-in tool handed out.
/// There is no session and no access token: the signature is the credential,
/// so the MCP is checked again in case it changed since the link was made.
async fn show(
    State(state): State<AppState>,
    ClientIp(address): ClientIp,
    OriginalUri(uri): OriginalUri,
    params: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, AppError> {
    // Checked before anything is counted: only the holder of a link can use up downloads.
    if !has_valid_signature(&state.core, &uri, BUILTIN_FILE_PURPOSE) {
        return Ok(text(StatusCode::FORBIDDEN, INVALID_LINK));
    }

    let Some(FileLink { id, reference }) = link_of(params) else {
        return Ok(text(StatusCode::NOT_FOUND, UNAVAILABLE));
    };

    // Counted for each MCP and address, so one client cannot use up the downloads of another.
    let client = format!("builtin-file:{id}:{address}");
    let limiter = &state.limiters.builtin_file;
    if !limiter.attempt(&client).await? {
        return Ok(try_again_in(
            limiter.available_in(&client).await?,
            "Too many downloads. Try again later.",
        ));
    }

    let Some(mut mcp) = linked_mcp(&state, id).await? else {
        return Ok(text(StatusCode::NOT_FOUND, UNAVAILABLE));
    };

    let Some(place) = state.file_traffic.downloads.take(mcp.id) else {
        return Ok(try_again_in(
            BUSY_RETRY_SECONDS,
            "Too many downloads at once. Try again in a few seconds.",
        ));
    };

    // On its own task, with its place: a client that hangs up does not stop
    // its download, which still signs out of the provider when it is done.
    let upstream = state.upstream.clone();
    let download = tokio::spawn(async move {
        let file = upstream.download_builtin_file(&mut mcp, reference).await;
        (file, mcp, place)
    });
    let (file, mcp, place) = download.await.map_err(AppError::internal)?;
    let file = match file {
        Ok(file) => file,
        Err(error) if error.is_tool_error() => {
            return Ok(text(StatusCode::NOT_FOUND, UNAVAILABLE));
        }
        Err(error) => {
            return Err(AppError::internal(sanitize_mcp_diagnostic(
                &state.core.encryption,
                &error.to_string(),
                &mcp,
            )));
        }
    };

    // The file was written by a stranger. A browser must save it, never
    // render it on this origin.
    let content_type = Some(file.content_type.as_str())
        .filter(|content_type| MEDIA_TYPE.test(content_type))
        .and_then(|content_type| HeaderValue::from_str(content_type).ok())
        .unwrap_or(HeaderValue::from_static("application/octet-stream"));
    let disposition = HeaderValue::from_str(&attachment_disposition(&file.filename))
        .map_err(AppError::internal)?;
    let length: usize = file.content.iter().map(Bytes::len).sum();

    // The place is kept for as long as the file is in memory.
    let body = FileBody::new(file.content, place, state.file_traffic.stalled_client);
    let mut response = Response::new(Body::from_stream(body));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, content_type);
    headers.insert(CONTENT_DISPOSITION, disposition);
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    Ok(response)
}

/// Why a body stopped before its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum Interrupted {
    /// Nothing arrived for too long, or the whole took too long.
    #[error("The client took too long to send the file")]
    TooSlow,
    /// The client hung up, or what it sent is not a body.
    #[error("The file stopped arriving")]
    Broken,
}

/// The body of an upload as it arrives, for as long as the client keeps
/// sending and until the time a whole file is given has passed.
struct Arriving {
    chunks: BoxStream<'static, Result<Bytes, Interrupted>>,
    /// The client waits to be told to go ahead before it sends anything.
    waits_to_continue: bool,
    started: bool,
    interrupted: Option<Interrupted>,
}

impl Arriving {
    fn new(body: Body, headers: &HeaderMap, traffic: &FileTraffic) -> Self {
        let stalled_client = traffic.stalled_client;
        let whole_by = Instant::now() + traffic.whole_upload;
        let chunks =
            futures::stream::unfold(Some(body.into_data_stream()), move |body| async move {
                let mut body = body?;
                // Looked at before each piece: one that is already there
                // would be handed over whatever the time.
                let left = whole_by.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Some((Err(Interrupted::TooSlow), None));
                }
                match tokio::time::timeout(stalled_client.min(left), body.next()).await {
                    Ok(Some(Ok(chunk))) => Some((Ok(chunk), Some(body))),
                    Ok(Some(Err(_))) => Some((Err(Interrupted::Broken), None)),
                    Ok(None) => None,
                    Err(_) => Some((Err(Interrupted::TooSlow), None)),
                }
            });
        Self {
            // Asked again once it has ended, when what is left is dropped.
            chunks: chunks.fuse().boxed(),
            waits_to_continue: headers
                .get(EXPECT)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("100-continue")),
            started: false,
            interrupted: None,
        }
    }

    /// What the client is still sending is read and dropped, like any body
    /// nobody asked for: hanging up on a client that is still sending would
    /// lose it the answer.
    async fn drain(mut self) {
        // A client that waits to be told to go ahead sends nothing once it
        // has its answer, and asking for its body would tell it to.
        if self.waits_to_continue && !self.started {
            return;
        }
        while let Some(Ok(_)) = self.next().await {
            // A client that sends without a pause keeps nothing else waiting.
            tokio::task::yield_now().await;
        }
    }

    fn discard(self) {
        tokio::spawn(self.drain());
    }
}

impl Stream for Arriving {
    type Item = Result<Bytes, Interrupted>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.started = true;
        let next = self.chunks.poll_next_unpin(context);
        if let Poll::Ready(Some(Err(interrupted))) = &next {
            self.interrupted = Some(*interrupted);
        }
        next
    }
}

/// What the sender of a file is told when it was not kept.
fn upload_refusal(reason: UploadRefusal, max_bytes: u64) -> (StatusCode, String) {
    match reason {
        UploadRefusal::Taken => (
            StatusCode::CONFLICT,
            "A file was already sent to this link. Ask for a new link to send another one."
                .to_owned(),
        ),
        UploadRefusal::Empty => (
            StatusCode::BAD_REQUEST,
            format!("The request has no body. {HOW_TO_UPLOAD}"),
        ),
        UploadRefusal::TooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "The file is larger than the {} MB this link takes.",
                max_bytes as f64 / 1_000_000.0
            ),
        ),
        UploadRefusal::Full => (
            StatusCode::TOO_MANY_REQUESTS,
            "Too many uploaded files are waiting for this MCP. They are deleted an hour after their upload: try again later."
                .to_owned(),
        ),
    }
}

fn refuse_upload(reason: UploadRefusal, max_bytes: u64) -> Response {
    let (status, message) = upload_refusal(reason, max_bytes);
    text(status, message)
}

/// What the client says it is about to send, when it says so.
fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn is_a_form(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.get(.."multipart/".len()))
        .is_some_and(|start| start.eq_ignore_ascii_case("multipart/"))
}

/// An upload that may go ahead, or the answer to one that may not.
enum Admission {
    Accepted {
        mcp_id: i64,
        target: BuiltinUploadTarget,
        place: Place,
    },
    Refused(Response),
}

/// Everything that is decided about an upload before its body is read.
async fn admit_upload(
    state: &AppState,
    address: &str,
    uri: &Uri,
    params: Result<Path<(String, String)>, PathRejection>,
    headers: &HeaderMap,
) -> Result<Admission, AppError> {
    let refused = |response| Ok(Admission::Refused(response));

    // Checked before anything is counted or read: only the holder of a link can send a file.
    if !has_valid_signature(&state.core, uri, BUILTIN_UPLOAD_PURPOSE) {
        return refused(text(StatusCode::FORBIDDEN, INVALID_LINK));
    }

    let Some(FileLink { id, reference }) = link_of(params) else {
        return refused(text(StatusCode::NOT_FOUND, UPLOAD_UNAVAILABLE));
    };

    let client = format!("builtin-upload:{id}:{address}");
    let limiter = &state.limiters.builtin_upload;
    if !limiter.attempt(&client).await? {
        return refused(try_again_in(
            limiter.available_in(&client).await?,
            "Too many uploads. Try again later.",
        ));
    }

    let Some(mut mcp) = linked_mcp(state, id).await? else {
        return refused(text(StatusCode::NOT_FOUND, UPLOAD_UNAVAILABLE));
    };

    let target = match state
        .upstream
        .builtin_upload_target(&mut mcp, reference)
        .await
    {
        Ok(target) => target,
        Err(error) if error.is_tool_error() => {
            return refused(text(StatusCode::NOT_FOUND, UPLOAD_UNAVAILABLE));
        }
        Err(error) => {
            return Err(AppError::internal(sanitize_mcp_diagnostic(
                &state.core.encryption,
                &error.to_string(),
                &mcp,
            )));
        }
    };

    // A form would be stored with its boundaries and field headers around the file.
    if is_a_form(headers) {
        return refused(text(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("A form cannot be stored as a file. {HOW_TO_UPLOAD}"),
        ));
    }
    // Most clients say how much they are about to send.
    if declared_length(headers).is_some_and(|length| length > target.max_bytes) {
        return refused(refuse_upload(UploadRefusal::TooLarge, target.max_bytes));
    }

    let Some(place) = state.file_traffic.uploads.take(mcp.id) else {
        return refused(try_again_in(
            BUSY_RETRY_SECONDS,
            "Too many uploads at once. Try again in a few seconds.",
        ));
    };
    Ok(Admission::Accepted {
        mcp_id: mcp.id,
        target,
        place,
    })
}

fn receipt(upload: &BuiltinUpload) -> Result<Response, AppError> {
    let expires_at = DateTime::from_timestamp_millis(upload.expires_at)
        .ok_or_else(|| AppError::internal("An upload expires at a time that is not a date"))?
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let body = json!({
        "upload_id": upload.id,
        "filename": upload.filename,
        "size": upload.size,
        "expires_at": expires_at,
    });
    Ok((
        StatusCode::CREATED,
        [(CONTENT_TYPE, "application/json; charset=utf-8")],
        body.to_string(),
    )
        .into_response())
}

/// Keep the file sent to a temporary link that a built-in tool handed out,
/// for the tool to use afterwards. As for a download, the signature is the
/// credential and the MCP is checked again. The body is the file whatever
/// its content type: it is not parsed, and goes to disk as it arrives.
async fn store(
    State(state): State<AppState>,
    ClientIp(address): ClientIp,
    OriginalUri(uri): OriginalUri,
    params: Result<Path<(String, String)>, PathRejection>,
    request: Request,
) -> Result<Response, AppError> {
    let (parts, body) = request.into_parts();
    let mut body = Arriving::new(body, &parts.headers, &state.file_traffic);

    let admission = admit_upload(&state, &address, &uri, params, &parts.headers).await;
    let (mcp_id, target, place) = match admission {
        Ok(Admission::Accepted {
            mcp_id,
            target,
            place,
        }) => (mcp_id, target, place),
        Ok(Admission::Refused(response)) => {
            body.discard();
            return Ok(response);
        }
        Err(error) => {
            body.discard();
            return Err(error);
        }
    };

    // On its own task, with its place: when the client hangs up, the file
    // that was arriving is still removed, so nothing is kept of it and its
    // link can be tried again.
    let max_bytes = target.max_bytes;
    let uploads = state.upstream.builtin_env().uploads.clone();
    let (answer, saved) = oneshot::channel();
    tokio::spawn(async move {
        let saved = uploads.save(mcp_id, &target, &mut body).await;
        drop(place);
        let _ = answer.send((saved, body.interrupted));
        // Reading stops at the first byte too many, which must not close the
        // connection the answer is sent on.
        body.drain().await;
    });

    match saved.await.map_err(AppError::internal)? {
        (Ok(upload), _) => receipt(&upload),
        (Err(UploadError::Refused(reason)), _) => Ok(refuse_upload(reason, max_bytes)),
        // Nothing was kept. The Node app closed the connection of a client
        // that had gone quiet, and had nobody left to answer when one hung up.
        (Err(UploadError::Body(_)), Some(Interrupted::TooSlow)) => {
            Ok(StatusCode::REQUEST_TIMEOUT.into_response())
        }
        (Err(UploadError::Body(_)), _) => Ok(StatusCode::BAD_REQUEST.into_response()),
        (Err(error), _) => Err(AppError::internal(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hands_a_file_over_in_small_pieces_that_do_not_hold_the_rest_of_it() {
        let places = LimitedPlaces::new(1);
        let large = Bytes::from(vec![7; 40_000]);
        let memory = large.as_ptr_range();
        let mut body = FileBody::new(
            vec![Bytes::new(), large, Bytes::from_static(b"end")],
            places.take(1).unwrap(),
            STALLED_CLIENT,
        );

        let mut sizes = Vec::new();
        while let Some(piece) = body.next().await {
            let piece = piece.unwrap();
            assert!(!memory.contains(&piece.as_ptr()));
            assert!(piece.iter().all(|byte| *byte == 7) || piece == "end");
            sizes.push(piece.len());
        }
        assert_eq!(
            sizes,
            [PIECE_BYTES, PIECE_BYTES, 40_000 - 2 * PIECE_BYTES, 3]
        );

        // The place is kept until the connection is done with the body.
        assert!(places.take(1).is_none());
        drop(body);
        assert!(places.take(1).is_some());
    }

    #[tokio::test]
    async fn gives_up_on_a_file_nobody_takes() {
        let places = LimitedPlaces::new(1);
        let patience = Duration::from_millis(50);
        let mut body = FileBody::new(
            vec![Bytes::from(vec![7; 3 * PIECE_BYTES])],
            places.take(1).unwrap(),
            patience,
        );
        assert_eq!(body.next().await.unwrap().unwrap().len(), PIECE_BYTES);
        assert!(places.take(1).is_none());

        tokio::time::sleep(patience * 3).await;
        // The place is free and the file is gone, though the body is still held.
        assert!(places.take(1).is_some());
        assert!(lock(&body.leaving).is_none());
        assert!(body.next().await.unwrap().is_err());
    }

    #[test]
    fn names_a_download_in_ascii_and_in_full() {
        assert_eq!(
            attachment_disposition("Menu été.pdf"),
            "attachment; filename=\"Menu _t_.pdf\"; filename*=UTF-8''Menu%20%C3%A9t%C3%A9.pdf"
        );
        assert_eq!(
            attachment_disposition("report.pdf"),
            "attachment; filename=\"report.pdf\"; filename*=UTF-8''report.pdf"
        );
        // Nothing of a name can end the quoted string, add a parameter or name a folder.
        assert_eq!(
            attachment_disposition("a\"b\\c/d\r\ne\u{0}f\u{7f}g\u{85}h"),
            "attachment; filename=\"a_b_c_d__e_f_g_h\"; filename*=UTF-8''a_b_c_d__e_f_g_h"
        );
        // What `encodeURIComponent` leaves alone, less what RFC 5987 does not allow.
        assert_eq!(
            attachment_disposition("it's (1) *new* ~v1.0!_-;=%.txt"),
            "attachment; filename=\"it's (1) *new* ~v1.0!_-;=%.txt\"; filename*=UTF-8''it%27s%20%281%29%20%2Anew%2A%20~v1.0!_-%3B%3D%25.txt"
        );
        // Outside the basic plane: two UTF-16 units in JavaScript, so two underscores.
        assert_eq!(
            attachment_disposition("😀.png"),
            "attachment; filename=\"__.png\"; filename*=UTF-8''%F0%9F%98%80.png"
        );
        // What a lone surrogate became when the name was read.
        assert_eq!(
            attachment_disposition("Menu \u{fffd}.pdf"),
            "attachment; filename=\"Menu _.pdf\"; filename*=UTF-8''Menu%20%EF%BF%BD.pdf"
        );
        assert_eq!(
            attachment_disposition(""),
            "attachment; filename=\"\"; filename*=UTF-8''"
        );
    }

    #[test]
    fn only_a_plain_media_type_is_sent_as_it_is() {
        for plain in [
            "application/pdf",
            "image/svg+xml",
            "application/vnd.ms-excel",
            "text/x-c_source",
        ] {
            assert!(MEDIA_TYPE.test(plain), "{plain}");
        }
        for other in [
            "",
            "pdf",
            "text/html; charset=utf-8",
            "text/plain\n",
            "text/plain\r\nX-Injected: 1",
            "a/b/c",
            "/pdf",
            "text/",
            "téxt/plain",
        ] {
            assert!(!MEDIA_TYPE.test(other), "{other:?}");
        }
    }

    #[test]
    fn says_why_a_file_was_not_kept() {
        let how = "Send the file itself as the body of the PUT request, for example with: curl -T <file> \"<link>\"";
        assert_eq!(
            upload_refusal(UploadRefusal::Taken, 20_000_000),
            (
                StatusCode::CONFLICT,
                "A file was already sent to this link. Ask for a new link to send another one."
                    .to_owned()
            )
        );
        assert_eq!(
            upload_refusal(UploadRefusal::Empty, 20_000_000),
            (
                StatusCode::BAD_REQUEST,
                format!("The request has no body. {how}")
            )
        );
        assert_eq!(
            upload_refusal(UploadRefusal::Full, 20_000_000),
            (
                StatusCode::TOO_MANY_REQUESTS,
                "Too many uploaded files are waiting for this MCP. They are deleted an hour after their upload: try again later."
                    .to_owned()
            )
        );
        // The megabytes are written as JavaScript wrote the division.
        for (max_bytes, megabytes) in [
            (20_000_000, "20"),
            (5_242_880, "5.24288"),
            (1_500_000, "1.5"),
            (1024, "0.001024"),
        ] {
            assert_eq!(
                upload_refusal(UploadRefusal::TooLarge, max_bytes),
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("The file is larger than the {megabytes} MB this link takes.")
                )
            );
        }
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    http::HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                )
            })
            .collect()
    }

    #[test]
    fn reads_what_the_client_says_it_sends() {
        assert_eq!(declared_length(&headers(&[])), None);
        assert_eq!(
            declared_length(&headers(&[("content-length", "20000001")])),
            Some(20_000_001)
        );
        assert_eq!(
            declared_length(&headers(&[("content-length", " 12 ")])),
            Some(12)
        );
        for unreadable in ["", "abc", "-1", "1.5", "99999999999999999999999"] {
            assert_eq!(
                declared_length(&headers(&[("content-length", unreadable)])),
                None,
                "{unreadable}"
            );
        }
    }

    #[test]
    fn tells_a_form_from_a_file() {
        for form in [
            "multipart/form-data; boundary=x",
            "Multipart/Mixed",
            "MULTIPART/",
        ] {
            assert!(is_a_form(&headers(&[("content-type", form)])), "{form}");
        }
        for file in [
            "application/pdf",
            "application/x-www-form-urlencoded",
            "text/multipart/",
            "multipart",
            "",
        ] {
            assert!(!is_a_form(&headers(&[("content-type", file)])), "{file}");
        }
        assert!(!is_a_form(&headers(&[])));
    }
}
