//! The MCPs page: the registry of upstream MCP servers, creating, editing
//! and deleting one, testing it, updating an npm one, and the OAuth
//! connection of the ones that sign in that way. (`mcps_controller.ts`)

use std::collections::HashSet;

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use http::header::ALLOW;
use http::{HeaderMap, Method, StatusCode, Uri};
use mymcps_builtin::{BuiltinMcpDefinition, BuiltinOauthConfig, BuiltinPasswordConfig};
use mymcps_core::Core;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, McpTransport};
use mymcps_core::redaction::{
    sanitize_diagnostic, sanitize_diagnostic_with, sanitize_mcp_diagnostic,
    sanitize_mcp_diagnostic_with,
};
use mymcps_core::secrets::{
    EnvironmentInput, decrypt_environment, environment_has_name, merge_environment,
};
use mymcps_net::parse_http_url;
use mymcps_upstream::Upstream;
use mymcps_upstream::validators::{OAUTH_CALLBACK_VALIDATOR, OAUTH_START_VALIDATOR};
use mymcps_vine::{FieldError, ValidationError};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sqlx::sqlite::SqliteExecutor;
use url::Url;

use crate::auth::CurrentUser;
use crate::error::{AppError, is_fetch};
use crate::forms::refusal;
use crate::input::{Input, parse_query};
use crate::mcp_templates::{McpTemplate, builtin_template, find_template};
use crate::redirect::{redirect_back, redirect_to, with_query};
use crate::respond::{fragment, invalid_form, navigate, page};
use crate::routes::FeatureRoutes;
use crate::session::Session;
use crate::state::AppState;
use crate::validators::mcp::{
    CREATE_MCP_VALIDATOR, MCP_ACTION_ORIGIN_VALIDATOR, MCP_TOGGLE_VALIDATOR, McpListQuery,
    McpPayload, UPDATE_MCP_VALIDATOR,
};
use crate::validators::route_params::record_id;
use crate::validators::session::FLASHED_RECORD_ID_VALIDATOR;
use crate::views::mcps::{
    CreateDialog, Dialog, EDIT_DIALOG_ID, EditDialog, FormValues, McpForm, McpView, McpsPage,
    PublicApp, ROWS_PER_PAGE, create_fragment, edit_fragment, mcps_page,
};
use crate::views::shell::PageContext;

const MAX_NPM_ENV_TOTAL_BYTES: usize = 64 * 1024;

const MAX_BUILTIN_ALIASES: usize = 20;

/// Flashed for the page that follows: the MCP whose edit dialog reopens.
const EDITING_KEY: &str = "editingMcpId";

const NOT_FOUND: &str = "MCP not found";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        signed_in: Router::new()
            .route("/mcps", get(index).post(store))
            .route("/mcps/new", get(new))
            .route("/mcps/oauth/callback", get(oauth_callback))
            .route("/mcps/{id}", get(show).put(update).delete(destroy))
            .route("/mcps/{id}/edit", get(edit))
            .route("/mcps/{id}/toggle", post(toggle))
            .route("/mcps/{id}/probe", post(probe))
            .route("/mcps/{id}/update", post(update_npm))
            .route("/mcps/{id}/oauth/start", get(oauth_start)),
        ..Default::default()
    }
}

/// Why a submitted form cannot be assigned to an MCP.
#[derive(Debug, thiserror::Error)]
pub enum AssignError {
    /// The form is refused. Nothing of it is saved.
    #[error(transparent)]
    Refused(#[from] ValidationError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

fn npm_env_refusal(field: String, message: impl Into<String>) -> ValidationError {
    ValidationError::single(field, "npmEnvironment", message)
}

fn is_set(column: &Option<String>) -> bool {
    column.as_deref().is_some_and(|value| !value.is_empty())
}

fn filled(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// The validator has checked the URL; this gives it its canonical form.
fn normalized_http_url(value: &str) -> Result<String, ValidationError> {
    parse_http_url(value, "MCP URL")
        .map(|url| url.to_string())
        .map_err(|error| ValidationError::single("httpUrl", "mcpEndpointUrl", error.to_string()))
}

/// `keeps_saved_values` is false when the MCP now runs another package: the
/// saved values were entered for the previous one and must be typed again.
fn assign_npm_environment(
    core: &Core,
    mcp: &mut Mcp,
    payload: &McpPayload,
    keeps_saved_values: bool,
) -> Result<(), ValidationError> {
    if payload.transport != McpTransport::Npm {
        mcp.npm_env = None;
        return Ok(());
    }

    let entries = payload.npm_env.clone().unwrap_or_default();
    let mut seen_names = HashSet::new();
    let saved_env = if keeps_saved_values {
        mcp.npm_env.clone()
    } else {
        None
    };

    for (index, entry) in entries.iter().enumerate() {
        if !seen_names.insert(entry.name.as_str()) {
            return Err(npm_env_refusal(
                format!("npmEnv.{index}.name"),
                "Environment variable names must be unique",
            ));
        }

        if entry.value.is_none() && !environment_has_name(saved_env.as_deref(), &entry.name) {
            return Err(npm_env_refusal(
                format!("npmEnv.{index}.value"),
                if environment_has_name(mcp.npm_env.as_deref(), &entry.name) {
                    "Enter this value again: saved values are not passed on to a different package"
                } else {
                    "A value is required for a new environment variable"
                },
            ));
        }
    }

    let inputs: Vec<EnvironmentInput> = entries
        .into_iter()
        .map(|entry| EnvironmentInput {
            name: entry.name,
            value: entry.value,
        })
        .collect();
    let next_value = merge_environment(&core.encryption, saved_env.as_deref(), &inputs);
    let environment = decrypt_environment(&core.encryption, next_value.as_deref())
        .map_err(|error| npm_env_refusal("npmEnv".into(), error.to_string()))?;

    let total_bytes: usize = environment
        .iter()
        .map(|(name, value)| name.len() + value.len())
        .sum();
    if total_bytes > MAX_NPM_ENV_TOTAL_BYTES {
        return Err(npm_env_refusal(
            "npmEnv".into(),
            "Environment variables must not exceed 64 KiB in total",
        ));
    }

    mcp.npm_env = next_value;
    Ok(())
}

fn apply_secrets(core: &Core, mcp: &mut Mcp, payload: &McpPayload) {
    if let Some(bearer) = filled(&payload.auth_bearer) {
        mcp.auth_bearer = core.encrypt_secret(Some(bearer));
    }
    if let Some(value) = filled(&payload.auth_header_value) {
        mcp.auth_header_value = core.encrypt_secret(Some(value));
    }
}

fn clear_oauth_connection(mcp: &mut Mcp) {
    mcp.oauth_authorize_url = None;
    mcp.oauth_token_url = None;
    mcp.oauth_scopes = None;
    mcp.oauth_client_id = None;
    mcp.oauth_client_secret = None;
    mcp.oauth_access_token = None;
    mcp.oauth_refresh_token = None;
    mcp.oauth_token_expires_at = None;
    mcp.oauth_issuer = None;
    mcp.oauth_resource = None;
    mcp.oauth_redirect_uri = None;
    mcp.oauth_client_auth_method = None;
    mcp.oauth_token_type = None;
    mcp.oauth_required = false;
}

/// Clear secrets that no longer apply to the selected auth type.
fn clear_unused_auth_secrets(mcp: &mut Mcp) {
    if mcp.auth_type != McpAuthType::Bearer {
        mcp.auth_bearer = None;
    }
    if mcp.auth_type != McpAuthType::Header {
        mcp.auth_header_name = None;
        mcp.auth_header_value = None;
    }
    if mcp.auth_type != McpAuthType::Auto {
        clear_oauth_connection(mcp);
    }
}

/// Save the API application credentials of a built-in MCP. Tokens belong to
/// the application that issued them, so new credentials disconnect the account.
/// Returns what is wrong with them, and then saves nothing.
fn assign_oauth_application(
    core: &Core,
    mcp: &mut Mcp,
    payload: &McpPayload,
    name: &str,
    oauth: &BuiltinOauthConfig,
) -> Vec<FieldError> {
    let client_id = payload.oauth_client_id.clone().unwrap_or_default();
    let saved_secret = core.decrypt_secret(mcp.oauth_client_secret.as_deref());
    let client_secret = filled(&payload.oauth_client_secret)
        .map(str::to_string)
        .or_else(|| saved_secret.clone());

    // Report both fields at once: they are copied from the same provider page.
    let mut failures = Vec::new();
    if client_id.is_empty() {
        failures.push(FieldError::new(
            "oauthClientId",
            "required",
            format!("Enter the Client ID of your {name} API application"),
        ));
    } else if oauth
        .client_id_pattern
        .as_ref()
        .is_some_and(|pattern| !pattern.is_match(&client_id))
    {
        failures.push(FieldError::new(
            "oauthClientId",
            "regex",
            oauth
                .client_id_hint
                .map_or_else(|| format!("{name} Client ID is not valid"), str::to_string),
        ));
    }
    let Some(client_secret) = client_secret else {
        failures.push(FieldError::new(
            "oauthClientSecret",
            "required",
            format!("Enter the Client Secret of your {name} API application"),
        ));
        return failures;
    };
    if !failures.is_empty() {
        return failures;
    }

    if mcp.oauth_client_id.as_deref() != Some(client_id.as_str())
        || saved_secret.as_deref() != Some(client_secret.as_str())
    {
        clear_oauth_connection(mcp);
    }
    mcp.oauth_client_id = Some(client_id);
    mcp.oauth_client_secret = core.encrypt_secret(Some(&client_secret));
    Vec::new()
}

/// Save the account name and app password of a built-in MCP, what agents may
/// do with them, and which other addresses of the account they may act as. A
/// blank password keeps the saved one, which is never sent back to the browser.
/// Returns what is wrong with them, and then saves nothing.
fn assign_password_sign_in(
    core: &Core,
    mcp: &mut Mcp,
    payload: &McpPayload,
    name: &str,
    sign_in: &BuiltinPasswordConfig,
) -> Vec<FieldError> {
    let username = payload.builtin_username.clone().unwrap_or_default();
    let password = filled(&payload.builtin_password)
        .map(str::to_string)
        .or_else(|| core.decrypt_secret(mcp.builtin_password.as_deref()));

    let mut failures = Vec::new();
    if !sign_in.username_pattern.is_match(&username) {
        failures.push(FieldError::new(
            "builtinUsername",
            if username.is_empty() {
                "required"
            } else {
                "regex"
            },
            sign_in.username_hint,
        ));
    }
    if !password
        .as_deref()
        .is_some_and(|password| sign_in.password_pattern.is_match(password))
    {
        failures.push(FieldError::new(
            "builtinPassword",
            if password.is_some() {
                "regex"
            } else {
                "required"
            },
            sign_in.password_hint,
        ));
    }
    let requested = payload.builtin_permissions.clone().unwrap_or_default();
    let unknown = requested
        .iter()
        .find(|name| !sign_in.permissions.contains(&name.as_str()));
    if unknown.is_some() || requested.is_empty() {
        failures.push(match unknown {
            None => FieldError::new(
                "builtinPermissions",
                "required",
                "Allow at least one permission",
            ),
            Some(unknown) => FieldError::new(
                "builtinPermissions",
                "enum",
                format!("{name} has no \"{unknown}\" permission"),
            ),
        });
    }
    let mut seen = HashSet::from([username.to_lowercase()]);
    let aliases: Vec<String> = payload
        .builtin_aliases
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|alias| seen.insert(alias.to_lowercase()))
        .collect();
    if aliases.len() > MAX_BUILTIN_ALIASES
        || aliases
            .iter()
            .any(|alias| !sign_in.username_pattern.is_match(alias))
    {
        failures.push(FieldError::new(
            "builtinAliases",
            "regex",
            sign_in.alias_hint,
        ));
    }
    let Some(password) = password.filter(|_| failures.is_empty()) else {
        return failures;
    };

    mcp.builtin_username = Some(username);
    mcp.builtin_password = core.encrypt_secret(Some(&password));
    mcp.builtin_permissions = Some(
        sign_in
            .permissions
            .iter()
            .filter(|name| requested.iter().any(|requested| requested == *name))
            .copied()
            .collect::<Vec<_>>()
            .join(" "),
    );
    mcp.builtin_aliases = (!aliases.is_empty()).then(|| aliases.join(" "));
    Vec::new()
}

/// Save what a built-in MCP needs beyond its sign-in, such as the account it
/// acts through. Returns what is wrong, and then saves nothing.
fn assign_builtin_settings(
    core: &Core,
    mcp: &mut Mcp,
    payload: &McpPayload,
    definition: &BuiltinMcpDefinition,
) -> Vec<FieldError> {
    let mut failures = Vec::new();
    let mut entries = Vec::new();
    for field in definition.settings() {
        let entered = payload
            .builtin_settings
            .as_ref()
            .and_then(|settings| settings.get(field.key))
            .and_then(|value| value.as_deref())
            .map(mymcps_vine::js::trim)
            .unwrap_or_default();
        let value = match (entered.is_empty(), field.normalize) {
            (true, _) => String::new(),
            (false, Some(normalize)) => normalize(entered),
            (false, None) => entered.to_string(),
        };
        let name = format!("builtinSettings.{}", field.key);
        if value.is_empty() {
            if field.required {
                failures.push(FieldError::new(name, "required", field.hint.clone()));
            }
        } else if !field.pattern.is_match(&value) {
            failures.push(FieldError::new(name, "regex", field.hint.clone()));
        } else {
            entries.push(EnvironmentInput {
                name: field.key.to_string(),
                value: Some(value),
            });
        }
    }
    if !failures.is_empty() {
        return failures;
    }

    // Encrypted like the rest of what is entered in this dialog.
    mcp.builtin_settings = merge_environment(&core.encryption, None, &entries);
    Vec::new()
}

fn assign_builtin_credentials(
    upstream: &Upstream,
    mcp: &mut Mcp,
    payload: &McpPayload,
) -> Result<(), ValidationError> {
    let core = upstream.core();
    let definition = if payload.transport == McpTransport::Builtin {
        upstream.builtin_mcp(payload.builtin_key.as_deref())
    } else {
        None
    };
    if definition.is_none_or(|definition| definition.password().is_none()) {
        mcp.builtin_username = None;
        mcp.builtin_password = None;
        mcp.builtin_permissions = None;
        mcp.builtin_aliases = None;
    }
    let Some(definition) = definition else {
        mcp.builtin_settings = None;
        return Ok(());
    };

    // Everything wrong is reported at once: it all comes from the same setup.
    let mut failures = match (definition.oauth(), definition.password()) {
        (Some(oauth), _) => assign_oauth_application(core, mcp, payload, definition.name(), oauth),
        (None, Some(sign_in)) => {
            assign_password_sign_in(core, mcp, payload, definition.name(), sign_in)
        }
        (None, None) => Vec::new(),
    };
    failures.extend(assign_builtin_settings(core, mcp, payload, definition));
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ValidationError::new(failures))
    }
}

/// Strava reports the granted scopes as `scope=read,activity:read_all`, and
/// the query parser splits comma-separated values into an array. The value is
/// only a hint that is sanitized before use, so it is read outside the
/// validator and can never fail the callback.
fn granted_scope_from_callback(value: Option<&Value>) -> Option<String> {
    let scopes: Vec<&str> = match value {
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(scope)) => vec![scope.as_str()],
        _ => Vec::new(),
    };
    (!scopes.is_empty()).then(|| scopes.join(" "))
}

fn http_origin(http_url: Option<&str>) -> Option<String> {
    let http_url = http_url.filter(|url| !url.is_empty())?;
    Url::parse(http_url)
        .ok()
        .map(|url| url.origin().ascii_serialization())
}

async fn unique_slug<'e, E>(
    db: E,
    name: &str,
    exclude_id: Option<i64>,
) -> Result<String, sqlx::Error>
where
    E: SqliteExecutor<'e> + Copy,
{
    let base = Mcp::slugify(name);
    let mut candidate = base.clone();
    let mut suffix = 2;
    loop {
        let existing: Option<i64> = match exclude_id.filter(|id| *id != 0) {
            Some(exclude_id) => {
                sqlx::query_scalar(
                    "select `id` from `mcps` where `slug` = ? and not `id` = ? limit 1",
                )
                .bind(&candidate)
                .bind(exclude_id)
                .fetch_optional(db)
                .await?
            }
            None => {
                sqlx::query_scalar("select `id` from `mcps` where `slug` = ? limit 1")
                    .bind(&candidate)
                    .fetch_optional(db)
                    .await?
            }
        };
        if existing.is_none() {
            return Ok(candidate);
        }
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
}

/// Assign what a validated form holds to the row of an MCP, new or saved.
/// Nothing is written to the database: the caller saves the row, or drops
/// it when the form is refused. (`assignMcpFromPayload`)
pub async fn assign_mcp_from_payload(
    upstream: &Upstream,
    mcp: &mut Mcp,
    payload: &McpPayload,
    exclude_id: Option<i64>,
) -> Result<(), AssignError> {
    let core = upstream.core();
    let transport = payload.transport;
    let next_http_url = match transport {
        McpTransport::Http => Some(normalized_http_url(
            payload.http_url.as_deref().unwrap_or_default(),
        )?),
        _ => None,
    };
    let next_builtin_key = payload
        .builtin_key
        .clone()
        .filter(|_| transport == McpTransport::Builtin);
    let next_npm_package = payload
        .npm_package
        .clone()
        .filter(|_| transport == McpTransport::Npm);
    // A row that was never saved has no transport yet.
    let transport_changed = !mcp.is_persisted() || mcp.transport != transport;
    let oauth_server_changed =
        transport_changed || mcp.http_url != next_http_url || mcp.builtin_key != next_builtin_key;
    if oauth_server_changed {
        clear_oauth_connection(mcp);
    }

    // Saved secrets are write-only and were entered for one destination. They
    // must not follow the MCP to another origin or package, where the probe
    // after saving would hand them over. A new path or version keeps them.
    let credential_target_changed = transport_changed
        || http_origin(mcp.http_url.as_deref()) != http_origin(next_http_url.as_deref())
        || mcp.npm_package != next_npm_package;
    if credential_target_changed {
        mcp.auth_bearer = None;
        mcp.auth_header_value = None;
    }

    mcp.name = payload.name.clone();
    mcp.slug = unique_slug(&*core.db, &payload.name, exclude_id).await?;
    mcp.description = payload
        .description
        .clone()
        .filter(|description| !description.is_empty());
    mcp.transport = transport;
    mcp.http_url = next_http_url;
    mcp.builtin_key = next_builtin_key;
    mcp.builtin_write_enabled =
        transport == McpTransport::Builtin && payload.builtin_write_enabled.unwrap_or(false);
    mcp.npm_package = next_npm_package;
    mcp.npm_version = payload
        .npm_version
        .clone()
        .filter(|version| transport == McpTransport::Npm && !version.is_empty());
    mcp.set_npm_args_list(&match transport {
        McpTransport::Npm => payload.npm_args.clone().unwrap_or_default(),
        _ => Vec::new(),
    });
    assign_npm_environment(core, mcp, payload, !credential_target_changed)?;
    // Built-in MCPs sign in the one way their provider supports.
    mcp.auth_type = match transport {
        McpTransport::Builtin => McpAuthType::Auto,
        _ => payload.auth_type,
    };
    mcp.auth_header_name = payload
        .auth_header_name
        .clone()
        .filter(|_| mcp.auth_type == McpAuthType::Header);
    mcp.enabled = payload.enabled.unwrap_or(false);
    clear_unused_auth_secrets(mcp);
    apply_secrets(core, mcp, payload);
    assign_builtin_credentials(upstream, mcp, payload)?;
    Ok(())
}

/// Whether saving left something for the admin to do in the dialog: connect an
/// account, or correct a sign-in the provider just rejected.
fn needs_attention(upstream: &Upstream, mcp: &Mcp) -> bool {
    mcp.oauth_required
        || (mcp.status == McpStatus::Error
            && upstream
                .builtin_mcp(mcp.builtin_key.as_deref())
                .is_some_and(|definition| definition.password().is_some()))
}

/// Files left in a sandbox are not worth failing a save or a delete over; a
/// leftover directory is reported for the operator to remove.
async fn discard_sandbox(state: &AppState, mcp_id: i64) {
    if let Err(error) = state.upstream.remove_mcp_sandbox(mcp_id).await {
        tracing::warn!(
            mcp_id,
            error = %sanitize_diagnostic(&error.to_string()),
            "Could not delete the sandbox directory of an npm MCP"
        );
    }
}

/// Files agents uploaded for a built-in MCP expire by themselves, so neither are these.
async fn discard_uploads(state: &AppState, mcp_id: i64) {
    if let Err(error) = state
        .upstream
        .builtin_env()
        .uploads
        .remove_all(mcp_id)
        .await
    {
        tracing::warn!(
            mcp_id,
            error = %sanitize_diagnostic(&error.to_string()),
            "Could not delete the uploaded files of a built-in MCP"
        );
    }
}

/// The MCP a route names, or `None` when its `{id}` is not an id or matches none.
async fn find_mcp(state: &AppState, id: &str) -> Result<Option<Mcp>, sqlx::Error> {
    match record_id(id) {
        Some(id) => Mcp::find(&*state.core.db, id).await,
        None => Ok(None),
    }
}

/// The registry, where every request of this page ends, with the query
/// string of the request being answered as the Node app's redirects carried it.
fn registry(uri: &Uri) -> String {
    with_query("/mcps", uri.query())
}

fn not_found(session: &Session, headers: &HeaderMap, uri: &Uri) -> Response {
    session.flash("error", NOT_FOUND);
    navigate(headers, &registry(uri))
}

/// Whether the page script asks for the content of one of the page's
/// dialogs, to put it in place itself.
fn wants_fragment(headers: &HeaderMap, dialog_id: &str) -> bool {
    is_fetch(headers)
        && headers
            .get("x-fragment")
            .and_then(|value| value.to_str().ok())
            == Some(dialog_id)
}

/// The rows of the environment editor as a list, whatever index the page
/// script gave them. A form names them `npmEnv[7][name]`, and the body
/// parser only reads small indexes as a list: a row added after others were
/// removed would otherwise turn the whole editor into something else.
fn list_environment_rows(input: &mut Map<String, Value>) {
    let Some(Value::Object(rows)) = input.get("npmEnv") else {
        return;
    };
    let mut indexed: Vec<(u32, Value)> = Vec::with_capacity(rows.len());
    for (key, row) in rows {
        match key.parse::<u32>() {
            Ok(index) if index.to_string() == *key => indexed.push((index, row.clone())),
            _ => return,
        }
    }
    indexed.sort_by_key(|(index, _)| *index);
    let rows = indexed.into_iter().map(|(_, row)| row).collect();
    input.insert("npmEnv".to_string(), Value::Array(rows));
}

/// What the page behind a dialog opens on.
enum Open {
    Nothing,
    Gallery,
    Create(Option<&'static McpTemplate>),
    Edit(i64),
}

fn keeps(query: &McpListQuery, mcp: &McpView) -> bool {
    query.q.as_deref().is_none_or(|search| mcp.matches(search))
        && query.status.is_none_or(|status| mcp.status == status)
        && query
            .transport
            .is_none_or(|transport| mcp.transport == transport)
        && query
            .auth
            .as_deref()
            .is_none_or(|auth| mcp.auth_label() == auth)
}

/// The page of the registry, with one of its dialogs open or none.
async fn registry_page(
    state: &AppState,
    context: &PageContext,
    mut query: McpListQuery,
    open: Open,
) -> Result<Response, AppError> {
    let mcps: Vec<Mcp> = sqlx::query_as("select * from `mcps` order by `name` asc, `id` asc")
        .fetch_all(&*state.core.db)
        .await?;
    let tool_counts = state.known_tool_counts();
    let views: Vec<McpView> = mcps
        .iter()
        .map(|mcp| McpView::of(&state.upstream, mcp, tool_counts.get(&mcp.id).copied()))
        .collect();

    let matching: Vec<&McpView> = views.iter().filter(|mcp| keeps(&query, mcp)).collect();
    let pages = matching.len().div_ceil(ROWS_PER_PAGE).max(1);
    let current = (query.page.unwrap_or(1).max(1) as usize).min(pages);
    query.page = u32::try_from(current).ok();
    let rows: Vec<McpView> = matching
        .iter()
        .skip((current - 1) * ROWS_PER_PAGE)
        .take(ROWS_PER_PAGE)
        .map(|mcp| (*mcp).clone())
        .collect();
    let matching_enabled = matching.iter().filter(|mcp| mcp.enabled).count();

    let public_app = PublicApp::of(&state.core.config);
    let render = |dialog: Dialog| {
        page(mcps_page(
            context,
            &McpsPage {
                query: &query,
                rows: &rows,
                matching: matching.len(),
                matching_enabled,
                registered: views.len(),
                dialog,
            },
        ))
    };
    Ok(match open {
        Open::Nothing => render(Dialog::None),
        Open::Gallery => render(Dialog::Gallery),
        Open::Create(template) => {
            let form = McpForm::new(template.map(FormValues::from_template).unwrap_or_default());
            render(Dialog::Create(&CreateDialog {
                template,
                form: &form,
                public_app: public_app.as_ref(),
                query: &query,
            }))
        }
        Open::Edit(id) => match views.iter().find(|mcp| mcp.id == id) {
            Some(mcp) => {
                let form = McpForm::new(FormValues::from_mcp(mcp));
                render(Dialog::Edit(&EditDialog {
                    mcp,
                    form: &form,
                    public_app: public_app.as_ref(),
                }))
            }
            None => render(Dialog::None),
        },
    })
}

/// `GET /mcps`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    Input(input): Input,
) -> Result<Response, AppError> {
    let editing = session
        .flashed(EDITING_KEY)
        .and_then(|id| FLASHED_RECORD_ID_VALIDATOR.validate(&id).ok())
        .and_then(|id| id.as_i64());
    let open = editing.map_or(Open::Nothing, Open::Edit);
    registry_page(&state, &context, McpListQuery::read(&input), open).await
}

/// `GET /mcps/new`: the registry with the gallery open, or with the create
/// form of `?template=` (`custom` for a blank one).
pub async fn new(
    State(state): State<AppState>,
    context: PageContext,
    Input(input): Input,
) -> Result<Response, AppError> {
    let query = McpListQuery::read(&input);
    let open = match query.template.as_deref() {
        Some("custom") => Open::Create(None),
        Some(id) => {
            find_template(id).map_or(Open::Gallery, |template| Open::Create(Some(template)))
        }
        None => Open::Gallery,
    };
    registry_page(&state, &context, query, open).await
}

/// `GET /mcps/{id}/edit`: the registry with the edit dialog of an MCP open.
/// The page script asks for the content of the dialog alone.
pub async fn edit(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(mcp) = find_mcp(&state, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };
    if !wants_fragment(&headers, EDIT_DIALOG_ID) {
        return registry_page(
            &state,
            &context,
            McpListQuery::read(&input),
            Open::Edit(mcp.id),
        )
        .await;
    }

    let tool_count = state.known_tool_counts().get(&mcp.id).copied();
    let view = McpView::of(&state.upstream, &mcp, tool_count);
    let form = McpForm::new(FormValues::from_mcp(&view));
    Ok(fragment(
        StatusCode::OK,
        edit_fragment(
            &context,
            &EditDialog {
                mcp: &view,
                form: &form,
                public_app: PublicApp::of(&state.core.config).as_ref(),
            },
        ),
    ))
}

/// `GET /mcps/{id}`
pub async fn show(
    State(state): State<AppState>,
    session: Session,
    OriginalUri(uri): OriginalUri,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    match find_mcp(&state, &id).await? {
        Some(mcp) => session.flash(EDITING_KEY, mcp.id),
        None => session.flash("error", NOT_FOUND),
    }
    Ok(redirect_to(&registry(&uri)))
}

/// A refused create form: the page script gets the dialog again with what
/// is wrong, a plain post goes back with the first error.
fn refused_create(
    state: &AppState,
    context: &PageContext,
    session: &Session,
    headers: &HeaderMap,
    input: &Map<String, Value>,
    error: &ValidationError,
) -> Response {
    let form = McpForm::refused(FormValues::from_input(input), error);
    // The form says which template it started from. A built-in MCP has one
    // of its own whatever the form says.
    let template = match form.values.transport {
        McpTransport::Builtin => builtin_template(&form.values.builtin_key),
        _ => input
            .get("template")
            .and_then(Value::as_str)
            .and_then(find_template),
    };
    let dialog = create_fragment(
        context,
        &CreateDialog {
            template,
            form: &form,
            public_app: PublicApp::of(&state.core.config).as_ref(),
            query: &McpListQuery::default(),
        },
    );
    invalid_form(
        headers,
        session,
        dialog,
        form.first_error().unwrap_or_default(),
        "/mcps",
    )
}

/// `POST /mcps`
pub async fn store(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(mut input): Input,
) -> Result<Response, AppError> {
    list_environment_rows(&mut input);
    let payload: McpPayload = match CREATE_MCP_VALIDATOR.validate_as(&Value::Object(input.clone()))
    {
        Ok(payload) => payload,
        Err(error) => {
            let error = refusal(error)?;
            return Ok(refused_create(
                &state, &context, &session, &headers, &input, &error,
            ));
        }
    };

    let mut mcp = Mcp {
        status: McpStatus::Draft,
        created_by: user.id,
        ..Default::default()
    };
    match assign_mcp_from_payload(&state.upstream, &mut mcp, &payload, None).await {
        Ok(()) => {}
        Err(AssignError::Refused(error)) => {
            return Ok(refused_create(
                &state, &context, &session, &headers, &input, &error,
            ));
        }
        Err(AssignError::Database(error)) => return Err(error.into()),
    }
    mcp.insert(&*state.core.db).await?;

    state
        .upstream
        .test_and_update_status(&mut mcp)
        .await
        .map_err(AppError::internal)?;
    if needs_attention(&state.upstream, &mcp) {
        session.flash(EDITING_KEY, mcp.id);
    }
    session.flash("success", "MCP created");
    Ok(navigate(&headers, &registry(&uri)))
}

/// A refused edit form, answered like a refused create form. The dialog is
/// drawn from the row as it is saved: nothing of the refused form was.
fn refused_update(
    state: &AppState,
    context: &PageContext,
    session: &Session,
    headers: &HeaderMap,
    mcp: &Mcp,
    input: &Map<String, Value>,
    error: &ValidationError,
) -> Response {
    let tool_count = state.known_tool_counts().get(&mcp.id).copied();
    let view = McpView::of(&state.upstream, mcp, tool_count);
    let form = McpForm::refused(FormValues::from_input(input), error);
    let dialog = edit_fragment(
        context,
        &EditDialog {
            mcp: &view,
            form: &form,
            public_app: PublicApp::of(&state.core.config).as_ref(),
        },
    );
    invalid_form(
        headers,
        session,
        dialog,
        form.first_error().unwrap_or_default(),
        "/mcps",
    )
}

/// `PUT /mcps/{id}`
pub async fn update(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(mut input): Input,
) -> Result<Response, AppError> {
    let Some(saved) = find_mcp(&state, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };

    list_environment_rows(&mut input);
    let payload: McpPayload = match UPDATE_MCP_VALIDATOR.validate_as(&Value::Object(input.clone()))
    {
        Ok(payload) => payload,
        Err(error) => {
            let error = refusal(error)?;
            return Ok(refused_update(
                &state, &context, &session, &headers, &saved, &input, &error,
            ));
        }
    };
    let previous_npm_package = saved
        .npm_package
        .clone()
        .filter(|package| saved.transport == McpTransport::Npm && !package.is_empty());
    let mut mcp = saved.clone();
    match assign_mcp_from_payload(&state.upstream, &mut mcp, &payload, Some(saved.id)).await {
        Ok(()) => {}
        Err(AssignError::Refused(error)) => {
            return Ok(refused_update(
                &state, &context, &session, &headers, &saved, &input, &error,
            ));
        }
        Err(AssignError::Database(error)) => return Err(error.into()),
    }
    mcp.save(&*state.core.db).await?;
    // Another package must not start in what the previous one left behind.
    if previous_npm_package.is_some() && previous_npm_package != mcp.npm_package {
        discard_sandbox(&state, mcp.id).await;
    }

    state
        .upstream
        .test_and_update_status(&mut mcp)
        .await
        .map_err(AppError::internal)?;
    if needs_attention(&state.upstream, &mcp) {
        session.flash(EDITING_KEY, mcp.id);
    }
    session.flash("success", "MCP updated");
    Ok(navigate(&headers, &registry(&uri)))
}

/// `DELETE /mcps/{id}`
pub async fn destroy(
    State(state): State<AppState>,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Some(mcp) = find_mcp(&state, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };
    mcp.delete(&*state.core.db).await?;
    discard_sandbox(&state, mcp.id).await;
    discard_uploads(&state, mcp.id).await;
    session.flash("success", "MCP deleted");
    Ok(navigate(&headers, &registry(&uri)))
}

/// `POST /mcps/{id}/toggle`: the switch of a row. The Node app only changed
/// this from the edit form.
pub async fn toggle(
    State(state): State<AppState>,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(mut mcp) = find_mcp(&state, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };
    let asked = MCP_TOGGLE_VALIDATOR
        .validate(&Value::Object(input))
        .ok()
        .and_then(|toggle| toggle.get("enabled").and_then(Value::as_bool));
    mcp.enabled = asked.unwrap_or(!mcp.enabled);
    mcp.save(&*state.core.db).await?;
    session.flash(
        "success",
        if mcp.enabled {
            "MCP enabled"
        } else {
            "MCP disabled"
        },
    );
    Ok(back_to_list(&headers, &uri))
}

/// Back to the list as the person left it, for what a row of it asked.
fn back_to_list(headers: &HeaderMap, uri: &Uri) -> Response {
    if is_fetch(headers) {
        navigate(headers, &registry(uri))
    } else {
        redirect_back(headers, &registry(uri))
    }
}

/// Whether a row of the list sent the request, and not the edit dialog.
fn from_list(input: &Map<String, Value>) -> bool {
    MCP_ACTION_ORIGIN_VALIDATOR
        .validate(&Value::Object(input.clone()))
        .ok()
        .is_some_and(|origin| origin.get("from").and_then(Value::as_str) == Some("list"))
}

/// Answer a request that tested or updated an MCP. The edit dialog, where
/// the Node app had these actions, opens again on the result. A row of the
/// list reads the result in the list.
fn tested(
    session: &Session,
    headers: &HeaderMap,
    uri: &Uri,
    input: &Map<String, Value>,
    mcp: &Mcp,
) -> Response {
    if from_list(input) {
        return back_to_list(headers, uri);
    }
    session.flash(EDITING_KEY, mcp.id);
    navigate(headers, &registry(uri))
}

/// `POST /mcps/{id}/probe`
pub async fn probe(
    State(state): State<AppState>,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(mut mcp) = find_mcp(&state, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };
    state
        .upstream
        .test_and_update_status(&mut mcp)
        .await
        .map_err(AppError::internal)?;
    if mcp.status == McpStatus::Ready {
        session.flash("success", "Connection OK");
    } else {
        session.flash(
            "error",
            filled(&mcp.last_error).unwrap_or("Connection failed"),
        );
    }
    Ok(tested(&session, &headers, &uri, &input, &mcp))
}

/// `POST /mcps/{id}/update`
pub async fn update_npm(
    State(state): State<AppState>,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(mut mcp) = find_mcp(&state, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };

    match state.upstream.update_mcp_to_latest(&mut mcp).await {
        Ok(()) if mcp.status == McpStatus::Ready => {
            session.flash("success", "MCP updated to latest");
        }
        Ok(()) => session.flash("error", filled(&mcp.last_error).unwrap_or("Update failed")),
        Err(error) if error.is_npm_update_error() => session.flash("error", error.to_string()),
        Err(error) => session.flash(
            "error",
            sanitize_mcp_diagnostic(&state.core.encryption, &error.to_string(), &mcp),
        ),
    }
    Ok(tested(&session, &headers, &uri, &input, &mcp))
}

/// `GET /mcps/{id}/oauth/start`
///
/// Starting a flow registers a client with the provider and replaces the
/// saved OAuth configuration, on a route that has to stay a GET navigation.
pub async fn oauth_start(
    State(state): State<AppState>,
    session: Session,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    // The router also sends HEAD requests to GET routes.
    if method != Method::GET {
        return Ok((StatusCode::METHOD_NOT_ALLOWED, [(ALLOW, "GET")]).into_response());
    }
    // Browsers say where a request comes from. Another site must not be able
    // to start, and so reset, an authorization by linking to or embedding this URL.
    let site: Vec<String> = headers
        .get_all("sec-fetch-site")
        .iter()
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .collect();
    let mut request_headers = Map::new();
    if !site.is_empty() {
        request_headers.insert("sec-fetch-site".to_string(), Value::String(site.join(", ")));
    }
    if OAUTH_START_VALIDATOR
        .validate(&json!({ "headers": request_headers }))
        .is_err()
    {
        session.flash("error", "Start the OAuth connection from the MCPs page");
        return Ok(redirect_to("/mcps"));
    }

    let Some(mut mcp) = find_mcp(&state, &id).await? else {
        session.flash("error", NOT_FOUND);
        return Ok(redirect_to(&registry(&uri)));
    };
    let uses_oauth = match mcp.transport {
        McpTransport::Builtin => state
            .upstream
            .builtin_mcp(mcp.builtin_key.as_deref())
            .is_some_and(|definition| definition.oauth().is_some()),
        _ => {
            mcp.auth_type == McpAuthType::Auto
                && (mcp.oauth_required || is_set(&mcp.oauth_access_token))
        }
    };
    if !uses_oauth {
        session.flash("error", "This MCP does not require OAuth authorization");
        session.flash(EDITING_KEY, mcp.id);
        return Ok(redirect_to(&registry(&uri)));
    }

    match state.upstream.start_oauth_flow(&session, &mut mcp).await {
        // The query string of this request is not carried over: it would
        // append whatever followed this URL to the provider's authorization request.
        Ok(authorization_url) => Ok(redirect_to(&authorization_url)),
        Err(error) => {
            session.flash(
                "error",
                sanitize_mcp_diagnostic(&state.core.encryption, &error.to_string(), &mcp),
            );
            session.flash(EDITING_KEY, mcp.id);
            Ok(redirect_to(&registry(&uri)))
        }
    }
}

/// The query string as the Node app's parser read it: a value with commas
/// is a list, whatever the commas stand for once decoded.
fn comma_separated_query(query: Option<&str>) -> Map<String, Value> {
    let pairs: Vec<String> = query
        .unwrap_or_default()
        .split('&')
        .flat_map(|pair| match pair.split_once('=') {
            Some((key, value)) if value.contains(',') => value
                .split(',')
                .map(|part| format!("{key}={part}"))
                .collect(),
            _ => vec![pair.to_string()],
        })
        .collect();
    parse_query(&pairs.join("&"))
}

/// Query params on `/mcps/oauth/callback` from the authorization server.
#[derive(Deserialize)]
struct OauthCallback {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// `GET /mcps/oauth/callback`
pub async fn oauth_callback(
    State(state): State<AppState>,
    session: Session,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, AppError> {
    let db = &*state.core.db;
    let encryption = &state.core.encryption;
    let upstream = &state.upstream;
    // Nothing of the callback is carried over to the registry.
    let to_registry = || redirect_to("/mcps");
    let invalid = || {
        session.flash("error", "Invalid OAuth callback");
        to_registry()
    };

    let query = comma_separated_query(uri.query());
    let callback: OauthCallback =
        match OAUTH_CALLBACK_VALIDATOR.validate_as(&Value::Object(query.clone())) {
            Ok(callback) => callback,
            Err(error) => {
                // A validation error would send the browser back without a word.
                refusal(error)?;
                return Ok(invalid());
            }
        };
    let code = callback.code.filter(|code| !code.is_empty());
    let callback_state = callback.state.filter(|state| !state.is_empty());
    let oauth = upstream.read_oauth_session(&session, callback_state.as_deref());
    upstream.clear_oauth_session(&session, callback_state.as_deref());

    if let Some(oauth_error) = callback.error.filter(|error| !error.is_empty()) {
        // Only the answer to an authorization this session started is the
        // word of a provider. Any page can send a browser here with an
        // `error` of its own, which the registry would show as its message.
        let Some(oauth) = &oauth else {
            return Ok(invalid());
        };
        let mcp = Mcp::find(db, oauth.mcp_id).await?;
        let message = format!("OAuth error: {oauth_error}");
        let callback_credentials = [code.as_deref(), callback_state.as_deref()]
            .into_iter()
            .flatten();
        session.flash(
            "error",
            match &mcp {
                Some(mcp) => sanitize_mcp_diagnostic_with(
                    encryption,
                    &message,
                    mcp,
                    500,
                    callback_credentials,
                ),
                None => sanitize_diagnostic_with(&message, 500, callback_credentials),
            },
        );
        session.flash(EDITING_KEY, oauth.mcp_id);
        return Ok(to_registry());
    }

    let (Some(oauth), Some(code), Some(callback_state)) = (oauth, code, callback_state) else {
        return Ok(invalid());
    };
    if callback_state != oauth.state {
        return Ok(invalid());
    }

    let Some(mut mcp) = Mcp::find(db, oauth.mcp_id).await? else {
        session.flash("error", NOT_FOUND);
        return Ok(to_registry());
    };

    let granted_scope = granted_scope_from_callback(query.get("scope"));
    let connected = match upstream
        .exchange_authorization_code(&mut mcp, &oauth, &code, granted_scope.as_deref())
        .await
    {
        Ok(()) => upstream.test_and_update_status(&mut mcp).await,
        Err(error) => Err(error),
    };
    match connected {
        Ok(()) if mcp.status == McpStatus::Ready => session.flash("success", "OAuth connected"),
        Ok(()) => session.flash(
            "error",
            filled(&mcp.last_error).unwrap_or("OAuth connected, but the connection test failed"),
        ),
        Err(error) => {
            let reason = sanitize_mcp_diagnostic_with(
                encryption,
                &error.to_string(),
                &mcp,
                500,
                [code.as_str(), callback_state.as_str()],
            );
            mcp.status = McpStatus::Error;
            mcp.last_error = Some(reason.clone());
            mcp.save(db).await?;
            session.flash("error", reason);
        }
    }

    session.flash(EDITING_KEY, mcp.id);
    Ok(to_registry())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_scopes_a_provider_reports_in_its_callback() {
        let query = comma_separated_query(Some(
            "state=abc&code=strava-code&scope=read,activity:read_all",
        ));
        assert_eq!(
            Value::Object(query.clone()),
            json!({ "state": "abc", "code": "strava-code", "scope": ["read", "activity:read_all"] })
        );
        assert_eq!(
            granted_scope_from_callback(query.get("scope")).as_deref(),
            Some("read activity:read_all")
        );

        // A comma that was encoded is part of the value, and a single scope is itself.
        let query = comma_separated_query(Some("state=one%2Ctwo&scope=read"));
        assert_eq!(query["state"], "one,two");
        assert_eq!(
            granted_scope_from_callback(query.get("scope")).as_deref(),
            Some("read")
        );
        assert_eq!(granted_scope_from_callback(None), None);
        assert_eq!(granted_scope_from_callback(Some(&json!({ "a": 1 }))), None);
        assert_eq!(
            granted_scope_from_callback(Some(&json!(["read", 3, "write"]))).as_deref(),
            Some("read write")
        );

        // A comma makes the validator see a list, which is not a state.
        let query = comma_separated_query(Some("state=one,two&code=strava-code"));
        assert!(
            OAUTH_CALLBACK_VALIDATOR
                .validate(&Value::Object(query))
                .is_err()
        );
        assert!(comma_separated_query(None).is_empty());
    }

    #[test]
    fn lists_the_rows_of_the_environment_editor_whatever_their_index() {
        let rows = |input: Value| {
            let mut input = input.as_object().cloned().unwrap();
            list_environment_rows(&mut input);
            input.get("npmEnv").cloned()
        };
        // Rows the body parser already listed are left as they are.
        let listed = json!([{ "name": "A", "value": "1" }]);
        assert_eq!(rows(json!({ "npmEnv": listed })), Some(listed));
        assert_eq!(rows(json!({ "name": "x" })), None);

        // An index past what the parser lists keeps the rows in their order.
        assert_eq!(
            rows(json!({ "npmEnv": {
                "25": { "name": "B", "value": "2" },
                "3": { "name": "A", "value": null },
            } })),
            Some(json!([{ "name": "A", "value": null }, { "name": "B", "value": "2" }]))
        );
        // Anything else is for the validator to refuse.
        let named = json!({ "first": { "name": "A" } });
        assert_eq!(rows(json!({ "npmEnv": named })), Some(named));
        let padded = json!({ "07": { "name": "A" } });
        assert_eq!(rows(json!({ "npmEnv": padded })), Some(padded));
    }

    #[test]
    fn compares_the_origins_of_two_endpoints() {
        assert_eq!(
            http_origin(Some("https://user:pass@old.example/mcp?x=1")).as_deref(),
            Some("https://old.example")
        );
        assert_eq!(
            http_origin(Some("https://old.example:8443/mcp")).as_deref(),
            Some("https://old.example:8443")
        );
        assert_eq!(
            http_origin(Some("http://old.example:80/mcp")).as_deref(),
            Some("http://old.example")
        );
        assert_eq!(http_origin(Some("")), None);
        assert_eq!(http_origin(Some("not a url")), None);
        assert_eq!(http_origin(None), None);
    }
}
