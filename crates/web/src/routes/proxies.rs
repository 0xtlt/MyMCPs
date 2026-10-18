//! The Proxies page: the Tailscale nodes a team runs for its HTTP MCPs.
//! Adding one saves a row and starts its node; the node logs in with the
//! saved auth key, or asks for a login link the page shows. A proxy that
//! MCPs use cannot be deleted.

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use http::{HeaderMap, StatusCode};
use mymcps_core::models::{MAX_PROXIES_PER_TEAM, Proxy, ProxyKind};
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::auth::{CurrentTeam, CurrentUser};
use crate::error::AppError;
use crate::forms::refusal;
use crate::input::Input;
use crate::proxy_nodes::Availability;
use crate::redirect::with_query;
use crate::respond::{fragment, invalid_form, navigate, page};
use crate::routes::FeatureRoutes;
use crate::routes::mcps::wants_fragment;
use crate::session::Session;
use crate::state::AppState;
use crate::validators::proxy::{CREATE_PROXY_VALIDATOR, ProxyPayload, UPDATE_PROXY_VALIDATOR};
use crate::validators::route_params::record_id;
use crate::views::proxies::{
    EDIT_DIALOG_ID, EditDialog, FormValues, ProxiesPage, ProxyForm, ProxyView, create_fragment,
    edit_fragment, proxies_page,
};
use crate::views::shell::PageContext;

const NOT_FOUND: &str = "Proxy not found";

/// Why a proxy cannot be added now.
enum CreationRefusal {
    /// `TAILSCALE_MAX_PROXIES` is 0.
    Off,
    /// The server cannot run `tailscaled`.
    Unavailable(String),
    /// The team has as many as it may.
    TeamFull,
    /// The instance runs as many as `TAILSCALE_MAX_PROXIES` allows.
    InstanceFull(usize),
}

impl CreationRefusal {
    /// The line under the disabled button. The banner above the list says
    /// the rest when Tailscale cannot run.
    fn help(&self) -> String {
        match self {
            Self::Off => "Tailscale proxies are off on this server.".to_string(),
            Self::Unavailable(_) => "Tailscale proxies cannot start on this server.".to_string(),
            Self::TeamFull => format!("A team has at most {MAX_PROXIES_PER_TEAM} proxies."),
            Self::InstanceFull(max) => {
                format!("This server runs at most {max} proxies: raise TAILSCALE_MAX_PROXIES.")
            }
        }
    }

    /// What a request to add one is told.
    fn message(&self) -> String {
        match self {
            Self::Off => {
                "Tailscale proxies are off: a super admin turns them on with TAILSCALE_MAX_PROXIES"
                    .to_string()
            }
            Self::Unavailable(reason) => format!("Tailscale proxies cannot start: {reason}"),
            Self::TeamFull => format!("A team has at most {MAX_PROXIES_PER_TEAM} proxies"),
            Self::InstanceFull(max) => format!(
                "This server runs at most {max} proxies: a super admin raises TAILSCALE_MAX_PROXIES"
            ),
        }
    }
}

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        // The nodes hold keys to a private network: owners and admins only.
        admin: Router::new()
            .route("/proxies", get(index).post(store))
            .route("/proxies/{id}", put(update).delete(destroy))
            .route("/proxies/{id}/edit", get(edit))
            .route("/proxies/{id}/reconnect", post(reconnect)),
        ..Default::default()
    }
}

/// Where every request of this page ends, with the query string of the
/// request being answered (without the method override).
fn list_path(uri: &http::Uri) -> String {
    with_query("/proxies", uri.query())
}

/// Why a proxy cannot be added to this team now, if it cannot. Read on the
/// connection that will insert it, so that two requests cannot both take
/// the last place.
async fn creation_refusal(
    state: &AppState,
    connection: &mut SqliteConnection,
    team_id: i64,
) -> Result<Option<CreationRefusal>, sqlx::Error> {
    match state.proxy_nodes.availability() {
        Availability::Available { .. } => {}
        Availability::Disabled => return Ok(Some(CreationRefusal::Off)),
        Availability::Unavailable { reason } => {
            return Ok(Some(CreationRefusal::Unavailable(reason)));
        }
    }
    let in_team = Proxy::count_for_team(&mut *connection, team_id).await?;
    if usize::try_from(in_team).is_ok_and(|count| count >= MAX_PROXIES_PER_TEAM) {
        return Ok(Some(CreationRefusal::TeamFull));
    }
    let max = state.core.config.tailscale_max_proxies;
    let on_instance = Proxy::count(&mut *connection).await?;
    if usize::try_from(on_instance).is_ok_and(|count| count >= max) {
        return Ok(Some(CreationRefusal::InstanceFull(max)));
    }
    Ok(None)
}

/// A proxy as the page shows it: the row, what its node is doing, and how
/// many MCPs go through it.
async fn proxy_view(state: &AppState, proxy: &Proxy) -> Result<ProxyView, sqlx::Error> {
    Ok(ProxyView {
        id: proxy.id,
        name: proxy.name.clone(),
        hostname: proxy.tailscale_hostname.clone(),
        has_key: proxy
            .tailscale_auth_key
            .as_deref()
            .is_some_and(|key| !key.is_empty()),
        status: state.proxy_nodes.status(proxy.id),
        used_by: proxy.mcps(&*state.core.db).await?.len(),
    })
}

/// The proxy a route names, or `None` when its `{id}` is not an id, matches
/// none, or names the proxy of another team: all three get the same answer.
async fn find_proxy(
    state: &AppState,
    team: &CurrentTeam,
    id: &str,
) -> Result<Option<Proxy>, sqlx::Error> {
    let proxy = match record_id(id) {
        Some(id) => Proxy::find(&*state.core.db, id).await?,
        None => None,
    };
    Ok(proxy.filter(|proxy| proxy.team_id == team.id()))
}

fn not_found(session: &Session, headers: &HeaderMap, uri: &http::Uri) -> Response {
    session.flash("error", NOT_FOUND);
    navigate(headers, &list_path(uri))
}

/// The page, with the edit dialog of a proxy open or none.
async fn list_page(
    state: &AppState,
    context: &PageContext,
    editing: Option<i64>,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let proxies = Proxy::for_team(&*state.core.db, team.id()).await?;
    let mut rows = Vec::with_capacity(proxies.len());
    for proxy in &proxies {
        rows.push(proxy_view(state, proxy).await?);
    }

    let refusal = {
        let mut connection = state.core.db.acquire().await?;
        creation_refusal(state, &mut connection, team.id())
            .await?
            .map(|refusal| refusal.help())
    };
    let availability = state.proxy_nodes.availability();
    let create_form = ProxyForm::new(FormValues::blank());
    let edit_form = editing
        .and_then(|id| rows.iter().find(|row| row.id == id))
        .map(|proxy| (proxy, ProxyForm::new(FormValues::of_proxy(proxy))));
    let editing = edit_form
        .as_ref()
        .map(|(proxy, form)| EditDialog { proxy, form });

    Ok(page(proxies_page(
        context,
        &ProxiesPage {
            rows: &rows,
            availability: &availability,
            creation_refusal: refusal.as_deref(),
            create_form: &create_form,
            editing: editing.as_ref(),
        },
    )))
}

/// `GET /proxies`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
) -> Result<Response, AppError> {
    list_page(&state, &context, None).await
}

/// `GET /proxies/{id}/edit`: the page with the edit dialog of a proxy
/// open. The page script asks for the content of the dialog alone.
pub async fn edit(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let Some(proxy) = find_proxy(&state, &team, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };
    if !wants_fragment(&headers, EDIT_DIALOG_ID) {
        return list_page(&state, &context, Some(proxy.id)).await;
    }

    let view = proxy_view(&state, &proxy).await?;
    let form = ProxyForm::new(FormValues::of_proxy(&view));
    Ok(fragment(
        StatusCode::OK,
        edit_fragment(
            &context,
            &EditDialog {
                proxy: &view,
                form: &form,
            },
        ),
    ))
}

/// `POST /proxies`
pub async fn store(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let values = FormValues::of_input(&input);
    let refused = |form: ProxyForm| {
        invalid_form(
            &headers,
            &session,
            create_fragment(&context, &form),
            form.first_error().unwrap_or_default(),
            "/proxies",
        )
    };

    let payload: ProxyPayload =
        match CREATE_PROXY_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => return Ok(refused(ProxyForm::refused(values, &refusal(error)?))),
        };

    // The count and the insert happen under one write lock.
    let mut transaction = state.core.db.begin().await?;
    if let Some(reason) = creation_refusal(&state, &mut transaction, team.id()).await? {
        transaction.rollback().await?;
        return Ok(refused(ProxyForm::refused_as_a_whole(
            values,
            reason.message(),
        )));
    }
    let mut proxy = Proxy {
        team_id: team.id(),
        name: payload.name,
        kind: ProxyKind::Tailscale,
        tailscale_hostname: payload.tailscale_hostname,
        tailscale_auth_key: state
            .core
            .encrypt_secret(payload.tailscale_auth_key.as_deref()),
        created_by: Some(user.id),
        ..Default::default()
    };
    proxy.insert(&mut *transaction).await?;
    transaction.commit().await?;

    // The row stays when the node does not start: the page shows its status.
    match state.proxy_nodes.ensure(state.node_config(&proxy)).await {
        Ok(()) => session.flash("success", "Proxy added"),
        Err(error) => {
            tracing::warn!(proxy = proxy.id, %error, "The Tailscale node of a new proxy did not start");
            session.flash(
                "error",
                format!("Proxy added, but its Tailscale node did not start: {error}"),
            );
        }
    }
    Ok(navigate(&headers, &list_path(&uri)))
}

/// A refused edit form. The dialog is drawn from the row as it is saved:
/// nothing of the refused form was.
async fn refused_update(
    state: &AppState,
    context: &PageContext,
    session: &Session,
    headers: &HeaderMap,
    saved: &Proxy,
    form: &ProxyForm,
) -> Result<Response, AppError> {
    let view = proxy_view(state, saved).await?;
    Ok(invalid_form(
        headers,
        session,
        edit_fragment(context, &EditDialog { proxy: &view, form }),
        form.first_error().unwrap_or_default(),
        "/proxies",
    ))
}

/// `PUT /proxies/{id}`
pub async fn update(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let Some(saved) = find_proxy(&state, &team, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };

    let payload: ProxyPayload =
        match UPDATE_PROXY_VALIDATOR.validate_as(&Value::Object(input.clone())) {
            Ok(payload) => payload,
            Err(error) => {
                let form = ProxyForm::refused(FormValues::of_input(&input), &refusal(error)?);
                return refused_update(&state, &context, &session, &headers, &saved, &form).await;
            }
        };

    let mut proxy = saved.clone();
    proxy.name = payload.name;
    proxy.tailscale_hostname = payload.tailscale_hostname;
    // A blank key keeps the saved one.
    if let Some(key) = payload.tailscale_auth_key {
        proxy.tailscale_auth_key = state.core.encrypt_secret(Some(&key));
    }
    proxy.save(&*state.core.db).await?;

    // While Tailscale cannot run, the page says so: saving is not an error.
    if matches!(
        state.proxy_nodes.availability(),
        Availability::Available { .. }
    ) {
        match state.proxy_nodes.ensure(state.node_config(&proxy)).await {
            Ok(()) => session.flash("success", "Proxy updated"),
            Err(error) => {
                tracing::warn!(proxy = proxy.id, %error, "The Tailscale node of a proxy was not updated");
                session.flash(
                    "error",
                    format!("Proxy updated, but its Tailscale node did not accept it: {error}"),
                );
            }
        }
    } else {
        session.flash("success", "Proxy updated");
    }
    Ok(navigate(&headers, &list_path(&uri)))
}

/// `POST /proxies/{id}/reconnect`: restart the node and log it in again.
pub async fn reconnect(
    State(state): State<AppState>,
    session: Session,
    team: CurrentTeam,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Some(proxy) = find_proxy(&state, &team, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };
    match state.proxy_nodes.availability() {
        Availability::Available { .. } => {}
        Availability::Disabled => {
            session.flash("error", CreationRefusal::Off.message());
            return Ok(navigate(&headers, &list_path(&uri)));
        }
        Availability::Unavailable { reason } => {
            session.flash("error", CreationRefusal::Unavailable(reason).message());
            return Ok(navigate(&headers, &list_path(&uri)));
        }
    }
    match state.proxy_nodes.reconnect(state.node_config(&proxy)).await {
        Ok(()) => session.flash("success", format!("Reconnecting {}", proxy.name)),
        Err(error) => {
            tracing::warn!(proxy = proxy.id, %error, "The Tailscale node of a proxy did not restart");
            session.flash("error", format!("{} did not restart: {error}", proxy.name));
        }
    }
    Ok(navigate(&headers, &list_path(&uri)))
}

/// What a request to delete a proxy that MCPs use is told.
fn in_use(mcps: &[(i64, String)]) -> String {
    let names: Vec<&str> = mcps.iter().map(|(_, name)| name.as_str()).collect();
    match names.as_slice() {
        [name] => format!(
            "Proxy is used by 1 MCP: {name}. Set its Network to Direct or another proxy first."
        ),
        names => format!(
            "Proxy is used by {} MCPs: {}. Set their Network to Direct or another proxy first.",
            names.len(),
            names.join(", ")
        ),
    }
}

/// `DELETE /proxies/{id}`
///
/// Refused while an MCP goes through the proxy: it would otherwise be sent
/// direct, or fail. The row goes first, under the write lock that also
/// reads the MCPs; the node is logged out and removed once it is gone.
pub async fn destroy(
    State(state): State<AppState>,
    session: Session,
    team: CurrentTeam,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Some(proxy) = find_proxy(&state, &team, &id).await? else {
        return Ok(not_found(&session, &headers, &uri));
    };

    let mut transaction = state.core.db.begin().await?;
    let mcps = proxy.mcps(&mut *transaction).await?;
    if !mcps.is_empty() {
        transaction.rollback().await?;
        session.flash("error", in_use(&mcps));
        return Ok(navigate(&headers, &list_path(&uri)));
    }
    proxy.delete(&mut *transaction).await?;
    transaction.commit().await?;

    // The row is gone either way: a node that cannot be removed is deleted
    // with the other state nothing owns at the next start.
    if let Err(error) = state.proxy_nodes.remove(proxy.id).await {
        tracing::error!(proxy = proxy.id, %error, "Could not remove the Tailscale node of a deleted proxy");
    }
    session.flash("success", "Proxy deleted");
    Ok(navigate(&headers, &list_path(&uri)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_mcps_that_keep_a_proxy() {
        assert_eq!(
            in_use(&[(1, "Notion".into())]),
            "Proxy is used by 1 MCP: Notion. Set its Network to Direct or another proxy first."
        );
        assert_eq!(
            in_use(&[(1, "Notion".into()), (2, "T3".into())]),
            "Proxy is used by 2 MCPs: Notion, T3. Set their Network to Direct or another proxy first."
        );
    }
}
