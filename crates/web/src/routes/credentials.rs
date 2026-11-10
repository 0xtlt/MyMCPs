//! The Credentials page: the OAuth clients a team saves once and shares
//! between its built-in MCPs of a provider. For the owners and
//! administrators of the team, as the MCPs that choose them.

use std::collections::HashMap;

use axum::Router;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, put};
use http::HeaderMap;
use mymcps_core::models::ProviderCredential;
use serde_json::{Map, Value};

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::forms::refusal;
use crate::input::Input;
use crate::respond::{invalid_form, navigate, page};
use crate::routes::FeatureRoutes;
use crate::session::Session;
use crate::state::AppState;
use crate::validators::credential::{
    CREATE_CREDENTIAL_VALIDATOR, CredentialPayload, UPDATE_CREDENTIAL_VALIDATOR, credential_id,
};
use crate::views::credentials::{
    CredentialForm, CredentialRow, CredentialsPage, FORM_ERROR, credential_fragment,
    credentials_page,
};
use crate::views::mcps::PublicApp;
use crate::views::shell::PageContext;

const NOT_FOUND: &str = "OAuth client not found";
const NAME_TAKEN: &str = "This team already has a credential with this name";
const CLIENT_TAKEN: &str = "This Client ID is already saved for this team";

/// How many credentials a team saves.
pub const MAX_CREDENTIALS_PER_TEAM: usize = 50;

/// How many names of MCPs the refusal to delete a credential lists.
const NAMES_SHOWN: usize = 5;

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        // As the MCPs that choose them: the owners and administrators of the team.
        admin: Router::new()
            .route("/credentials", get(index).post(store))
            .route("/credentials/new", get(new))
            .route("/credentials/{id}", put(update).delete(destroy))
            .route("/credentials/{id}/edit", get(edit)),
        ..Default::default()
    }
}

/// The credentials of a team as the list shows them.
async fn rows(state: &AppState, team_id: i64) -> Result<Vec<CredentialRow>, sqlx::Error> {
    let db = &*state.core.db;
    let credentials = ProviderCredential::of_team(db, team_id, None).await?;
    let used: Vec<(i64, String)> = sqlx::query_as(
        "select `provider_credential_id`, `name` from `mcps` \
         where `team_id` = ? and `provider_credential_id` is not null order by `name` collate nocase asc, `id` asc",
    )
    .bind(team_id)
    .fetch_all(db)
    .await?;
    let mut names: HashMap<i64, Vec<String>> = HashMap::new();
    for (id, name) in used {
        names.entry(id).or_default().push(name);
    }
    Ok(credentials
        .into_iter()
        .map(|credential| CredentialRow {
            mcps: names.remove(&credential.id).unwrap_or_default(),
            id: credential.id,
            name: credential.name,
            provider: credential.provider,
            client_id: credential.client_id,
        })
        .collect())
}

async fn credentials_response(
    state: &AppState,
    context: &PageContext,
    open: Option<&CredentialForm>,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let rows = rows(state, team.id()).await?;
    let public_app = PublicApp::of(&state.core.config);
    Ok(page(credentials_page(
        context,
        &CredentialsPage {
            rows: &rows,
            open,
            public_app: public_app.as_ref(),
        },
    )))
}

/// `GET /credentials`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
) -> Result<Response, AppError> {
    credentials_response(&state, &context, None).await
}

/// `GET /credentials/new`: the list with the dialog that adds one open.
pub async fn new(
    State(state): State<AppState>,
    context: PageContext,
) -> Result<Response, AppError> {
    credentials_response(&state, &context, Some(&CredentialForm::default())).await
}

async fn find(
    state: &AppState,
    context: &PageContext,
    id: &str,
) -> Result<Option<ProviderCredential>, AppError> {
    let team = context.current_team()?;
    Ok(match credential_id(id) {
        Some(id) => ProviderCredential::find_in_team(&*state.core.db, team.id(), id).await?,
        None => None,
    })
}

/// `GET /credentials/{id}/edit`: the list with the dialog of one open.
pub async fn edit(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Some(credential) = find(&state, &context, &id).await? else {
        session.flash("error", NOT_FOUND);
        return Ok(navigate(&headers, "/credentials"));
    };
    let used_by = credential.mcps(&*state.core.db).await?.len();
    let form = CredentialForm::new(&credential.name, &credential.client_id)
        .editing(credential.id, used_by);
    credentials_response(&state, &context, Some(&form)).await
}

/// A refused form: the dialog again for the page's script, the first error
/// as a message for a plain post.
fn refused(
    state: &AppState,
    context: &PageContext,
    session: &Session,
    headers: &HeaderMap,
    form: CredentialForm,
) -> Response {
    let message = form.first_error().unwrap_or_default().to_string();
    let public_app = PublicApp::of(&state.core.config);
    invalid_form(
        headers,
        session,
        credential_fragment(context, &form, public_app.as_ref()),
        &message,
        "/credentials",
    )
}

fn text(input: &Map<String, Value>, field: &str) -> String {
    match input.get(field) {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// The form as it was sent, without its secret.
fn form_of(input: &Map<String, Value>) -> CredentialForm {
    CredentialForm::new(&text(input, "name"), &text(input, "clientId"))
}

fn validated(
    validator: &mymcps_vine::Validator,
    input: &Map<String, Value>,
) -> Result<Result<CredentialPayload, Vec<(String, String)>>, AppError> {
    match validator.validate_as(&Value::Object(input.clone())) {
        Ok(payload) => Ok(Ok(payload)),
        Err(error) => {
            let error = refusal(error)?;
            Ok(Err(error
                .messages
                .iter()
                .map(|message| (message.field.clone(), message.message.clone()))
                .collect()))
        }
    }
}

/// Which unique index a refused write broke, as the field that says so.
fn taken(error: &sqlx::Error) -> Option<(String, String)> {
    let database = error.as_database_error()?;
    if !database.is_unique_violation() {
        return None;
    }
    Some(if database.message().contains("client_id") {
        ("clientId".into(), CLIENT_TAKEN.into())
    } else {
        ("name".into(), NAME_TAKEN.into())
    })
}

/// `POST /credentials`
pub async fn store(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let team = context.current_team()?;
    let refuse = |errors: Vec<(String, String)>| {
        refused(
            &state,
            &context,
            &session,
            &headers,
            form_of(&input).with_errors(errors),
        )
    };
    let payload = match validated(&CREATE_CREDENTIAL_VALIDATOR, &input)? {
        Ok(payload) => payload,
        Err(errors) => return Ok(refuse(errors)),
    };
    let db = &*state.core.db;
    let existing = ProviderCredential::of_team(db, team.id(), None).await?;
    if existing.len() >= MAX_CREDENTIALS_PER_TEAM {
        return Ok(refuse(vec![(
            FORM_ERROR.into(),
            format!("A team can save up to {MAX_CREDENTIALS_PER_TEAM} OAuth clients"),
        )]));
    }
    let mut credential = ProviderCredential {
        team_id: team.id(),
        provider: payload.provider.clone().unwrap_or_default(),
        name: payload.name.clone(),
        client_id: payload.client_id.clone(),
        client_secret: state
            .core
            .encrypt_secret(payload.client_secret.as_deref())
            .unwrap_or_default(),
        created_by: user.id,
        ..Default::default()
    };
    if let Err(error) = credential.insert(db).await {
        return match taken(&error) {
            Some(field) => Ok(refuse(vec![field])),
            None => Err(error.into()),
        };
    }
    session.flash("success", "OAuth client added");
    Ok(navigate(&headers, "/credentials"))
}

/// `PUT /credentials/{id}`: the MCPs that use it get the new client at once.
pub async fn update(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(mut credential) = find(&state, &context, &id).await? else {
        session.flash("error", NOT_FOUND);
        return Ok(navigate(&headers, "/credentials"));
    };
    let db = &*state.core.db;
    let credential_id = credential.id;
    let used_by = credential.mcps(db).await?.len();
    let refuse = |errors: Vec<(String, String)>| {
        refused(
            &state,
            &context,
            &session,
            &headers,
            form_of(&input)
                .editing(credential_id, used_by)
                .with_errors(errors),
        )
    };
    let payload = match validated(&UPDATE_CREDENTIAL_VALIDATOR, &input)? {
        Ok(payload) => payload,
        Err(errors) => return Ok(refuse(errors)),
    };
    credential.name = payload.name.clone();
    credential.client_id = payload.client_id.clone();
    if let Some(secret) = payload.client_secret.as_deref() {
        credential.client_secret = state
            .core
            .encrypt_secret_keeping(Some(&credential.client_secret), Some(secret))
            .unwrap_or_default();
    }
    let change = match credential.save_to_mcps(db, &state.core.encryption).await {
        Ok(change) => change,
        Err(error) => {
            return match taken(&error) {
                Some(field) => Ok(refuse(vec![field])),
                None => Err(error.into()),
            };
        }
    };
    let message = match change.signed_out {
        0 => "OAuth client saved".to_owned(),
        1 => "OAuth client saved. Its MCP has to be connected again: the Client ID changed"
            .to_owned(),
        many => format!(
            "OAuth client saved. Its {many} MCPs have to be connected again: the Client ID changed"
        ),
    };
    session.flash("success", &message);
    Ok(navigate(&headers, "/credentials"))
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

/// `DELETE /credentials/{id}`: refused while an MCP signs in with it.
pub async fn destroy(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let back = |kind: &str, message: &str| {
        session.flash(kind, message);
        navigate(&headers, "/credentials")
    };
    let Some(credential) = find(&state, &context, &id).await? else {
        return Ok(back("error", NOT_FOUND));
    };
    let db = &*state.core.db;
    let users: Vec<String> = credential
        .mcps(db)
        .await?
        .into_iter()
        .map(|mcp| mcp.name)
        .collect();
    if !users.is_empty() || !credential.delete_unused(db).await? {
        let names = if users.is_empty() {
            "an MCP".to_owned()
        } else {
            names_sentence(&users)
        };
        return Ok(back(
            "error",
            &format!(
                "{} is used by {names}. Choose another OAuth client for {} first.",
                credential.name,
                if users.len() == 1 { "it" } else { "them" },
            ),
        ));
    }
    Ok(back("success", "OAuth client deleted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_mcps_that_keep_a_credential() {
        let names =
            |count: usize| -> Vec<String> { (1..=count).map(|n| format!("M{n}")).collect() };
        assert_eq!(names_sentence(&names(1)), "M1");
        assert_eq!(names_sentence(&names(3)), "M1, M2 and M3");
        assert_eq!(names_sentence(&names(7)), "M1, M2, M3, M4, M5 and 2 more");
    }
}
