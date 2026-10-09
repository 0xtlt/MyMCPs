//! Backups: an administrator exports one from the Settings page, and an
//! instance that has no user yet is set up from one.
//!
//! The export is a plain form post answered with the file. The import is
//! the one form of the app that carries a file: it is not parsed before its
//! handler runs, as the other forms are, but read here field by field, the
//! file going to disk as it arrives.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, EXPECT};
use http::{HeaderMap, HeaderValue, StatusCode};
use mymcps_core::backup::{self, BackupDir, BackupError, Export, Imported, KeyDerivations};
use mymcps_core::client_ip::rate_limit_client_key;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio::sync::OwnedMutexGuard;
use tokio::time::Instant;

use crate::auth::CurrentUser;
use crate::client_ip::ClientIp;
use crate::csrf::{CSRF_FIELD, CSRF_HEADER, CSRF_MESSAGE, verify_csrf_token};
use crate::error::{AppError, wants_html};
use crate::forms::{FormState, refusal};
use crate::input::{BodyRefusal, Input, normalize_body};
use crate::redirect::{redirect_back, redirect_to, with_query};
use crate::respond::page;
use crate::routes::FeatureRoutes;
use crate::routes::auth::consume;
use crate::routes::settings::{
    BACKUP_FORM, PasswordCheck, confirm_current_password, refuse, wrong_current_password,
};
use crate::session::Session;
use crate::state::AppState;
use crate::validators::backup::{EXPORT_BACKUP_VALIDATOR, IMPORT_BACKUP_VALIDATOR};
use crate::views::auth::import_page;
use crate::views::settings::backup_form;
use crate::views::shell::PageContext;

const IMPORT_PATH: &str = "/onboarding/import";
const FILE_FIELD: &str = "backup";
const PASSWORD_FIELD: &str = "password";
/// The name of the uploaded file in its private directory.
const UPLOAD_FILE: &str = "upload.mymcps";

const IMPORTED: &str = "Backup imported. Sign in with an account of the imported instance.";
const IMPORT_UNDER_WAY: &str = "Another import is in progress. Try again in a moment.";
const FILE_TOO_LARGE: &str = "The backup file is larger than 4 GB";

/// The largest backup file an import takes: 4 GiB.
const MAX_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// What a form holds around its file: boundaries, field headers, the token
/// and the password.
const FORM_OVERHEAD_BYTES: u64 = 1024 * 1024;
/// The token and the password are a few dozen bytes each.
const MAX_TEXT_FIELD_BYTES: u64 = 16 * 1024;
/// The time a client has to send its form. An upload holds the place of
/// the one import that may run: one that stalls must not hold it for good.
const WHOLE_UPLOAD: Duration = Duration::from_secs(30 * 60);

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        admin: Router::new().route("/settings/backup", post(export)),
        first_run_raw: Router::new().route(IMPORT_PATH, get(show_import).post(import)),
        ..Default::default()
    }
}

/// What the backup routes of one server share: the place of the one import
/// that may run, the turn of the one key that may be derived, and the
/// limits of an upload.
#[derive(Debug, Clone)]
pub struct BackupWork {
    /// Held from the first byte of an uploaded file until its import is
    /// over and its files are deleted.
    import: Arc<tokio::sync::Mutex<()>>,
    key_derivations: KeyDerivations,
    max_upload_bytes: u64,
    whole_upload: Duration,
}

impl BackupWork {
    pub fn new() -> Self {
        Self::with_limits(MAX_UPLOAD_BYTES, WHOLE_UPLOAD)
    }

    /// The same, for files of at most `max_upload_bytes` sent within
    /// `whole_upload`. For tests, which can neither send 4 GiB nor wait
    /// half an hour.
    pub fn with_limits(max_upload_bytes: u64, whole_upload: Duration) -> Self {
        Self {
            import: Arc::default(),
            key_derivations: KeyDerivations::new(),
            max_upload_bytes,
            whole_upload,
        }
    }

    /// Take the place of the one import that may run, as an upload does.
    /// `None` while another holds it. For tests of what a second import is
    /// told meanwhile.
    pub fn begin_import(&self) -> Option<OwnedMutexGuard<()>> {
        self.import.clone().try_lock_owned().ok()
    }
}

impl Default for BackupWork {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------------ export

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportRequest {
    password: String,
    current_password: String,
}

/// The answer to an export: the file, streamed as it is encrypted.
fn download(export: Export) -> Result<Response, AppError> {
    let length = export.content_length();
    let disposition = format!("attachment; filename=\"{}\"", export.file_name());

    let blocks = futures::stream::unfold(Some(export), |export| async move {
        let mut export = export?;
        match export.next_block().await {
            Ok(Some(block)) => Some((Ok(Bytes::from(block)), Some(export))),
            Ok(None) => None,
            // The connection is cut short: a file that lacks its end never opens.
            Err(error) => {
                tracing::error!(%error, "A backup could not be written to its end");
                Some((Err(error), None))
            }
        }
    });
    let mut response = Response::new(Body::from_stream(blocks));
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(AppError::internal)?,
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    Ok(response)
}

/// `POST /settings/backup`
///
/// A backup holds every credential of the instance: as for a change of
/// email or password, a signed-in browser is not enough, and the
/// administrator types the password of the account again.
pub async fn export(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let refused = |form: FormState| {
        refuse(
            &headers,
            &session,
            BACKUP_FORM,
            &form,
            backup_form(&context, &form),
        )
    };

    let request: ExportRequest =
        match EXPORT_BACKUP_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(request) => request,
            Err(error) => return Ok(refused(FormState::new(&refusal(error)?, &input))),
        };
    match confirm_current_password(&state, &user, &request.current_password).await? {
        PasswordCheck::Confirmed => {}
        PasswordCheck::Wrong => return Ok(refused(wrong_current_password(&input))),
        PasswordCheck::Limited(response) => return Ok(response),
    }

    let export = Export::start(
        &state.core,
        request.password,
        &state.backups.key_derivations,
    )
    .await
    .map_err(AppError::internal)?;
    // Who and when. The password is in no log.
    tracing::info!(
        user_id = user.id,
        email = %user.email,
        created_at = %export.created_at().to_rfc3339(),
        bytes = export.content_length(),
        "Exported a backup"
    );
    download(export)
}

// ------------------------------------------------------------------ import

/// `GET /onboarding/import`
pub async fn show_import(context: PageContext, session: Session) -> Response {
    page(import_page(&context, &FormState::from_session(&session)))
}

/// Why the body of an upload stopped before its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum Interrupted {
    #[error("The client took too long to send the backup")]
    TooSlow,
    #[error("The backup stopped arriving")]
    Broken,
    #[error("The form holds more than it may")]
    TooLarge,
}

struct ArrivingState {
    chunks: BoxStream<'static, Result<Bytes, Interrupted>>,
    /// The client waits to be told to go ahead before it sends anything.
    waits_to_continue: bool,
    started: bool,
    /// A piece was just handed over: the next one waits for a turn.
    handed: bool,
    /// How much of the body has arrived, and how much may. The form parser
    /// keeps what it reads until it finds the end of what it looks for: a
    /// body with no such end would be kept whole, so no more arrives than
    /// what the parts read so far may hold.
    arrived: u64,
    allowance: u64,
    /// What is left is being dropped as it arrives: nothing holds it.
    dropping: bool,
}

/// The body of an import as it arrives, until the time a whole form is
/// given has passed. Cloning shares it: the form is read through one
/// handle, and what the form did not read is dropped through another.
#[derive(Clone)]
struct Arriving {
    state: Arc<Mutex<ArrivingState>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Arriving {
    fn new(body: Body, headers: &HeaderMap, patience: Duration) -> Self {
        let whole_by = Instant::now() + patience;
        let chunks =
            futures::stream::unfold(Some(body.into_data_stream()), move |body| async move {
                let mut body = body?;
                // Looked at before each piece: one that is already there
                // would be handed over whatever the time.
                let left = whole_by.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Some((Err(Interrupted::TooSlow), None));
                }
                match tokio::time::timeout(left, body.next()).await {
                    Ok(Some(Ok(chunk))) => Some((Ok(chunk), Some(body))),
                    Ok(Some(Err(_))) => Some((Err(Interrupted::Broken), None)),
                    Ok(None) => None,
                    Err(_) => Some((Err(Interrupted::TooSlow), None)),
                }
            });
        Self {
            state: Arc::new(Mutex::new(ArrivingState {
                // Asked again once it has ended, when what is left is dropped.
                chunks: chunks.fuse().boxed(),
                waits_to_continue: headers
                    .get(EXPECT)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("100-continue")),
                started: false,
                handed: false,
                arrived: 0,
                allowance: FORM_OVERHEAD_BYTES,
                dropping: false,
            })),
        }
    }

    /// The file of the form starts here, and is written to disk as it
    /// arrives: let that much more through.
    fn admit_file(&self, max_bytes: u64) {
        let mut state = lock(&self.state);
        state.allowance = state.allowance.saturating_add(max_bytes);
    }

    /// The file has ended: what follows is small again.
    fn file_ended(&self) {
        let mut state = lock(&self.state);
        state.allowance = state.arrived.saturating_add(FORM_OVERHEAD_BYTES);
    }

    /// Read and drop what the client is still sending: a browser reads its
    /// answer once its form is sent, and hanging up on it meanwhile would
    /// lose it the answer. Ends with the time a form is given.
    async fn drain(mut self) {
        {
            // A client that waits to be told to go ahead sends nothing once
            // it has its answer, and asking for its body would tell it to.
            let mut state = lock(&self.state);
            if state.waits_to_continue && !state.started {
                return;
            }
            state.dropping = true;
        }
        while let Some(Ok(_)) = self.next().await {}
    }
}

impl Stream for Arriving {
    type Item = Result<Bytes, Interrupted>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = lock(&self.state);
        state.started = true;
        // One piece a turn. The form parser takes everything that is ready
        // before it hands any of it on: without this, a client faster than
        // the disk would have its file pile up in memory.
        if state.handed {
            state.handed = false;
            context.waker().wake_by_ref();
            return Poll::Pending;
        }
        let next = state.chunks.poll_next_unpin(context);
        if let Poll::Ready(Some(Ok(piece))) = &next {
            state.arrived = state.arrived.saturating_add(piece.len() as u64);
            if !state.dropping && state.arrived > state.allowance {
                return Poll::Ready(Some(Err(Interrupted::TooLarge)));
            }
            state.handed = true;
        }
        next
    }
}

/// Why a form was refused before an import could start.
enum Refused {
    /// No valid CSRF token before the file, or at all.
    Csrf,
    /// Another import holds the place.
    UnderWay,
    FileTooLarge,
    /// Not a form, or one that did not arrive whole.
    Body(BodyRefusal),
}

fn refused_form(error: multer::Error) -> Refused {
    match error {
        multer::Error::FieldSizeExceeded { field_name, .. }
            if field_name.as_deref() == Some(FILE_FIELD) =>
        {
            Refused::FileTooLarge
        }
        multer::Error::StreamSizeExceeded { .. } => Refused::FileTooLarge,
        multer::Error::FieldSizeExceeded { .. } => Refused::Body(BodyRefusal::TooLarge),
        multer::Error::StreamReadFailed(cause) => match cause.downcast_ref::<Interrupted>() {
            Some(Interrupted::TooSlow) => Refused::Body(BodyRefusal::TooSlow),
            Some(Interrupted::TooLarge) => Refused::Body(BodyRefusal::TooLarge),
            _ => Refused::Body(BodyRefusal::Broken),
        },
        _ => Refused::Body(BodyRefusal::Broken),
    }
}

/// The file of an import, on disk, with the place of the one import that
/// may run. Dropping it deletes the file and gives the place back.
struct Upload {
    path: PathBuf,
    bytes: u64,
    // Dropped in this order: the directory is gone before the next import
    // may start.
    dir: BackupDir,
    _turn: OwnedMutexGuard<()>,
}

/// A text field of the form as the other forms of the app are read:
/// trimmed, and missing when nothing is left.
fn normalized(text: String) -> Option<String> {
    let mut value = Value::String(text);
    normalize_body(&mut value);
    match value {
        Value::String(text) => Some(text),
        _ => None,
    }
}

/// Write the file part to disk as it arrives. `None` for a part that holds
/// nothing, which is what a form sends when no file was chosen: nothing is
/// created for it.
async fn receive_file(
    state: &AppState,
    field: &mut multer::Field<'static>,
) -> Result<Result<Option<Upload>, Refused>, AppError> {
    let mut receiving: Option<(tokio::fs::File, Upload)> = None;
    loop {
        let chunk = match field.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => return Ok(Err(refused_form(error))),
        };
        if chunk.is_empty() {
            continue;
        }
        if receiving.is_none() {
            // The first byte of a file: from here to the end of its import,
            // this is the one import of the server.
            let Some(turn) = state.backups.begin_import() else {
                return Ok(Err(Refused::UnderWay));
            };
            let dir = BackupDir::create(&state.core.config).map_err(AppError::internal)?;
            let path = dir.path().join(UPLOAD_FILE);
            let file = backup::private_file()
                .open(&path)
                .map_err(AppError::internal)?;
            receiving = Some((
                tokio::fs::File::from_std(file),
                Upload {
                    path,
                    bytes: 0,
                    dir,
                    _turn: turn,
                },
            ));
        }
        if let Some((file, upload)) = receiving.as_mut() {
            file.write_all(&chunk).await.map_err(AppError::internal)?;
            upload.bytes += chunk.len() as u64;
        }
    }
    let Some((mut file, upload)) = receiving else {
        return Ok(Ok(None));
    };
    // What was written is on its way to the disk in the background: it is
    // there before the file is opened again.
    file.flush().await.map_err(AppError::internal)?;
    Ok(Ok(Some(upload)))
}

struct ImportForm {
    password: Option<String>,
    upload: Option<Upload>,
}

/// Read the form: `_csrf`, `backup` and `password`, in the order they
/// come. The page puts the token first. A file that comes before a valid
/// token is refused with nothing written, and so is a form without one.
async fn read_form(
    state: &AppState,
    session: &Session,
    headers: &HeaderMap,
    body: Arriving,
) -> Result<Result<ImportForm, Refused>, AppError> {
    let header_token = headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok());
    let mut verified = verify_csrf_token(session, header_token);

    let boundary = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|content_type| multer::parse_boundary(content_type).ok());
    let Some(boundary) = boundary else {
        // Not a form that could carry a file, nor a token.
        return Ok(Err(if verified {
            Refused::Body(BodyRefusal::Broken)
        } else {
            Refused::Csrf
        }));
    };
    let max_upload_bytes = state.backups.max_upload_bytes;
    let limits = multer::SizeLimit::new()
        .whole_stream(max_upload_bytes.saturating_add(FORM_OVERHEAD_BYTES))
        .per_field(MAX_TEXT_FIELD_BYTES)
        .for_field(FILE_FIELD, max_upload_bytes);
    let constraints = multer::Constraints::new()
        .allowed_fields(vec![CSRF_FIELD, FILE_FIELD, PASSWORD_FIELD])
        .size_limit(limits);
    let arriving = body.clone();
    let mut form = multer::Multipart::with_constraints(body, boundary, constraints);

    let mut password = None;
    let mut upload = None;
    let mut file_seen = false;
    loop {
        let mut field = match form.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return Ok(Err(refused_form(error))),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == FILE_FIELD {
            if !verified {
                return Ok(Err(Refused::Csrf));
            }
            // One file: a second one would be a second import.
            if std::mem::replace(&mut file_seen, true) {
                return Ok(Err(Refused::Body(BodyRefusal::Broken)));
            }
            arriving.admit_file(max_upload_bytes);
            let received = receive_file(state, &mut field).await?;
            arriving.file_ended();
            upload = match received {
                Ok(upload) => upload,
                Err(refused) => return Ok(Err(refused)),
            };
            continue;
        }
        let text = match field.text().await {
            Ok(text) => normalized(text),
            Err(error) => return Ok(Err(refused_form(error))),
        };
        if name == CSRF_FIELD {
            verified = verified || verify_csrf_token(session, text.as_deref());
        } else {
            password = text;
        }
    }
    if !verified {
        return Ok(Err(Refused::Csrf));
    }
    Ok(Ok(ImportForm { password, upload }))
}

#[derive(Deserialize)]
struct ImportRequest {
    password: String,
}

/// Send the visitor back to the form with what was wrong. `field` is the
/// field the message is shown under, when it is about one.
fn back_to_form(session: &Session, headers: &HeaderMap, field: &str, message: &str) -> Response {
    FormState::with_error(field, message, &Map::new()).flash(session);
    redirect_back(headers, IMPORT_PATH)
}

/// The field an import that failed is about: a wrong password is told
/// under the password, anything else under the file.
fn field_of(error: &BackupError) -> &'static str {
    match error {
        BackupError::WrongPassword => PASSWORD_FIELD,
        _ => FILE_FIELD,
    }
}

/// Import the file, on a task of its own: an import that started runs to
/// its end and deletes its files whether or not the visitor is still
/// there, and the server log says how it went.
async fn run_import(
    state: &AppState,
    client: &str,
    upload: Upload,
    password: String,
) -> Result<Result<Imported, BackupError>, AppError> {
    let state = state.clone();
    let client = client.to_string();
    tokio::spawn(async move {
        let outcome = backup::import(
            &state.core,
            &upload.dir,
            &upload.path,
            password,
            &state.backups.key_derivations,
        )
        .await;
        // The files are deleted, then the place is given back.
        drop(upload);

        match &outcome {
            Ok(imported) => {
                tracing::info!(
                    %client,
                    outcome = "imported",
                    users = imported.users,
                    backup_created_at = %imported.created_at,
                    migrations_run = imported.migrated.len(),
                    reencrypted = imported.reencrypted,
                    "Imported a backup"
                );
                // The schedule in memory is the one of an instance that
                // had no settings.
                if let Err(error) = state.database_replaced().await {
                    tracing::error!(%error, "The background work was not told about the imported backup");
                }
            }
            Err(error) => match error.message() {
                Some(message) => {
                    tracing::warn!(%client, outcome = %message, "Refused the import of a backup")
                }
                None => {
                    tracing::warn!(%client, outcome = %error, "The import of a backup did not finish")
                }
            },
        }
        outcome
    })
    .await
    .map_err(AppError::internal)
}

/// Everything about an import but the body left unread.
async fn admit_and_import(
    state: &AppState,
    session: &Session,
    client: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Arriving,
) -> Result<Response, AppError> {
    // Counted before anything of the body is read. The guard of the route
    // has already seen that the instance has no user.
    let key = format!("backup-import:{}", rate_limit_client_key(client));
    if let Err(refused) = consume(&state.limiters.backup_import, &key).await? {
        tracing::warn!(%client, outcome = "Too many requests", "Refused the import of a backup");
        return Ok(refused);
    }

    let form = match read_form(state, session, headers, body).await? {
        Ok(form) => form,
        Err(refused) => {
            let (outcome, response) = match refused {
                // Answered as the forms that are parsed before their handler.
                Refused::Csrf if wants_html(headers) => {
                    session.flash("error", CSRF_MESSAGE);
                    (CSRF_MESSAGE, redirect_back(headers, "/"))
                }
                Refused::Csrf => (
                    CSRF_MESSAGE,
                    (StatusCode::FORBIDDEN, CSRF_MESSAGE).into_response(),
                ),
                Refused::UnderWay => (
                    IMPORT_UNDER_WAY,
                    back_to_form(session, headers, "import", IMPORT_UNDER_WAY),
                ),
                Refused::FileTooLarge => (
                    FILE_TOO_LARGE,
                    back_to_form(session, headers, FILE_FIELD, FILE_TOO_LARGE),
                ),
                Refused::Body(refusal) => {
                    ("The form did not arrive whole", refusal.into_response())
                }
            };
            tracing::warn!(%client, outcome, "Refused the import of a backup");
            return Ok(response);
        }
    };

    let ImportForm { password, upload } = form;
    let mut input = Map::new();
    if let Some(upload) = &upload {
        input.insert(FILE_FIELD.into(), Value::from(upload.bytes));
    }
    if let Some(password) = password {
        input.insert(PASSWORD_FIELD.into(), Value::String(password));
    }
    let request = IMPORT_BACKUP_VALIDATOR.validate_as::<ImportRequest>(&Value::Object(input));
    let (request, upload) = match (request, upload) {
        (Ok(request), Some(upload)) => (request, upload),
        (Err(error), _) => {
            // Nothing to put back in a file field or a password field.
            let form = FormState::new(&refusal(error)?, &Map::new());
            tracing::warn!(
                %client,
                outcome = form.first_error().unwrap_or_default(),
                "Refused the import of a backup"
            );
            form.flash(session);
            return Ok(redirect_back(headers, IMPORT_PATH));
        }
        (Ok(_), None) => {
            return Err(AppError::internal(
                "An import without a file passed validation",
            ));
        }
    };

    match run_import(state, client, upload, request.password).await? {
        Ok(_) => {
            // Nobody is signed in by an import.
            session.flash("success", IMPORTED);
            Ok(redirect_to("/login"))
        }
        // As the guard of the route answers once the instance is set up.
        Err(BackupError::AlreadySetUp) => Ok(redirect_to(&with_query("/login", query))),
        Err(error) => match error.message() {
            Some(message) => Ok(back_to_form(session, headers, field_of(&error), &message)),
            None => Err(AppError::internal(error)),
        },
    }
}

/// `POST /onboarding/import`: `multipart/form-data` with `_csrf`, `backup`
/// and `password`.
pub async fn import(
    State(state): State<AppState>,
    session: Session,
    ClientIp(client): ClientIp,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = Arriving::new(body, &parts.headers, state.backups.whole_upload);

    let answer = admit_and_import(
        &state,
        &session,
        &client,
        parts.uri.query(),
        &parts.headers,
        body.clone(),
    )
    .await
    .unwrap_or_else(IntoResponse::into_response);
    body.drain().await;
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn pieces(count: usize) -> Body {
        Body::from_stream(futures::stream::iter(
            (0..count).map(|index| Ok::<_, std::io::Error>(Bytes::from(vec![index as u8; 10]))),
        ))
    }

    #[tokio::test]
    async fn hands_over_one_piece_of_a_body_a_turn() {
        let mut body = Arriving::new(pieces(3), &HeaderMap::new(), WHOLE_UPLOAD);
        let waker = futures::task::noop_waker();
        let mut context = Context::from_waker(&waker);

        // Everything is ready, and still a reader that asks until it is
        // told to wait gets one piece each time it comes back.
        for index in 0..3u8 {
            let piece = Pin::new(&mut body).poll_next(&mut context);
            assert!(
                matches!(&piece, Poll::Ready(Some(Ok(piece))) if piece[..] == [index; 10]),
                "{piece:?}"
            );
            assert!(Pin::new(&mut body).poll_next(&mut context).is_pending());
        }
        assert!(matches!(
            Pin::new(&mut body).poll_next(&mut context),
            Poll::Ready(None)
        ));
    }

    #[tokio::test]
    async fn gives_a_form_a_time_to_arrive_and_drops_what_nobody_read() {
        // A body that never ends: a first piece, then nothing.
        let stalled = futures::stream::once(async { Ok::<_, std::io::Error>(Bytes::from("--x")) })
            .chain(futures::stream::pending());
        let mut body = Arriving::new(
            Body::from_stream(stalled),
            &HeaderMap::new(),
            Duration::from_millis(50),
        );
        assert_eq!(body.next().await, Some(Ok(Bytes::from("--x"))));
        assert_eq!(body.next().await, Some(Err(Interrupted::TooSlow)));
        assert_eq!(body.next().await, None);

        // What is left of a body is read through any handle of it.
        let body = Arriving::new(pieces(5), &HeaderMap::new(), WHOLE_UPLOAD);
        let mut reader = body.clone();
        assert!(reader.next().await.is_some());
        body.drain().await;
        assert_eq!(reader.next().await, None);

        // No more arrives than the parts read so far may hold, until what
        // is left is dropped.
        let large = || {
            Body::from_stream(futures::stream::iter((0..3).map(|_| {
                Ok::<_, std::io::Error>(Bytes::from(vec![0; FORM_OVERHEAD_BYTES as usize / 2 + 1]))
            })))
        };
        let mut body = Arriving::new(large(), &HeaderMap::new(), WHOLE_UPLOAD);
        assert!(matches!(body.next().await, Some(Ok(_))));
        assert_eq!(body.next().await, Some(Err(Interrupted::TooLarge)));
        let mut body = Arriving::new(large(), &HeaderMap::new(), WHOLE_UPLOAD);
        body.admit_file(FORM_OVERHEAD_BYTES);
        assert!(matches!(body.next().await, Some(Ok(_))));
        assert!(matches!(body.next().await, Some(Ok(_))));
        body.file_ended();
        assert!(matches!(body.next().await, Some(Ok(_))));
        assert_eq!(body.next().await, None);
        let body = Arriving::new(large(), &HeaderMap::new(), WHOLE_UPLOAD);
        let mut reader = body.clone();
        body.drain().await;
        assert_eq!(reader.next().await, None);

        // A client that waits to be told to go ahead is not asked for a
        // body nobody wants.
        let waiting = Arriving::new(
            pieces(2),
            &headers(&[("expect", "100-continue")]),
            WHOLE_UPLOAD,
        );
        let mut reader = waiting.clone();
        waiting.drain().await;
        assert!(reader.next().await.is_some());
    }

    #[test]
    fn tells_a_file_that_is_too_large_from_a_form_that_is_not_one() {
        let too_large = |name: Option<&str>| multer::Error::FieldSizeExceeded {
            limit: 1,
            field_name: name.map(str::to_string),
        };
        assert!(matches!(
            refused_form(too_large(Some("backup"))),
            Refused::FileTooLarge
        ));
        assert!(matches!(
            refused_form(multer::Error::StreamSizeExceeded { limit: 1 }),
            Refused::FileTooLarge
        ));
        assert!(matches!(
            refused_form(too_large(Some("password"))),
            Refused::Body(BodyRefusal::TooLarge)
        ));
        assert!(matches!(
            refused_form(multer::Error::StreamReadFailed(Box::new(
                Interrupted::TooSlow
            ))),
            Refused::Body(BodyRefusal::TooSlow)
        ));
        assert!(matches!(
            refused_form(multer::Error::StreamReadFailed(Box::new(
                Interrupted::Broken
            ))),
            Refused::Body(BodyRefusal::Broken)
        ));
        assert!(matches!(
            refused_form(multer::Error::IncompleteStream),
            Refused::Body(BodyRefusal::Broken)
        ));
        assert!(matches!(
            refused_form(multer::Error::UnknownField {
                field_name: Some("other".into())
            }),
            Refused::Body(BodyRefusal::Broken)
        ));
    }

    #[test]
    fn reads_text_fields_as_the_other_forms_are_read() {
        assert_eq!(
            normalized("  pass word \n".into()).as_deref(),
            Some("pass word")
        );
        assert_eq!(normalized("   ".into()), None);
        assert_eq!(normalized(String::new()), None);
    }
}
