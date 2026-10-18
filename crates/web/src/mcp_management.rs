//! What the tools that manage the MCP registry do: the operations of the
//! MCPs page, for an agent. They run the page's own validators and assign
//! the same way, so a tool accepts exactly what the form accepts; only the
//! answer differs. Nothing here returns a secret.

use std::sync::Arc;

use async_trait::async_trait;
use mymcps_core::models::{Mcp, McpStatus, McpTransport, Team};
use mymcps_core::redaction::{sanitize_diagnostic, sanitize_mcp_diagnostic};
use mymcps_gateway::mcp_management::{
    ManagementActor, ManagementError, McpFields, McpRegistryManager,
};
use mymcps_upstream::Upstream;
use mymcps_vine::{Error as VineError, ValidationError};
use serde_json::{Map, Value, json};
use url::Url;

use crate::routes::mcps::{
    AssignError, assign_mcp_from_payload, discard_sandbox, discard_uploads, uses_oauth,
};
use crate::validators::mcp::{CREATE_MCP_VALIDATOR, McpPayload, UPDATE_MCP_VALIDATOR};
use crate::views::mcps::McpView;

const BUILTIN_REFUSAL: &str = "Built-in MCPs cannot be created or changed with this tool: configure built-in MCPs from the MyMCPs page.";

/// The MCPs of a team, managed by the agent of an access token.
#[derive(Debug, Clone)]
pub struct WebMcpRegistry {
    upstream: Arc<Upstream>,
}

impl WebMcpRegistry {
    pub fn new(upstream: Arc<Upstream>) -> Self {
        Self { upstream }
    }
}

fn failed(error: impl std::fmt::Display) -> ManagementError {
    ManagementError::Failed(sanitize_diagnostic(&error.to_string()))
}

fn refused(message: impl Into<String>) -> ManagementError {
    ManagementError::Refused(message.into())
}

fn is_set(column: &Option<String>) -> bool {
    column.as_deref().is_some_and(|value| !value.is_empty())
}

/// The fields of the form under the names the tools give them.
const ARGUMENT_NAMES: [(&str, &str); 9] = [
    ("httpUrl", "url"),
    ("npmPackage", "npm_package"),
    ("npmVersion", "npm_version"),
    ("npmArgs", "npm_args"),
    ("npmEnv", "npm_env"),
    ("authType", "auth_type"),
    ("authBearer", "bearer_token"),
    ("authHeaderName", "header_name"),
    ("authHeaderValue", "header_value"),
];

fn argument_names(text: &str) -> String {
    ARGUMENT_NAMES
        .iter()
        .fold(text.to_owned(), |text, (field, argument)| {
            text.replace(field, argument)
        })
}

/// The first thing the form validators found wrong, as one sentence naming
/// the argument.
fn refusal_of(error: &ValidationError) -> ManagementError {
    let Some(first) = error.messages.first() else {
        return refused("The MCP is not valid");
    };
    refused(format!(
        "Invalid {}: {}",
        argument_names(&first.field),
        argument_names(&first.message)
    ))
}

/// The URL of an MCP as an agent may read it: no credentials, and no value
/// in the query, where API keys are often put.
fn public_url(url: &str) -> String {
    let Ok(mut url) = Url::parse(url) else {
        return "(invalid URL)".to_owned();
    };
    // Only fails for a URL that cannot hold credentials, which has none.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);
    let names: Vec<String> = url
        .query_pairs()
        .map(|(name, _)| name.into_owned())
        .collect();
    if names.is_empty() {
        url.set_query(None);
    } else {
        url.set_query(None);
        let mut pairs = url.query_pairs_mut();
        for name in &names {
            pairs.append_pair(name, "***");
        }
    }
    url.to_string()
}

/// Whether saving `fields` leaves the MCP on another origin or package than
/// `saved`, which is when `assign_mcp_from_payload` forgets its secrets.
fn changes_package(saved: &Mcp, fields: &McpFields) -> bool {
    let transport = fields.transport.unwrap_or(saved.transport);
    let package = fields
        .npm_package
        .clone()
        .or_else(|| saved.npm_package.clone());
    transport != saved.transport || (transport == McpTransport::Npm && package != saved.npm_package)
}

/// The form of the MCPs page, filled in from what the agent gave over what
/// is saved. A new MCP signs in with `auto` and is enabled, as the create
/// form starts.
fn form_input(saved: Option<&Mcp>, fields: &McpFields) -> Map<String, Value> {
    let mut input = Map::new();
    let mut put = |name: &str, value: Value| {
        input.insert(name.to_owned(), value);
    };
    match saved {
        Some(mcp) => {
            put("name", json!(mcp.name));
            put(
                "description",
                json!(mcp.description.clone().unwrap_or_default()),
            );
            put("transport", json!(mcp.transport.as_str()));
            put("httpUrl", json!(mcp.http_url.clone().unwrap_or_default()));
            put(
                "npmPackage",
                json!(mcp.npm_package.clone().unwrap_or_default()),
            );
            put(
                "npmVersion",
                json!(mcp.npm_version.clone().unwrap_or_default()),
            );
            put("npmArgs", json!(mcp.npm_args_list().join(" ")));
            put("authType", json!(mcp.auth_type.as_str()));
            put(
                "authHeaderName",
                json!(mcp.auth_header_name.clone().unwrap_or_default()),
            );
            put("enabled", json!(mcp.enabled));
        }
        None => {
            put("authType", json!("auto"));
            put("enabled", json!(true));
        }
    }

    let given = |put: &mut dyn FnMut(&str, Value), name: &str, value: &Option<String>| {
        if let Some(value) = value {
            put(name, json!(value));
        }
    };
    given(&mut put, "name", &fields.name);
    if let Some(transport) = fields.transport {
        put("transport", json!(transport.as_str()));
    }
    given(&mut put, "description", &fields.description);
    given(&mut put, "httpUrl", &fields.url);
    given(&mut put, "npmPackage", &fields.npm_package);
    given(&mut put, "npmVersion", &fields.npm_version);
    if let Some(args) = &fields.npm_args {
        put("npmArgs", json!(args.join(" ")));
    }
    if let Some(auth_type) = fields.auth_type {
        put("authType", json!(auth_type.as_str()));
    }
    given(&mut put, "authBearer", &fields.bearer_token);
    given(&mut put, "authHeaderName", &fields.header_name);
    given(&mut put, "authHeaderValue", &fields.header_value);
    if let Some(enabled) = fields.enabled {
        put("enabled", json!(enabled));
    }

    // The environment is the whole list, as in the form: a variable without
    // a value keeps the saved one of its name. Left out, the saved
    // variables stay, unless the MCP runs another package, where the form
    // itself would ask for each value again.
    let rows: Option<Vec<Value>> = match (&fields.npm_env, saved) {
        (Some(rows), _) => Some(
            rows.iter()
                .map(|row| json!({ "name": row.name, "value": row.value }))
                .collect(),
        ),
        (None, Some(saved)) if !changes_package(saved, fields) => Some(
            saved
                .npm_env_names()
                .into_iter()
                .map(|name| json!({ "name": name, "value": null }))
                .collect(),
        ),
        (None, _) => None,
    };
    if let Some(rows) = rows {
        input.insert("npmEnv".to_owned(), Value::Array(rows));
    }
    input
}

/// Validate a filled form with the validator of the page.
fn validated(
    validator: &mymcps_vine::Validator,
    input: &Map<String, Value>,
) -> Result<McpPayload, ManagementError> {
    match validator.validate_as::<McpPayload>(&Value::Object(input.clone())) {
        Ok(payload) => Ok(payload),
        Err(VineError::Validation(error)) => Err(refusal_of(&error)),
        Err(VineError::Output(error)) => Err(failed(error)),
    }
}

fn assign_refusal(error: AssignError) -> ManagementError {
    match error {
        AssignError::Refused(error) => refusal_of(&error),
        AssignError::Database(error) => failed(error),
    }
}

impl WebMcpRegistry {
    async fn find(&self, actor: &ManagementActor, slug: &str) -> Result<Mcp, ManagementError> {
        let mcp: Option<Mcp> =
            sqlx::query_as("select * from `mcps` where `team_id` = ? and `slug` = ? limit 1")
                .bind(actor.team_id)
                .bind(slug)
                .fetch_optional(&*self.upstream.core().db)
                .await
                .map_err(failed)?;
        mcp.ok_or_else(|| {
            refused(format!(
                "MCP \"{slug}\" was not found in this team. List the MCPs with mcp_registry_list."
            ))
        })
    }

    /// What an agent may know of an MCP.
    fn summary(&self, mcp: &Mcp) -> Value {
        let tool_count = self.upstream.known_tool_counts().get(&mcp.id).copied();
        let view = McpView::of(&self.upstream, mcp, tool_count);
        let builtin = mcp.transport == McpTransport::Builtin;
        json!({
            "id": mcp.id,
            "name": mcp.name,
            "slug": mcp.slug,
            "description": mcp.description,
            "transport": mcp.transport.as_str(),
            "enabled": mcp.enabled,
            "status": mcp.status.as_str(),
            "last_error": mcp.last_error,
            "url": mcp.http_url.as_deref().map(public_url),
            "builtin_key": mcp.builtin_key,
            "npm_package": mcp.npm_package,
            "npm_version": mcp.npm_version,
            "npm_args": mcp.npm_args_list(),
            "npm_env": view.npm_env.iter().map(|name| json!({ "name": name, "value_set": true })).collect::<Vec<_>>(),
            "auth_type": (!builtin).then(|| mcp.auth_type.as_str()),
            "bearer_token_set": view.has_auth_bearer,
            "header_name": mcp.auth_header_name,
            "header_value_set": view.has_auth_header_value,
            "uses_oauth": uses_oauth(&self.upstream, mcp),
            "oauth_connected": uses_oauth(&self.upstream, mcp) && view.is_connected(),
            "oauth_required": mcp.oauth_required,
            "tool_count": view.tool_count,
        })
    }

    /// The link that opens the dialog of an MCP in the app, where its OAuth
    /// connection is started. Not the route that starts it: that one refuses
    /// a click that comes from another site, a web chat for one.
    fn auth_url(&self, mcp: &Mcp) -> Result<String, ManagementError> {
        let origin = self
            .upstream
            .core()
            .config
            .require_public_app_url()
            .map_err(|error| refused(format!("No link can be made: {error}")))?;
        Ok(format!("{origin}/mcps/{}", mcp.id))
    }

    async fn team_name(&self, actor: &ManagementActor) -> Option<String> {
        Team::find(&*self.upstream.core().db, actor.team_id)
            .await
            .ok()
            .flatten()
            .map(|team| team.name)
    }

    /// The result of testing an MCP, with the link to connect it when it
    /// waits for OAuth.
    fn test_report(&self, mcp: &Mcp) -> Value {
        let ok = mcp.status == McpStatus::Ready;
        let (auth_link, auth_link_problem) =
            if !ok && mcp.oauth_required && uses_oauth(&self.upstream, mcp) {
                match self.auth_url(mcp) {
                    Ok(link) => (Some(link), None),
                    Err(ManagementError::Refused(message)) => (None, Some(message)),
                    Err(error) => (None, Some(error.to_string())),
                }
            } else {
                (None, None)
            };
        json!({
            "status": mcp.status.as_str(),
            "ok": ok,
            "error": if ok { None } else { mcp.last_error.clone() },
            "oauth_required": mcp.oauth_required,
            "auth_link": auth_link,
            "auth_link_problem": auth_link_problem,
            "tool_count": self.upstream.known_tool_counts().get(&mcp.id).copied(),
        })
    }

    async fn test_row(&self, mcp: &mut Mcp) -> Result<(), ManagementError> {
        self.upstream
            .test_and_update_status(mcp)
            .await
            .map_err(|error| {
                ManagementError::Failed(sanitize_mcp_diagnostic(
                    &self.upstream.core().encryption,
                    &error.to_string(),
                    mcp,
                ))
            })
    }
}

#[async_trait]
impl McpRegistryManager for WebMcpRegistry {
    async fn list(&self, actor: &ManagementActor) -> Result<Value, ManagementError> {
        let mcps: Vec<Mcp> = sqlx::query_as(
            "select * from `mcps` where `team_id` = ? order by `name` asc, `id` asc",
        )
        .bind(actor.team_id)
        .fetch_all(&*self.upstream.core().db)
        .await
        .map_err(failed)?;
        Ok(json!({ "mcps": mcps.iter().map(|mcp| self.summary(mcp)).collect::<Vec<_>>() }))
    }

    async fn create(
        &self,
        actor: &ManagementActor,
        fields: McpFields,
    ) -> Result<Value, ManagementError> {
        let input = form_input(None, &fields);
        let payload = validated(&CREATE_MCP_VALIDATOR, &input)?;
        let mut mcp = Mcp {
            status: McpStatus::Draft,
            created_by: actor.user_id,
            team_id: actor.team_id,
            ..Default::default()
        };
        assign_mcp_from_payload(&self.upstream, &mut mcp, &payload, None)
            .await
            .map_err(assign_refusal)?;
        mcp.insert(&*self.upstream.core().db)
            .await
            .map_err(failed)?;

        self.test_row(&mut mcp).await?;
        Ok(json!({
            "mcp": self.summary(&mcp),
            "test": self.test_report(&mcp),
        }))
    }

    async fn update(
        &self,
        actor: &ManagementActor,
        slug: &str,
        fields: McpFields,
    ) -> Result<Value, ManagementError> {
        let saved = self.find(actor, slug).await?;
        if saved.transport == McpTransport::Builtin {
            return Err(refused(BUILTIN_REFUSAL));
        }

        let input = form_input(Some(&saved), &fields);
        let payload = validated(&UPDATE_MCP_VALIDATOR, &input)?;
        let previous_npm_package = saved
            .npm_package
            .clone()
            .filter(|package| saved.transport == McpTransport::Npm && !package.is_empty());
        let mut mcp = saved.clone();
        assign_mcp_from_payload(&self.upstream, &mut mcp, &payload, Some(saved.id))
            .await
            .map_err(assign_refusal)?;
        mcp.save(&*self.upstream.core().db).await.map_err(failed)?;
        // Another package must not start in what the previous one left behind.
        if previous_npm_package.is_some() && previous_npm_package != mcp.npm_package {
            discard_sandbox(&self.upstream, mcp.id).await;
        }

        // What saving forgot, so the agent can enter it again.
        let mut cleared = Vec::new();
        if is_set(&saved.auth_bearer) && !is_set(&mcp.auth_bearer) {
            cleared.push("bearer_token");
        }
        if is_set(&saved.auth_header_value) && !is_set(&mcp.auth_header_value) {
            cleared.push("header_value");
        }
        let saved_variables = saved.npm_env_names();
        let kept_variables = mcp.npm_env_names();
        if saved_variables
            .iter()
            .any(|name| !kept_variables.contains(name))
            && fields.npm_env.is_none()
        {
            cleared.push("npm_env");
        }
        if is_set(&saved.oauth_access_token) && !is_set(&mcp.oauth_access_token) {
            cleared.push("oauth_connection");
        }

        self.test_row(&mut mcp).await?;
        let mut notes = Vec::new();
        if !cleared.is_empty() {
            notes.push(format!(
                "Saved secrets are cleared when the URL origin or the npm package changes, or when the sign-in type stops using them. Cleared: {}. Send them again with {}.",
                cleared.join(", "),
                mymcps_gateway::mcp_management::UPDATE_TOOL
            ));
        }
        if mcp.slug != saved.slug {
            notes.push(format!(
                "The slug changed from \"{}\" to \"{}\": use the new one.",
                saved.slug, mcp.slug
            ));
        }
        Ok(json!({
            "mcp": self.summary(&mcp),
            "test": self.test_report(&mcp),
            "cleared_secrets": cleared,
            "previous_slug": (mcp.slug != saved.slug).then_some(saved.slug),
            "notes": notes,
        }))
    }

    async fn delete(&self, actor: &ManagementActor, slug: &str) -> Result<Value, ManagementError> {
        let mcp = self.find(actor, slug).await?;
        mcp.delete(&*self.upstream.core().db)
            .await
            .map_err(failed)?;
        discard_sandbox(&self.upstream, mcp.id).await;
        discard_uploads(&self.upstream, mcp.id).await;
        Ok(json!({
            "deleted": true,
            "mcp": { "id": mcp.id, "name": mcp.name, "slug": mcp.slug },
        }))
    }

    async fn test(&self, actor: &ManagementActor, slug: &str) -> Result<Value, ManagementError> {
        let mut mcp = self.find(actor, slug).await?;
        self.test_row(&mut mcp).await?;
        let mut report = self.test_report(&mcp);
        report["mcp"] = self.summary(&mcp);
        Ok(report)
    }

    async fn auth_link(
        &self,
        actor: &ManagementActor,
        slug: &str,
    ) -> Result<Value, ManagementError> {
        let mcp = self.find(actor, slug).await?;
        if !uses_oauth(&self.upstream, &mcp) {
            return Ok(json!({
                "mcp": { "id": mcp.id, "name": mcp.name, "slug": mcp.slug },
                "uses_oauth": false,
                "message": "This MCP does not sign in with OAuth, so there is nothing to connect. If it should, test it with mcp_registry_test: an MCP only shows it needs OAuth once its server asks for it.",
            }));
        }
        let link = self.auth_url(&mcp)?;
        let team = self.team_name(actor).await;
        Ok(json!({
            "mcp": { "id": mcp.id, "name": mcp.name, "slug": mcp.slug },
            "uses_oauth": true,
            "connected": self.summary(&mcp)["oauth_connected"],
            "auth_link": link,
            "instructions": format!(
                "Give this link to the user. They must be signed in to MyMCPs with the team {} selected; the link opens the dialog of this MCP, where they press Connect and authorize the access. Once they did, test the MCP again.",
                team.map_or_else(|| "of this access token".to_owned(), |name| format!("\"{name}\""))
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_credentials_and_query_values_of_an_url() {
        assert_eq!(
            public_url("https://user:pass@mcp.example/mcp?key=abc&x=1#top"),
            "https://mcp.example/mcp?key=***&x=***"
        );
        assert_eq!(
            public_url("https://mcp.example/mcp"),
            "https://mcp.example/mcp"
        );
        assert_eq!(public_url("not a url"), "(invalid URL)");
    }

    #[test]
    fn names_the_arguments_of_the_tools_in_a_refusal() {
        let error = ValidationError::single("httpUrl", "required", "The httpUrl field is required");
        assert_eq!(
            refusal_of(&error),
            ManagementError::Refused("Invalid url: The url field is required".into())
        );
    }
}
