//! The Access tokens page: the tokens made by hand and the OAuth connections
//! of MCP clients. (`access_tokens_controller.ts`)

use std::collections::HashMap;

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use chrono::{DateTime, Utc};
use http::header::CACHE_CONTROL;
use http::{HeaderMap, HeaderValue, StatusCode, Uri};
use mymcps_core::models::{AccessToken, Mcp, ScopeMode, TokenSource};
use mymcps_core::{Db, Timestamp};
use mymcps_gateway::access_token::{self, AccessTokenUpdate, NewAccessToken};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sqlx::{QueryBuilder, Sqlite};

use crate::auth::CurrentUser;
use crate::error::{AppError, is_fetch};
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::{redirect_back, with_query};
use crate::respond::{fragment, invalid_form, navigate, page};
use crate::routes::FeatureRoutes;
use crate::session::Session;
use crate::state::AppState;
use crate::validators::access_token::{
    ACCESS_TOKEN_PARAMS_VALIDATOR, CREATE_ACCESS_TOKEN_VALIDATOR, DELETE_ACCESS_TOKENS_VALIDATOR,
    TOKEN_PAGE_VALIDATOR, TOKEN_STATUS_VALIDATOR, UPDATE_ACCESS_TOKEN_VALIDATOR,
};
use crate::validators::session::FLASHED_TEXT_VALIDATOR;
use crate::views::shell::PageContext;
use crate::views::tokens::{
    Counts, ListView, OpenDialog, ROWS_PER_PAGE, StatusFilter, TokenRow, TokensPage, create_form,
    edit_form, tokens_page,
};

/// Session key of the token the previous request created, shown once.
const CREATED_PLAINTEXT_KEY: &str = "createdPlaintext";

const TOKEN_NOT_FOUND: &str = "Token not found";
const UNKNOWN_MCPS: &str = "One or more selected MCPs do not exist";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        signed_in: Router::new()
            .route("/tokens", get(index).post(store).delete(destroy))
            .route("/tokens/new", get(new))
            .route("/tokens/install", get(install))
            .route("/tokens/{id}", put(update))
            .route("/tokens/{id}/edit", get(edit))
            .route("/tokens/{id}/revoke", post(revoke)),
        ..Default::default()
    }
}

/// The token list, followed by the query string of the request being
/// answered, as the Node app's redirects carried it over: the forms of the
/// page name the view they were sent from there. The method override of an
/// HTML form is how the request was sent, not part of the view.
fn forwarded_index(uri: &Uri) -> String {
    let kept: Vec<&str> = uri
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|pair| !pair.is_empty() && pair.split('=').next() != Some("_method"))
        .collect();
    with_query("/tokens", Some(&kept.join("&")))
}

/// The page lists identifiers and, once, shows a token in full: the browser
/// keeps no copy of it, neither on disk nor for its back button.
fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The part of the list the query string asks for. A value that is not one
/// of the choices asks for the default.
fn list_view(input: &Map<String, Value>) -> ListView {
    let status = TOKEN_STATUS_VALIDATOR.validate(input.get("status")).ok();
    let page = TOKEN_PAGE_VALIDATOR
        .validate(input.get("page"))
        .ok()
        .and_then(|page| page.as_u64())
        .and_then(|page| usize::try_from(page).ok())
        .unwrap_or(1);
    ListView {
        status: StatusFilter::parse(status.as_ref().and_then(Value::as_str)),
        page,
    }
}

/// The id in a path segment, or `None` when the segment is not one.
fn token_id(segment: &str) -> Option<i64> {
    ACCESS_TOKEN_PARAMS_VALIDATOR
        .try_validate(&json!({ "id": segment }))
        .ok()?
        .get("id")?
        .as_i64()
}

async fn load_mcps(db: &Db) -> Result<Vec<Mcp>, sqlx::Error> {
    sqlx::query_as("select * from `mcps` order by `name` asc")
        .fetch_all(&**db)
        .await
}

async fn mcp_ids_of(db: &Db, token_id: i64) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "select `mcps`.`id` from `mcps` \
         inner join `access_token_mcps` on `mcps`.`id` = `access_token_mcps`.`mcp_id` \
         where `access_token_mcps`.`access_token_id` = ? order by `mcps`.`id` asc",
    )
    .bind(token_id)
    .fetch_all(&**db)
    .await
}

/// Every token, newest first, with the MCPs it is limited to.
async fn load_rows(db: &Db) -> Result<Vec<TokenRow>, sqlx::Error> {
    let tokens: Vec<AccessToken> =
        sqlx::query_as("select * from `access_tokens` order by `created_at` desc, `id` desc")
            .fetch_all(&**db)
            .await?;
    let links: Vec<(i64, i64)> = sqlx::query_as(
        "select `access_token_mcps`.`access_token_id`, `mcps`.`id` from `mcps` \
         inner join `access_token_mcps` on `mcps`.`id` = `access_token_mcps`.`mcp_id` \
         order by `mcps`.`id` asc",
    )
    .fetch_all(&**db)
    .await?;
    let mut mcp_ids: HashMap<i64, Vec<i64>> = HashMap::new();
    for (token_id, mcp_id) in links {
        mcp_ids.entry(token_id).or_default().push(mcp_id);
    }
    Ok(tokens
        .into_iter()
        .map(|token| TokenRow {
            mcp_ids: mcp_ids.remove(&token.id).unwrap_or_default(),
            token,
        })
        .collect())
}

/// Whether every one of these ids is the id of an MCP. An id given twice
/// counts twice, and so is refused.
async fn all_mcps_exist(db: &Db, mcp_ids: &[i64]) -> Result<bool, sqlx::Error> {
    if mcp_ids.is_empty() {
        return Ok(true);
    }
    let mut query = QueryBuilder::<Sqlite>::new("select count(*) from `mcps` where `id` in (");
    let mut ids = query.separated(", ");
    for mcp_id in mcp_ids {
        ids.push_bind(*mcp_id);
    }
    query.push(")");
    let existing: i64 = query.build_query_scalar().fetch_one(&**db).await?;
    Ok(usize::try_from(existing).is_ok_and(|existing| existing == mcp_ids.len()))
}

/// The dialog a URL of the page names.
enum Open {
    None,
    Create,
    Install,
    Edit(i64),
}

async fn render(
    state: &AppState,
    mut context: PageContext,
    session: &Session,
    input: &Map<String, Value>,
    open: Open,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let rows = load_rows(db).await?;
    let mcps = load_mcps(db).await?;

    let counts = Counts {
        all: rows.len(),
        active: rows.iter().filter(|row| row.token.is_active()).count(),
        inactive: rows.iter().filter(|row| !row.token.is_active()).count(),
    };
    let deletable_ids: Vec<i64> = rows
        .iter()
        .filter(|row| row.can_delete())
        .map(|row| row.token.id)
        .collect();

    let mut view = list_view(input);
    let in_view: Vec<&TokenRow> = rows
        .iter()
        .filter(|row| view.status.shows(&row.token))
        .collect();
    let total = in_view.len();
    // A page past the end, such as the last one after its tokens were deleted.
    view.page = view.page.min(total.div_ceil(ROWS_PER_PAGE).max(1));
    let shown: Vec<TokenRow> = in_view
        .into_iter()
        .skip((view.page - 1) * ROWS_PER_PAGE)
        .take(ROWS_PER_PAGE)
        .cloned()
        .collect();

    let created_plaintext = FLASHED_TEXT_VALIDATOR
        .validate_as::<String>(session.flashed(CREATED_PLAINTEXT_KEY).as_ref())
        .ok();
    // The sentence that announces the token stays next to it, in a banner:
    // it is not shown a second time as a toast.
    let created_message = match created_plaintext {
        Some(_) => context.flash_success.take(),
        None => None,
    };
    let gateway_url = state
        .core
        .config
        .public_oauth_app_url()
        .map(|app_url| format!("{app_url}/mcp"));
    let editing = match open {
        Open::Edit(id) => rows.iter().find(|row| row.token.id == id),
        _ => None,
    };

    let markup = tokens_page(
        &context,
        &TokensPage {
            view,
            rows: &shown,
            total,
            counts,
            deletable_ids: &deletable_ids,
            mcps: &mcps,
            gateway_url: gateway_url.as_deref(),
            created_plaintext: created_plaintext.as_deref(),
            created_message: created_message.as_deref(),
            open: match (open, editing) {
                (Open::Create, _) => OpenDialog::Create,
                (Open::Install, _) => OpenDialog::Install,
                (Open::Edit(_), Some(row)) => OpenDialog::Edit(row),
                _ => OpenDialog::None,
            },
        },
    );
    Ok(no_store(page(markup)))
}

/// `GET /tokens`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    Input(input): Input,
) -> Result<Response, AppError> {
    render(&state, context, &session, &input, Open::None).await
}

/// `GET /tokens/new`: the page with the create dialog open.
pub async fn new(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    Input(input): Input,
) -> Result<Response, AppError> {
    render(&state, context, &session, &input, Open::Create).await
}

/// `GET /tokens/install`: the page with the install dialog open.
pub async fn install(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    Input(input): Input,
) -> Result<Response, AppError> {
    render(&state, context, &session, &input, Open::Install).await
}

/// Why a token cannot be edited, if it cannot.
fn edit_refusal(token: &AccessToken) -> Option<&'static str> {
    if token.is_revoked() {
        return Some("Revoked tokens cannot be edited");
    }
    if token.source == TokenSource::Oauth {
        return Some("OAuth connections cannot be edited — revoke the connection instead");
    }
    None
}

/// `GET /tokens/{id}/edit`: the form of the edit dialog for the page's
/// script, and the page with that dialog open for a plain request.
pub async fn edit(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let view = list_view(&input);
    let token = match token_id(&id) {
        Some(id) => AccessToken::find(&**db, id).await?,
        None => None,
    };
    let reason = match &token {
        Some(token) => edit_refusal(token),
        None => Some(TOKEN_NOT_FOUND),
    };
    let (Some(token), None) = (token, reason) else {
        session.flash("error", reason.unwrap_or(TOKEN_NOT_FOUND));
        return Ok(navigate(&headers, &view.url("/tokens")));
    };

    if !is_fetch(&headers) {
        return render(&state, context, &session, &input, Open::Edit(token.id)).await;
    }
    let row = TokenRow {
        mcp_ids: mcp_ids_of(db, token.id).await?,
        token,
    };
    let mcps = load_mcps(db).await?;
    Ok(no_store(fragment(
        StatusCode::OK,
        edit_form(&context, &view, &row, &FormState::default(), &mcps),
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenPayload {
    name: String,
    scope_mode: ScopeMode,
    mcp_ids: Option<Vec<i64>>,
    expires_at: Option<DateTime<Utc>>,
}

/// `POST /tokens`
pub async fn store(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let index = forwarded_index(&uri);
    let payload: TokenPayload =
        match CREATE_ACCESS_TOKEN_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => {
                let form = FormState::new(&refusal(error)?, &input);
                let mcps = load_mcps(db).await?;
                return Ok(invalid_form(
                    &headers,
                    &session,
                    create_form(&context, &form, &mcps),
                    form.first_error().unwrap_or_default(),
                    "/tokens",
                ));
            }
        };
    let mcp_ids = payload.mcp_ids.unwrap_or_default();

    if payload.scope_mode == ScopeMode::Selected && !all_mcps_exist(db, &mcp_ids).await? {
        session.flash("error", UNKNOWN_MCPS);
        return Ok(navigate(&headers, &index));
    }

    let created = access_token::create(
        db,
        NewAccessToken {
            name: &payload.name,
            scope_mode: payload.scope_mode,
            mcp_ids: match payload.scope_mode {
                ScopeMode::Selected => &mcp_ids,
                ScopeMode::All => &[],
            },
            expires_at: payload.expires_at.map(Timestamp::from),
            created_by: user.id,
        },
    )
    .await?;

    session.flash(
        "success",
        "Access token created — copy it now, it will not be shown again",
    );
    session.flash(CREATED_PLAINTEXT_KEY, created.plaintext);
    Ok(navigate(&headers, &index))
}

/// `PUT /tokens/{id}`
pub async fn update(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let index = forwarded_index(&uri);
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(&headers, &index)
    };

    // Unlike the other actions, an id that is not one is a refused form.
    let id = match ACCESS_TOKEN_PARAMS_VALIDATOR.validate(&json!({ "id": id })) {
        Ok(route) => route.get("id").and_then(Value::as_i64),
        Err(error) => {
            let form = FormState::new(&error, &input);
            form.flash(&session);
            session.flash("error", form.first_error().unwrap_or_default());
            return Ok(if is_fetch(&headers) {
                navigate(&headers, &index)
            } else {
                redirect_back(&headers, "/tokens")
            });
        }
    };
    let token = match id {
        Some(id) => AccessToken::find(&**db, id).await?,
        None => None,
    };
    let Some(mut token) = token else {
        return Ok(refuse(TOKEN_NOT_FOUND));
    };
    if let Some(message) = edit_refusal(&token) {
        return Ok(refuse(message));
    }

    let payload: TokenPayload =
        match UPDATE_ACCESS_TOKEN_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => {
                let form = FormState::new(&refusal(error)?, &input);
                let row = TokenRow {
                    mcp_ids: mcp_ids_of(db, token.id).await?,
                    token,
                };
                let mcps = load_mcps(db).await?;
                return Ok(invalid_form(
                    &headers,
                    &session,
                    edit_form(&context, &list_view(&input), &row, &form, &mcps),
                    form.first_error().unwrap_or_default(),
                    "/tokens",
                ));
            }
        };
    let mcp_ids = payload.mcp_ids.unwrap_or_default();

    if payload.scope_mode == ScopeMode::Selected && !all_mcps_exist(db, &mcp_ids).await? {
        return Ok(refuse(UNKNOWN_MCPS));
    }

    access_token::update(
        db,
        &mut token,
        AccessTokenUpdate {
            name: &payload.name,
            scope_mode: payload.scope_mode,
            mcp_ids: &mcp_ids,
            expires_at: payload.expires_at.map(Timestamp::from),
        },
    )
    .await?;

    session.flash("success", "Token updated");
    Ok(navigate(&headers, &index))
}

/// `POST /tokens/{id}/revoke`
pub async fn revoke(
    State(state): State<AppState>,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let db = &state.core.db;
    let index = forwarded_index(&uri);
    let token = match token_id(&id) {
        Some(id) => AccessToken::find(&**db, id).await?,
        None => None,
    };
    let Some(mut token) = token else {
        session.flash("error", TOKEN_NOT_FOUND);
        return Ok(navigate(&headers, &index));
    };
    if token.is_revoked() {
        session.flash("error", "Token already revoked");
        return Ok(navigate(&headers, &index));
    }
    access_token::revoke(db, &mut token).await?;
    session.flash("success", "Token revoked");
    Ok(navigate(&headers, &index))
}

/// Why a deletion was refused as a whole.
enum CleanupRefusal {
    Missing,
    Active,
}

/// Delete these tokens, all of them or none: every one must exist, and be
/// revoked or expired.
async fn delete_inactive(db: &Db, ids: &[i64]) -> Result<Result<(), CleanupRefusal>, sqlx::Error> {
    let mut transaction = db.begin().await?;

    let mut selection =
        QueryBuilder::<Sqlite>::new("select count(*) from `access_tokens` where `id` in (");
    let mut list = selection.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    selection.push(")");
    let found: i64 = selection
        .build_query_scalar()
        .fetch_one(&mut *transaction)
        .await?;
    if usize::try_from(found).ok() != Some(ids.len()) {
        return Ok(Err(CleanupRefusal::Missing));
    }

    let now = Timestamp::now();
    let mut removal = QueryBuilder::<Sqlite>::new("delete from `access_tokens` where `id` in (");
    let mut list = removal.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    removal
        .push(
            ") and (`revoked_at` is not null \
             or (`source` = 'manual' and `expires_at` is not null and `expires_at` < ",
        )
        .push_bind(now)
        .push(
            ") or (`source` = 'oauth' and `revoked_at` is null \
             and ((`oauth_refresh_expires_at` is not null and `oauth_refresh_expires_at` < ",
        )
        .push_bind(now)
        .push(
            ") or (`oauth_refresh_expires_at` is null and `expires_at` is not null and `expires_at` < ",
        )
        .push_bind(now)
        .push("))))");
    let deleted = removal
        .build()
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    if usize::try_from(deleted).ok() != Some(ids.len()) {
        // Dropping the transaction undoes what was deleted.
        return Ok(Err(CleanupRefusal::Active));
    }

    transaction.commit().await?;
    Ok(Ok(()))
}

#[derive(Deserialize)]
struct Deletion {
    ids: Vec<i64>,
}

/// `DELETE /tokens`
pub async fn destroy(
    State(state): State<AppState>,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let index = forwarded_index(&uri);
    let Deletion { ids } =
        match DELETE_ACCESS_TOKENS_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(deletion) => deletion,
            Err(error) => {
                let form = FormState::new(&refusal(error)?, &input);
                session.flash("error", form.first_error().unwrap_or_default());
                return Ok(if is_fetch(&headers) {
                    navigate(&headers, &index)
                } else {
                    redirect_back(&headers, "/tokens")
                });
            }
        };
    let mut unique_ids: Vec<i64> = Vec::with_capacity(ids.len());
    for id in ids {
        if !unique_ids.contains(&id) {
            unique_ids.push(id);
        }
    }

    if let Err(refusal) = delete_inactive(&state.core.db, &unique_ids).await? {
        session.flash(
            "error",
            match refusal {
                CleanupRefusal::Missing => "One or more tokens no longer exist",
                CleanupRefusal::Active => {
                    "Active tokens must be revoked before they can be deleted"
                }
            },
        );
        return Ok(navigate(&headers, &index));
    }

    let suffix = if unique_ids.len() == 1 { "" } else { "s" };
    session.flash(
        "success",
        format!("{} token{suffix} deleted", unique_ids.len()),
    );
    Ok(navigate(&headers, &index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_the_view_over_and_drops_the_method_override() {
        let index = |uri: &str| forwarded_index(&uri.parse::<Uri>().unwrap());
        assert_eq!(index("/tokens"), "/tokens");
        assert_eq!(index("/tokens/4?_method=PUT"), "/tokens");
        assert_eq!(
            index("/tokens?_method=DELETE&status=inactive&page=2"),
            "/tokens?status=inactive&page=2"
        );
        assert_eq!(
            index("/tokens/4/revoke?status=active"),
            "/tokens?status=active"
        );
    }

    #[test]
    fn reads_the_view_a_query_string_asks_for() {
        let view = |input: Value| list_view(input.as_object().unwrap());
        assert_eq!(
            view(json!({})),
            ListView {
                status: StatusFilter::All,
                page: 1
            }
        );
        assert_eq!(
            view(json!({ "status": "inactive", "page": "3" })),
            ListView {
                status: StatusFilter::Inactive,
                page: 3
            }
        );
        for unreadable in [
            json!({ "status": "revoked", "page": "0" }),
            json!({ "status": ["active", "inactive"], "page": "1.5" }),
            json!({ "status": "", "page": "abc" }),
        ] {
            assert_eq!(
                view(unreadable),
                ListView {
                    status: StatusFilter::All,
                    page: 1
                }
            );
        }
    }
}
