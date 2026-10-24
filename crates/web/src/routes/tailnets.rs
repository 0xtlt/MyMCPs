//! The Tailnets page: the Tailscale networks a team connects, for its HTTP
//! MCPs to reach devices that only answer inside them. For the owners and
//! administrators of the team, as the MCPs they are chosen on.

use std::collections::{BTreeSet, HashMap};

use axum::Router;
use axum::extract::{OriginalUri, Path, State};
use axum::response::Response;
use axum::routing::{delete, get};
use futures::future::join_all;
use http::HeaderMap;
use mymcps_core::models::{Mcp, Tailnet};
use mymcps_core::redaction::sanitize_diagnostic_with;
use mymcps_net::parse_http_url;
use mymcps_tailnet::{ConnectOptions, TailnetState, Tailnets};
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
    CREATE_TAILNET_VALIDATOR, SignInMethod, TailnetPayload, tailnet_id,
};
use crate::views::shell::PageContext;
use crate::views::tailnets::{
    ConnectForm, FORM_ERROR, TailnetRow, TailnetsPage, connect_fragment, tailnets_page,
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
            .route("/tailnets/{id}", delete(destroy)),
        ..Default::default()
    }
}

/// The list, where every request of this page ends.
fn list(uri: &http::Uri) -> String {
    with_query("/tailnets", uri.query())
}

/// What the server knows of a tailnet, which starts its daemon if it is
/// not running: a server that restarted has none until something asks.
async fn state_of(tailnets: Option<&Tailnets>, id: i64) -> Option<TailnetState> {
    let tailnets = tailnets?;
    if let Err(error) = tailnets.ensure_running(id).await {
        return Some(TailnetState::Failed(sanitize_diagnostic_with(
            &error.to_string(),
            DIAGNOSTIC_LIMIT,
            [],
        )));
    }
    Some(tailnets.state(id).await)
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
    let running = state.upstream.tailnets();
    let states = join_all(tailnets.iter().map(|tailnet| state_of(running, tailnet.id))).await;
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
    tailnets_response(&state, &context, Some(&ConnectForm::default())).await
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
        form_of(input, vec![(FORM_ERROR.to_string(), message.to_string())]),
    )
}

/// The form as it was sent, without its key.
fn form_of(input: &serde_json::Map<String, Value>, errors: Vec<(String, String)>) -> ConnectForm {
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
            &context,
            &session,
            &headers,
            &input,
            TAILSCALE_OFF,
        ));
    };
    let payload: TailnetPayload =
        match CREATE_TAILNET_VALIDATOR.validate_as(&Value::Object(input.clone())) {
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
                    form_of(&input, errors),
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
            form_of(&input, vec![(field.to_string(), message.to_string())]),
        )
    };

    let db = &*state.core.db;
    let existing = Tailnet::of_team(db, team.id()).await?;
    if existing.len() >= MAX_TAILNETS_PER_TEAM {
        return Ok(refused_form(
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
            &context,
            &session,
            &headers,
            &input,
            &format!("A team can connect up to {MAX_TAILNETS_PER_TEAM} tailnets"),
        ));
    }

    let connected = async {
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
    if let Some(tailnets) = state.upstream.tailnets()
        && let Err(error) = tailnets.remove(tailnet.id).await
    {
        // The row is gone and its id is never given again: what is left is
        // a directory for the operator to delete.
        tracing::warn!(
            tailnet = tailnet.id,
            "could not delete the state of a tailnet: {error}"
        );
    }
    session.flash("success", "Tailnet deleted");
    Ok(navigate(&headers, &list(&uri)))
}

/// The ids of every tailnet of the instance, to tell afterwards which a
/// deletion took with it.
pub async fn all_ids(state: &AppState) -> Result<BTreeSet<i64>, sqlx::Error> {
    let ids: Vec<i64> = sqlx::query_scalar("select `id` from `tailnets`")
        .fetch_all(&*state.core.db)
        .await?;
    Ok(ids.into_iter().collect())
}

/// Stop the daemons of the tailnets that were in `before` and are no more:
/// the ones a deleted team, or the personal team of a deleted account, had.
/// A daemon that cannot be stopped is logged: the deletion is done.
pub async fn discard_removed(state: &AppState, before: &BTreeSet<i64>) {
    let Some(tailnets) = state.upstream.tailnets() else {
        return;
    };
    let after = match all_ids(state).await {
        Ok(after) => after,
        Err(error) => {
            tracing::warn!("could not list the tailnets that remain: {error}");
            return;
        }
    };
    for id in before.difference(&after) {
        state.upstream.forget_tailnet(*id);
        if let Err(error) = tailnets.remove(*id).await {
            tracing::warn!(
                tailnet = id,
                "could not delete the state of a tailnet: {error}"
            );
        }
    }
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
