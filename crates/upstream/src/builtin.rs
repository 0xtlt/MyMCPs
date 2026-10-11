//! Built-in MCPs at run time: what the saved sign-in may do, and the calls
//! the gateway makes into a provider once that is known.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use mymcps_builtin::oauth::parse_oauth_scopes;
use mymcps_builtin::{
    ApprovalSummary, BuiltinError, BuiltinFile, BuiltinMcpDefinition, BuiltinOauthConfig,
    BuiltinPasswordContext, BuiltinProvider, BuiltinResult, BuiltinTool, BuiltinToolContext,
    BuiltinUploadTarget,
};
use mymcps_core::models::Mcp;
use mymcps_core::secrets::decrypt_environment;
use serde_json::{Map, Value, json};

use crate::Upstream;
use crate::http_client::UpstreamTool;

/// Run the same code for whichever kind of sign-in a definition has, with
/// its provider.
macro_rules! for_provider {
    ($definition:expr, |$provider:ident| $body:expr) => {
        match $definition {
            BuiltinMcpDefinition::Oauth {
                provider: $provider,
                ..
            } => $body,
            BuiltinMcpDefinition::Password {
                provider: $provider,
                ..
            } => $body,
        }
    };
}

fn not_connected(definition: &BuiltinMcpDefinition) -> BuiltinError {
    BuiltinError::authorization(format!(
        "{} is not connected. Connect it from the MCPs page in MyMCPs.",
        definition.name()
    ))
}

fn is_set(column: &Option<String>) -> bool {
    column.as_deref().is_some_and(|value| !value.is_empty())
}

/// What the saved sign-in may do. An OAuth provider reports the scopes it
/// granted, or nothing at all (`None`, which allows every tool). A password
/// may do exactly what the admin allowed in MyMCPs.
fn granted_scopes(definition: &BuiltinMcpDefinition, mcp: &Mcp) -> Option<Vec<String>> {
    if definition.password().is_some() {
        return Some(parse_oauth_scopes(mcp.builtin_permissions.as_deref()));
    }
    let scopes = parse_oauth_scopes(mcp.oauth_scopes.as_deref());
    (!scopes.is_empty()).then_some(scopes)
}

fn is_granted(requires_any_scope: &[&'static str], scopes: Option<&[String]>) -> bool {
    let Some(scopes) = scopes else {
        return true;
    };
    requires_any_scope.is_empty()
        || requires_any_scope
            .iter()
            .any(|scope| scopes.iter().any(|granted| granted == scope))
}

fn not_granted(
    definition: &BuiltinMcpDefinition,
    tool_name: &str,
    needs: &[&'static str],
) -> String {
    let permission = format!("\"{}\"", needs.join("\" or \""));
    let name = definition.name();
    if definition.oauth().is_some() {
        format!(
            "{tool_name} needs the {name} permission {permission}, which was not granted. Re-authorize this MCP in MyMCPs and keep that permission checked."
        )
    } else {
        format!(
            "{tool_name} needs the {permission} permission, which is not allowed for this {name} MCP. An administrator can allow it from the MCPs page in MyMCPs."
        )
    }
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// Whether a sign-in was saved. The provider may have revoked it since.
fn has_sign_in(definition: &BuiltinMcpDefinition, mcp: &Mcp) -> bool {
    if definition.oauth().is_some() {
        is_set(&mcp.oauth_access_token)
    } else {
        is_set(&mcp.builtin_username) && is_set(&mcp.builtin_password)
    }
}

/// The context of one kind of sign-in, loaded from the row of its MCP. A
/// provider is paired with the loader of its own kind, so its tools only ever
/// run with the context they were written for.
trait SignIn: Sized + Send + Sync + 'static {
    fn load(
        upstream: &Upstream,
        definition: &BuiltinMcpDefinition,
        mcp: &mut Mcp,
    ) -> impl Future<Output = BuiltinResult<Self>> + Send;
}

impl SignIn for BuiltinToolContext {
    async fn load(
        upstream: &Upstream,
        definition: &BuiltinMcpDefinition,
        mcp: &mut Mcp,
    ) -> BuiltinResult<Self> {
        if !is_set(&mcp.oauth_access_token) {
            return Err(not_connected(definition));
        }
        upstream
            .refresh_oauth_access_token(mcp)
            .await
            .map_err(|error| error.into_builtin())?;

        // The refresh reloads the row, so read the token it left behind.
        let access_token = upstream
            .core
            .decrypt_secret(mcp.oauth_access_token.as_deref())
            .ok_or_else(|| not_connected(definition))?;
        Ok(Self {
            env: upstream.builtin_env.clone(),
            mcp_id: mcp.id,
            access_token,
            granted_scopes: granted_scopes(definition, mcp),
            settings: upstream.builtin_settings(definition, mcp),
        })
    }
}

impl SignIn for BuiltinPasswordContext {
    async fn load(
        upstream: &Upstream,
        definition: &BuiltinMcpDefinition,
        mcp: &mut Mcp,
    ) -> BuiltinResult<Self> {
        let username = mcp
            .builtin_username
            .clone()
            .filter(|username| !username.is_empty());
        let password = upstream
            .core
            .decrypt_secret(mcp.builtin_password.as_deref());
        let (Some(username), Some(password)) = (username, password) else {
            return Err(not_connected(definition));
        };
        Ok(Self {
            env: upstream.builtin_env.clone(),
            mcp_id: mcp.id,
            username,
            password,
            permissions: granted_scopes(definition, mcp).unwrap_or_default(),
            aliases: mcp
                .builtin_aliases
                .as_deref()
                .map(|aliases| aliases.split(' ').map(str::to_owned).collect())
                .unwrap_or_default(),
            settings: upstream.builtin_settings(definition, mcp),
        })
    }
}

impl Upstream {
    /// The built-in MCPs this instance has.
    pub fn builtins(&self) -> &mymcps_builtin::BuiltinRegistry {
        &self.builtins
    }

    /// The definition of a built-in MCP, or `None` when its key is not one
    /// this instance has.
    pub fn builtin_mcp(&self, key: Option<&str>) -> Option<&Arc<BuiltinMcpDefinition>> {
        self.builtins.get(key)
    }

    pub fn require_builtin_mcp(&self, mcp: &Mcp) -> BuiltinResult<&Arc<BuiltinMcpDefinition>> {
        self.builtin_mcp(mcp.builtin_key.as_deref()).ok_or_else(|| {
            BuiltinError::internal(format!(
                "Unknown built-in MCP: {}",
                mcp.builtin_key.as_deref().unwrap_or("none")
            ))
        })
    }

    /// The name and OAuth settings of a built-in MCP that signs in with OAuth.
    pub(crate) fn require_builtin_oauth_mcp(
        &self,
        mcp: &Mcp,
    ) -> BuiltinResult<(&'static str, &BuiltinOauthConfig)> {
        let definition = self.require_builtin_mcp(mcp)?;
        match definition.oauth() {
            Some(oauth) => Ok((definition.name(), oauth)),
            None => Err(BuiltinError::internal(format!(
                "{} does not sign in with OAuth",
                definition.name()
            ))),
        }
    }

    /// Whether the provider granted any write scope. Unknown scopes count as
    /// granted so the UI does not ask to re-authorize on a guess. A password
    /// sign-in has nothing to re-authorize, and neither has a provider whose one
    /// scope both reads and writes.
    pub fn builtin_write_granted(&self, mcp: &Mcp) -> BuiltinResult<bool> {
        let definition = self.require_builtin_mcp(mcp)?;
        let Some(oauth) = definition
            .oauth()
            .filter(|oauth| !oauth.write_scopes.is_empty())
        else {
            return Ok(true);
        };

        Ok(match granted_scopes(definition, mcp) {
            Some(scopes) => oauth
                .write_scopes
                .iter()
                .any(|scope| scopes.iter().any(|granted| granted == scope)),
            None => true,
        })
    }

    /// What the admin entered for the provider's settings. Values saved under a
    /// key the provider no longer has, or that can no longer be decrypted, are
    /// left out: the tool that needs one says so.
    pub fn builtin_settings(
        &self,
        definition: &BuiltinMcpDefinition,
        mcp: &Mcp,
    ) -> BTreeMap<String, String> {
        let saved = decrypt_environment(&self.core.encryption, mcp.builtin_settings.as_deref())
            .unwrap_or_default();
        definition
            .settings()
            .iter()
            .filter_map(|field| {
                let (_, value) = saved.iter().find(|(key, _)| key == field.key)?;
                Some((field.key.to_owned(), value.clone()))
            })
            .collect()
    }

    /// Tool definitions are static, so listing them never calls the provider.
    /// Write tools are left out until the admin allows write access, and so are
    /// tools whose permission was unchecked: on the provider's consent screen, or
    /// in MyMCPs for a password sign-in.
    pub fn list_builtin_tools(&self, mcp: &Mcp) -> BuiltinResult<Vec<UpstreamTool>> {
        let definition = self.require_builtin_mcp(mcp)?;
        if !has_sign_in(definition, mcp) {
            return Err(not_connected(definition));
        }

        let scopes = granted_scopes(definition, mcp);
        Ok(definition
            .tools()
            .into_iter()
            .filter(|tool| {
                (!tool.write || mcp.builtin_write_enabled)
                    && is_granted(tool.requires_any_scope, scopes.as_deref())
            })
            .map(|tool| UpstreamTool {
                name: tool.name.to_owned(),
                description: Some(tool.description.to_owned()),
                input_schema: tool.input_schema.clone(),
            })
            .collect())
    }

    /// The sign-in a provider's tools run with, loaded from the row of the MCP.
    async fn sign_in<C: SignIn>(
        &self,
        _provider: &BuiltinProvider<C>,
        definition: &BuiltinMcpDefinition,
        mcp: &mut Mcp,
    ) -> BuiltinResult<Arc<C>> {
        Ok(Arc::new(C::load(self, definition, mcp).await?))
    }

    /// A tool and its sign-in, once the call is known to be one this MCP
    /// allows. Fails with a tool error when it is not.
    async fn allowed_tool<'a, C: SignIn>(
        &self,
        definition: &BuiltinMcpDefinition,
        provider: &'a BuiltinProvider<C>,
        mcp: &mut Mcp,
        tool_name: &str,
    ) -> BuiltinResult<(&'a BuiltinTool<C>, Arc<C>)> {
        let Some(tool) = provider.tool(tool_name) else {
            return Err(BuiltinError::tool(format!(
                "Unknown {} tool: {tool_name}",
                provider.name
            )));
        };
        if tool.write && !mcp.builtin_write_enabled {
            return Err(BuiltinError::tool(format!(
                "{tool_name} changes {} data, and write access is turned off for this MCP. An administrator can allow it from the MCPs page in MyMCPs.",
                provider.name
            )));
        }

        let context = self.sign_in(provider, definition, mcp).await?;
        if !is_granted(
            &tool.requires_any_scope,
            granted_scopes(definition, mcp).as_deref(),
        ) {
            return Err(BuiltinError::tool(not_granted(
                definition,
                tool_name,
                &tool.requires_any_scope,
            )));
        }
        Ok((tool, context))
    }

    /// Call a tool of a built-in MCP. A failure the agent can act on is the
    /// result of the call, with `isError`.
    pub async fn call_builtin_tool(
        &self,
        mcp: &mut Mcp,
        tool_name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> BuiltinResult<Value> {
        let definition = Arc::clone(self.require_builtin_mcp(mcp)?);
        let arguments = arguments.unwrap_or_default();
        let outcome = for_provider!(&*definition, |provider| {
            match self
                .allowed_tool(&definition, provider, mcp, tool_name)
                .await
            {
                Ok((tool, context)) => tool.run(arguments, context).await,
                Err(error) => Err(error),
            }
        });
        match outcome {
            // `JSON.stringify(data)`: an agent reads the same text as before.
            Ok(data) => Ok(json!({
                "content": [{ "type": "text", "text": mymcps_vine::js::json_stringify(&data) }]
            })),
            Err(error) if error.is_tool_error() => Ok(tool_error(&error.to_string())),
            Err(error) => Err(error),
        }
    }

    /// What a call would do, for the person asked to approve it, or `None` when
    /// the tool only has its arguments to show. Nothing is changed at the
    /// provider. Fails with a tool error for a call that would be refused, so
    /// that nobody is asked to approve one.
    pub async fn describe_builtin_call(
        &self,
        mcp: &mut Mcp,
        tool_name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> BuiltinResult<Option<ApprovalSummary>> {
        let definition = Arc::clone(self.require_builtin_mcp(mcp)?);
        let arguments = arguments.unwrap_or_default();
        for_provider!(&*definition, |provider| {
            let (tool, context) = self
                .allowed_tool(&definition, provider, mcp, tool_name)
                .await?;
            tool.describe_call(arguments, context).await
        })
    }

    /// The file behind a link one of the MCP's tools handed out. Fails with a
    /// tool error when it cannot be served any more.
    pub async fn download_builtin_file(
        &self,
        mcp: &mut Mcp,
        reference: Value,
    ) -> BuiltinResult<BuiltinFile> {
        let definition = Arc::clone(self.require_builtin_mcp(mcp)?);
        for_provider!(&*definition, |provider| {
            if !provider.has_download() {
                return Err(BuiltinError::tool(format!(
                    "{} has no files to download",
                    provider.name
                )));
            }
            let context = self.sign_in(provider, &definition, mcp).await?;
            provider.download_file(reference, context).await
        })
    }

    /// Where to keep the file sent to an upload link one of the MCP's tools handed
    /// out. Fails with a tool error when the link can no longer be used.
    pub async fn builtin_upload_target(
        &self,
        mcp: &mut Mcp,
        reference: Value,
    ) -> BuiltinResult<BuiltinUploadTarget> {
        let definition = Arc::clone(self.require_builtin_mcp(mcp)?);
        for_provider!(&*definition, |provider| {
            if !provider.has_upload() {
                return Err(BuiltinError::tool(format!(
                    "{} takes no files",
                    provider.name
                )));
            }
            // The link outlives the call that made it. Where write access is one
            // switch, a file is only taken while it is on.
            if definition.oauth().is_some() && !mcp.builtin_write_enabled {
                return Err(BuiltinError::tool(format!(
                    "Write access is turned off for this {} MCP",
                    provider.name
                )));
            }
            let context = self.sign_in(provider, &definition, mcp).await?;
            provider.upload_target(reference, context).await
        })
    }

    /// Fails with an authorization error when the provider must be (re)authorized.
    pub async fn verify_builtin(&self, mcp: &mut Mcp) -> BuiltinResult<()> {
        let definition = Arc::clone(self.require_builtin_mcp(mcp)?);
        for_provider!(&*definition, |provider| {
            let context = self.sign_in(provider, &definition, mcp).await?;
            provider.verify(context).await
        })
    }
}
