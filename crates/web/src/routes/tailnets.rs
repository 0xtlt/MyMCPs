//! The Tailnets page: the Tailscale networks a team connects, for its HTTP
//! MCPs to reach devices that only answer inside them. For the owners and
//! administrators of the team, as the MCPs they are chosen on.

use std::collections::{BTreeMap, HashMap};

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use futures::future::join_all;
use http::HeaderMap;
use mymcps_core::models::{Mcp, Tailnet};
use mymcps_core::redaction::sanitize_diagnostic_with;
use mymcps_net::parse_http_url;
use mymcps_tailnet::{ConnectOptions, TailnetState};
use serde_json::Value;
use url::Url;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::forms::refusal;
use crate::input::Input;
use crate::redirect::with_query;
use crate::respond::{invalid_form, navigate, page};
use crate::routes::FeatureRoutes;
use crate::session::Session;
use crate::state::AppState;
use crate::validators::tailnet::{
    SignInMethod, TailnetPayload, create_tailnet_validator, tailnet_id,
};
use crate::views::shell::PageContext;
use crate::views::tailnets::{
    ConnectForm, ControlField, FORM_ERROR, TailnetRow, TailnetsPage, can_sign_in_again,
    connect_fragment, tailnets_page,
};

/// How many tailnets a team connects. Each one is a `tailscaled` process.
pub const MAX_TAILNETS_PER_TEAM: usize = 5;

const NOT_FOUND: &str = "Tailnet not found";
const TAILSCALE_OFF: &str = "Tailscale is off on this server: set TAILSCALED_BINARY to the tailscaled program and restart MyMCPs";

/// The longest a diagnostic of tailscaled is shown.
const DIAGNOSTIC_LIMIT: usize = 300;

/// How many names of MCPs the refusal to delete a tailnet lists.
const NAMES_SHOWN: usize = 5;

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        // As the MCPs that choose them: the owners and administrators of the
        // team. The sign-in link of a node is a way to put a device in the
        // tailnet, which no member needs.
        admin: Router::new()
            .route("/tailnets", get(index).post(store))
            .route("/tailnets/new", get(new))
            .route("/tailnets/{id}", delete(destroy))
            .route("/tailnets/{id}/sign-in", post(sign_in_again)),
        ..Default::default()
    }
}

/// The list, where every request of this page ends.
fn list(uri: &http::Uri) -> String {
    with_query("/tailnets", uri.query())
}

/// What the server knows of a tailnet, which starts its daemon if it is
/// not running: a server that restarted has none until something asks. A
/// tailnet whose control server the operator does not allow is not started.
async fn state_of(state: &AppState, tailnet: &Tailnet) -> Option<TailnetState> {
    let tailnets = state.upstream.tailnets()?;
    if let Some(message) = state.upstream.tailnet_control_refusal(tailnet) {
        return Some(TailnetState::Failed(message));
    }
    if let Err(error) = tailnets.ensure_running(tailnet.id).await {
        return Some(TailnetState::Failed(sanitize_diagnostic_with(
            &error.to_string(),
            DIAGNOSTIC_LIMIT,
            [],
        )));
    }
    Some(tailnets.state(tailnet.id).await)
}

/// The tailnets of a team as the list shows them.
async fn rows(state: &AppState, team_id: i64) -> Result<Vec<TailnetRow>, sqlx::Error> {
    let tailnets = Tailnet::of_team(&*state.core.db, team_id).await?;
    let used: Vec<(i64, String)> = sqlx::query_as(
        "select `tailnet_id`, `name` from `mcps` \
         where `team_id` = ? and `tailnet_id` is not null order by `name` collate nocase asc, `id` asc",
    )
    .bind(team_id)
    .fetch_all(&*state.core.db)
    .await?;
    let mut names: HashMap<i64, Vec<String>> = HashMap::new();
    for (tailnet_id, name) in used {
        names.entry(tailnet_id).or_default().push(name);
    }
    let states = join_all(tailnets.iter().map(|tailnet| state_of(state, tailnet))).await;
    Ok(tailnets
        .into_iter()
        .zip(states)
        .map(|(tailnet, state)| TailnetRow {
            mcps: names.remove(&tailnet.id).unwrap_or_default(),
            id: tailnet.id,
            name: tailnet.name,
            control_url: tailnet.control_url,
            state,
        })
        .collect())
}

async fn tailnets_response(
    state: &AppState,
    context: &PageContext,
    open: Option<&ConnectForm>,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let rows = rows(state, team.id()).await?;
    let markup = tailnets_page(
        context,
        &TailnetsPage {
            at_limit: rows.len() >= MAX_TAILNETS_PER_TEAM,
            limit: MAX_TAILNETS_PER_TEAM,
            enabled: state.upstream.tailnets().is_some(),
            rows: &rows,
            open,
            control: ControlField::of(&state.core.config.tailnet_control),
        },
    );
    Ok(page(markup))
}

/// `GET /tailnets`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
) -> Result<Response, AppError> {
    tailnets_response(&state, &context, None).await
}

/// `GET /tailnets/new`: the list with the dialog that connects a tailnet open.
pub async fn new(
    State(state): State<AppState>,
    context: PageContext,
) -> Result<Response, AppError> {
    let form = ConnectForm::default().with_control(control_field(&state));
    tailnets_response(&state, &context, Some(&form)).await
}

/// What the Connect dialog offers for the control server.
fn control_field(state: &AppState) -> ControlField {
    ControlField::of(&state.core.config.tailnet_control)
}

/// The name this server has in a tailnet: the same for the same name, and
/// a valid hostname for any name the form lets through.
pub fn hostname_for(name: &str) -> String {
    let slug = Mcp::slugify(name);
    let hostname: String = format!("mymcps-{slug}").chars().take(63).collect();
    hostname.trim_end_matches('-').to_string()
}

/// A refused form: the dialog again for the page's script, the first error
/// as a message for a plain post.
fn refused(
    context: &PageContext,
    session: &Session,
    headers: &HeaderMap,
    form: ConnectForm,
) -> Response {
    let message = form.first_error().unwrap_or_default().to_string();
    invalid_form(
        headers,
        session,
        connect_fragment(context, &form),
        &message,
        "/tailnets",
    )
}

/// A refusal that belongs to no field.
fn refused_form(
    state: &AppState,
    context: &PageContext,
    session: &Session,
    headers: &HeaderMap,
    input: &serde_json::Map<String, Value>,
    message: &str,
) -> Response {
    refused(
        context,
        session,
        headers,
        form_of(
            state,
            input,
            vec![(FORM_ERROR.to_string(), message.to_string())],
        ),
    )
}

/// The form as it was sent, without its key.
fn form_of(
    state: &AppState,
    input: &serde_json::Map<String, Value>,
    errors: Vec<(String, String)>,
) -> ConnectForm {
    let text = |field: &str| match input.get(field) {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    };
    let method = match text("method").as_str() {
        "key" => SignInMethod::Key,
        _ => SignInMethod::Link,
    };
    // A secret typed into the address is not sent back to the page.
    let control_url = text("controlUrl");
    let control_url = match Url::parse(&control_url) {
        Ok(url) if !url.username().is_empty() || url.password().is_some() => String::new(),
        _ => control_url,
    };
    ConnectForm::refused(&text("name"), method, &control_url, errors)
        .with_control(control_field(state))
}

/// What a person is told of tailscaled failing: its words, without the key
/// that was sent and without anything that looks like a secret.
fn describe(error: &mymcps_tailnet::Error, key: Option<&str>) -> String {
    sanitize_diagnostic_with(&error.to_string(), DIAGNOSTIC_LIMIT, key)
}

/// Take back a tailnet that could not be connected: its row, and whatever
/// its daemon wrote to its directory.
async fn abandon(state: &AppState, tailnet: &Tailnet) {
    state.upstream.forget_tailnet(tailnet.id);
    if let Some(tailnets) = state.upstream.tailnets()
        && let Err(error) = tailnets.remove(tailnet.id).await
    {
        tracing::warn!(
            tailnet = tailnet.id,
            "could not clean up a tailnet: {error}"
        );
    }
    if let Err(error) = tailnet.delete(&*state.core.db).await {
        tracing::warn!(tailnet = tailnet.id, "could not delete a tailnet: {error}");
    }
}

/// `POST /tailnets`
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
    let Some(tailnets) = state.upstream.tailnets() else {
        return Ok(refused_form(
            &state,
            &context,
            &session,
            &headers,
            &input,
            TAILSCALE_OFF,
        ));
    };
    let validator = create_tailnet_validator(&state.core.config.tailnet_control);
    let payload: TailnetPayload = match validator.validate_as(&Value::Object(input.clone())) {
        Ok(payload) => payload,
        Err(error) => {
            let error = refusal(error)?;
            let errors = error
                .messages
                .iter()
                .map(|message| (message.field.clone(), message.message.clone()))
                .collect();
            return Ok(refused(
                &context,
                &session,
                &headers,
                form_of(&state, &input, errors),
            ));
        }
    };
    // The key is only read for the method that uses it.
    let key = payload
        .auth_key
        .clone()
        .filter(|_| payload.method == SignInMethod::Key);
    let control_url = payload
        .control_url
        .as_deref()
        .and_then(|url| parse_http_url(url, "Control server URL").ok());
    let field_error = |field: &str, message: &str| {
        refused(
            &context,
            &session,
            &headers,
            form_of(
                &state,
                &input,
                vec![(field.to_string(), message.to_string())],
            ),
        )
    };

    let db = &*state.core.db;
    let existing = Tailnet::of_team(db, team.id()).await?;
    if existing.len() >= MAX_TAILNETS_PER_TEAM {
        return Ok(refused_form(
            &state,
            &context,
            &session,
            &headers,
            &input,
            &format!("A team can connect up to {MAX_TAILNETS_PER_TEAM} tailnets"),
        ));
    }
    if existing
        .iter()
        .any(|tailnet| tailnet.name.eq_ignore_ascii_case(&payload.name))
    {
        return Ok(field_error(
            "name",
            "This team already has a tailnet with this name",
        ));
    }

    let mut tailnet = Tailnet {
        team_id: team.id(),
        hostname: hostname_for(&payload.name),
        name: payload.name.clone(),
        control_url: control_url.as_ref().map(Url::to_string),
        created_by: user.id,
        ..Default::default()
    };
    if let Err(error) = tailnet.insert(db).await {
        if error
            .as_database_error()
            .is_some_and(|error| error.is_unique_violation())
        {
            return Ok(field_error(
                "name",
                "This team already has a tailnet with this name",
            ));
        }
        return Err(error.into());
    }
    // Two requests that both passed the count above could each add the last
    // one: the one that finds too many gives its row back, so a team never
    // holds more than its limit.
    if Tailnet::count_in_team(db, team.id()).await? > MAX_TAILNETS_PER_TEAM as i64 {
        abandon(&state, &tailnet).await;
        return Ok(refused_form(
            &state,
            &context,
            &session,
            &headers,
            &input,
            &format!("A team can connect up to {MAX_TAILNETS_PER_TEAM} tailnets"),
        ));
    }

    let connected = async {
        // No tombstone is cleared here: the id of a new row (AUTOINCREMENT)
        // is never one of a removed tailnet, and clearing the tombstone of
        // another id would let a concurrent deletion be undone.
        tailnets.ensure_running(tailnet.id).await?;
        tailnets
            .connect(
                tailnet.id,
                ConnectOptions {
                    hostname: tailnet.hostname.clone(),
                    control_url,
                    auth_key: key.clone(),
                },
            )
            .await
    }
    .await;
    if let Err(error) = connected {
        let message = describe(&error, key.as_deref());
        tracing::warn!(
            tailnet = tailnet.id,
            "could not connect a tailnet: {message}"
        );
        abandon(&state, &tailnet).await;
        return Ok(refused_form(
            &state,
            &context,
            &session,
            &headers,
            &input,
            &format!("Could not connect: {message}"),
        ));
    }

    session.flash(
        "success",
        match payload.method {
            SignInMethod::Key => "Tailnet connected",
            SignInMethod::Link => "Tailnet added: use its Sign in link to finish",
        },
    );
    Ok(navigate(&headers, &list(&uri)))
}

/// The names of MCPs as a sentence: "A, B and 3 more".
fn names_sentence(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(NAMES_SHOWN).map(String::as_str).collect();
    match names.len().saturating_sub(NAMES_SHOWN) {
        0 => match shown.as_slice() {
            [] => String::new(),
            [only] => (*only).to_string(),
            [init @ .., last] => format!("{} and {last}", init.join(", ")),
        },
        more => format!("{} and {more} more", shown.join(", ")),
    }
}

/// `DELETE /tailnets/{id}`: refused while an MCP uses the tailnet, and
/// otherwise the node leaves it, its daemon stops and its state is deleted.
pub async fn destroy(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let refuse = |message: &str| {
        session.flash("error", message);
        navigate(&headers, &list(&uri))
    };
    let db = &*state.core.db;
    let tailnet = match tailnet_id(&id) {
        Some(id) => Tailnet::find_in_team(db, team.id(), id).await?,
        None => None,
    };
    let Some(tailnet) = tailnet else {
        return Ok(refuse(NOT_FOUND));
    };

    let users: Vec<String> = tailnet
        .mcps(db)
        .await?
        .into_iter()
        .map(|mcp| mcp.name)
        .collect();
    if !users.is_empty() {
        return Ok(refuse(&format!(
            "{} is used by {}. Move {} to another network first.",
            tailnet.name,
            names_sentence(&users),
            if users.len() == 1 { "it" } else { "them" },
        )));
    }
    // The row goes first, and only while nothing uses it: an MCP that chose
    // this tailnet since the check above keeps it.
    let deleted = sqlx::query(
        "delete from `tailnets` where `id` = ? \
         and not exists (select 1 from `mcps` where `mcps`.`tailnet_id` = `tailnets`.`id`)",
    )
    .bind(tailnet.id)
    .execute(db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Ok(refuse(&format!(
            "{} is used by an MCP. Move it to another network first.",
            tailnet.name
        )));
    }
    // The connections an MCP kept through it end with it.
    state.upstream.forget_tailnet(tailnet.id);
    let left = remove_node(&state, tailnet.id, tailnet.control_url.as_deref()).await;
    session.flash(
        "success",
        with_logout_note("Tailnet deleted", usize::from(left == Node::LeftLoggedIn)),
    );
    Ok(navigate(&headers, &list(&uri)))
}

/// `POST /tailnets/{id}/sign-in`: the way back for a tailnet that is not
/// connected and cannot be signed in with what it holds (a link that expired,
/// a node key that expired, a node restored from a backup, a daemon that was
/// stopped). The node is asked for a new interactive login, and the page then
/// shows its new link. A plain post, answered by a redirect, as the other
/// actions of a row.
pub async fn sign_in_again(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let back = |kind: &str, message: &str| {
        session.flash(kind, message);
        navigate(&headers, &list(&uri))
    };
    let tailnet = match tailnet_id(&id) {
        Some(id) => Tailnet::find_in_team(&*state.core.db, team.id(), id).await?,
        None => None,
    };
    let Some(tailnet) = tailnet else {
        return Ok(back("error", NOT_FOUND));
    };
    let Some(tailnets) = state.upstream.tailnets() else {
        return Ok(back("error", TAILSCALE_OFF));
    };
    // Its control server may have stopped being allowed since it was connected.
    if let Some(message) = state.upstream.tailnet_control_refusal(&tailnet) {
        return Ok(back(
            "error",
            &format!("Could not sign in again: {message}"),
        ));
    }
    // A node that is connected, or that waits for an admin, has nothing to
    // sign in to: a new login would only put it out of the tailnet.
    if let Some(current) = state_of(&state, &tailnet).await
        && !can_sign_in_again(&current)
    {
        return Ok(back(
            "error",
            &format!("{} does not need to sign in again", tailnet.name),
        ));
    }

    let control_url = tailnet
        .control_url
        .as_deref()
        .and_then(|url| Url::parse(url).ok());
    let signed = async {
        tailnets.ensure_running(tailnet.id).await?;
        tailnets
            .sign_in_again(
                tailnet.id,
                ConnectOptions {
                    hostname: tailnet.hostname.clone(),
                    control_url,
                    auth_key: None,
                },
            )
            .await
    }
    .await;
    if let Err(error) = signed {
        let message = describe(&error, None);
        tracing::warn!(
            tailnet = tailnet.id,
            "could not sign a tailnet in again: {message}"
        );
        return Ok(back(
            "error",
            &format!("Could not sign in again: {message}"),
        ));
    }
    // The connections an MCP kept through the old login end with it.
    state.upstream.forget_tailnet(tailnet.id);
    Ok(back(
        "success",
        "Use the Sign in link of the tailnet to finish",
    ))
}

/// What became of the node of a tailnet that was taken out of this server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    /// Nothing is left of it here, and it logged out of its tailnet where it
    /// could (a control server that does not answer is only logged).
    Removed,
    /// Its state is deleted, but Tailscale is off on this server, so no daemon
    /// could log it out: the device stays in the admin console of its
    /// tailnet. Only said for a node that had a state here.
    LeftLoggedIn,
}

/// Takes the node of a tailnet out of this server. The node logs out of its
/// tailnet, unless the control server it was connected to is not allowed any
/// more (`TAILNET_CONTROL_URLS`): the logout needs a daemon, which that server
/// would steer, so the daemon is then never started and the state is only
/// deleted. `control_url` is the one the row had.
///
/// When Tailscale is off on this server (`TAILSCALED_BINARY=off`) there is no
/// daemon to ask either, and the state directory, with the key of the node,
/// is deleted all the same: the key of a device that was joined before the
/// feature was turned off must not stay behind a tailnet that is gone.
async fn remove_node(state: &AppState, id: i64, control_url: Option<&str>) -> Node {
    let Some(tailnets) = state.upstream.tailnets() else {
        return match mymcps_tailnet::delete_state(&state.core.config.data_dir, id).await {
            Ok(true) => Node::LeftLoggedIn,
            Ok(false) => Node::Removed,
            Err(error) => {
                tracing::warn!(
                    tailnet = id,
                    "could not delete the state of a tailnet: {error}"
                );
                Node::LeftLoggedIn
            }
        };
    };
    let result = match state.upstream.control_url_refusal(control_url) {
        Some(_) => tailnets.discard(id).await,
        None => tailnets.remove(id).await,
    };
    if let Err(error) = result {
        // The row is gone and its id is never given again: what is left is
        // a directory for the operator to delete.
        tracing::warn!(
            tailnet = id,
            "could not delete the state of a tailnet: {error}"
        );
    }
    Node::Removed
}

/// `message`, and what a person has to do when the device of `devices`
/// tailnets of a deletion could not be logged out remotely.
pub fn with_logout_note(message: &str, devices: usize) -> String {
    match devices {
        0 => message.to_owned(),
        1 => format!(
            "{message}. Tailscale is off on this server, so its device could not be logged out of the tailnet: remove it in the admin console of the tailnet."
        ),
        many => format!(
            "{message}. Tailscale is off on this server, so the devices of {many} tailnets could not be logged out: remove them in the admin console of each tailnet."
        ),
    }
}

/// Every tailnet of the instance with the control server it was connected to,
/// to tell afterwards which a deletion took with it, and what to do with the
/// state of each: the rows are gone by then.
///
/// A deletion reads it inside its own transaction, once the write lock is
/// held: a tailnet created before the deletion took the lock is in it, and
/// none can be created after, until the transaction ends. Read before the
/// transaction, a tailnet inserted in between would be cascaded away with a
/// daemon (and its node key) that nothing removes.
pub async fn all_control_urls<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Sqlite>,
) -> Result<BTreeMap<i64, Option<String>>, sqlx::Error> {
    let rows: Vec<(i64, Option<String>)> =
        sqlx::query_as("select `id`, `control_url` from `tailnets`")
            .fetch_all(executor)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Stop the daemons of the tailnets that were in `before` (read with
/// [`all_control_urls`] inside the deletion's transaction) and are no more:
/// the ones a deleted team, or the personal team of a deleted account, had.
/// A daemon that cannot be stopped is logged: the deletion is done. When
/// Tailscale is off the state of each is deleted all the same; the count that
/// comes back is the one of the devices that could not be logged out for
/// that reason, for [`with_logout_note`].
pub async fn discard_removed(state: &AppState, before: &BTreeMap<i64, Option<String>>) -> usize {
    let after = match all_control_urls(&*state.core.db).await {
        Ok(after) => after,
        Err(error) => {
            tracing::warn!("could not list the tailnets that remain: {error}");
            return 0;
        }
    };
    let mut left = 0;
    for (id, control_url) in before {
        if after.contains_key(id) {
            continue;
        }
        state.upstream.forget_tailnet(*id);
        if remove_node(state, *id, control_url.as_deref()).await == Node::LeftLoggedIn {
            left += 1;
        }
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_a_hostname_that_tailscale_takes() {
        assert_eq!(hostname_for("pro"), "mymcps-pro");
        assert_eq!(hostname_for("Home lab_2"), "mymcps-home-lab-2");
        assert_eq!(hostname_for("A.B"), "mymcps-a-b");
        let long = hostname_for(&"x".repeat(40));
        assert_eq!(long.len(), 47);
        for name in ["pro", "a - b", &"abc-".repeat(10)] {
            let hostname = hostname_for(name);
            assert!(
                hostname.len() <= 63 && !hostname.ends_with('-'),
                "{hostname}"
            );
        }
    }

    #[test]
    fn lists_the_mcps_that_keep_a_tailnet() {
        let names =
            |count: usize| -> Vec<String> { (1..=count).map(|n| format!("M{n}")).collect() };
        assert_eq!(names_sentence(&names(1)), "M1");
        assert_eq!(names_sentence(&names(2)), "M1 and M2");
        assert_eq!(names_sentence(&names(3)), "M1, M2 and M3");
        assert_eq!(names_sentence(&names(7)), "M1, M2, M3, M4, M5 and 2 more");
    }
}
