//! How a built-in MCP is described: its sign-in, its tools, and the hooks
//! the gateway calls. A provider crate builds one [`BuiltinMcpDefinition`]
//! and the registry hands it to whoever lists, calls or configures the MCP.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use mymcps_core::Core;
use mymcps_net::Fetcher;
use regex::Regex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{BuiltinError, BuiltinResult};
use crate::upload_store::{BuiltinUploadTarget, UploadStore};

/// What the tools of every built-in MCP share: the instance, how they reach
/// the network, and where uploaded files are kept. Built once by the server,
/// and by each test with what it wants to observe or fake.
#[derive(Clone)]
pub struct BuiltinEnv {
    pub core: Arc<Core>,
    /// The only way a tool makes an HTTP request.
    pub fetcher: Fetcher,
    pub uploads: UploadStore,
    extensions: Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl BuiltinEnv {
    pub fn new(core: Arc<Core>) -> Self {
        let uploads = UploadStore::new(&core);
        Self {
            core,
            fetcher: Fetcher::shared(),
            uploads,
            extensions: Arc::default(),
        }
    }

    pub fn with_fetcher(mut self, fetcher: Fetcher) -> Self {
        self.fetcher = fetcher;
        self
    }

    /// Attach a value a provider looks up by its type: the seam a provider
    /// offers its tests, such as the mail servers to connect to. The server
    /// attaches none, and a provider falls back to its defaults.
    pub fn with_extension<T: Any + Send + Sync>(mut self, value: T) -> Self {
        let mut extensions = (*self.extensions).clone();
        extensions.insert(TypeId::of::<T>(), Arc::new(value));
        self.extensions = Arc::new(extensions);
        self
    }

    pub fn extension<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.extensions
            .get(&TypeId::of::<T>())
            .cloned()
            .and_then(|value| value.downcast::<T>().ok())
    }
}

impl std::fmt::Debug for BuiltinEnv {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("BuiltinEnv").finish_non_exhaustive()
    }
}

/// OAuth 2.0 authorization-code settings for a provider whose API
/// application the admin registers themselves, then pastes its client ID and
/// secret.
#[derive(Debug, Clone)]
pub struct BuiltinOauthConfig {
    pub issuer: &'static str,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub scopes: Vec<&'static str>,
    /// Requested on top of `scopes` once the admin allows write access.
    pub write_scopes: Vec<&'static str>,
    /// RFC 6749 separates scopes with spaces. Some providers expect commas.
    pub scope_separator: &'static str,
    pub authorize_params: Vec<(&'static str, &'static str)>,
    /// The provider wants the redirect URI again when the code is exchanged,
    /// as RFC 6749 asks. Opt-in, since a provider may also refuse a
    /// parameter it does not document.
    pub sends_redirect_uri_with_code: bool,
    /// Catches a secret pasted into the client ID field before the provider does.
    pub client_id_pattern: Option<Regex>,
    pub client_id_hint: Option<&'static str>,
}

/// Sign-in with an account name and a password the provider issues for apps,
/// for services that have no OAuth a self-hosted gateway can use.
#[derive(Debug, Clone)]
pub struct BuiltinPasswordConfig {
    pub username_pattern: Regex,
    pub username_hint: &'static str,
    /// Rejects the account's main password before it is stored.
    pub password_pattern: Regex,
    pub password_hint: &'static str,
    /// What the admin can allow agents to do. The provider cannot restrict
    /// such a password, so MyMCPs enforces these itself: a tool is only
    /// available when one of its `requires_any_scope` is allowed.
    pub permissions: Vec<&'static str>,
    /// Explains what the other addresses of the account must look like.
    pub alias_hint: &'static str,
}

/// Something a provider needs beyond its sign-in, which the admin enters
/// when adding the MCP: the account to act through, or the ones agents may
/// use. Not for credentials: the values are shown again in the setup dialog.
#[derive(Debug, Clone)]
pub struct BuiltinSettingField {
    pub key: &'static str,
    pub required: bool,
    pub pattern: Regex,
    /// What the admin reads when the value is missing or does not match.
    pub hint: String,
    /// The form the value is stored in, such as an account number without its dashes.
    pub normalize: Option<fn(&str) -> String>,
}

/// What a tool of an OAuth provider runs with.
#[derive(Debug, Clone)]
pub struct BuiltinToolContext {
    pub env: BuiltinEnv,
    pub mcp_id: i64,
    pub access_token: String,
    /// `None` when the provider did not report which scopes were granted.
    pub granted_scopes: Option<Vec<String>>,
    /// What the admin entered for the provider's `settings`. A blank one is left out.
    pub settings: BTreeMap<String, String>,
}

/// What a tool of a password provider runs with.
#[derive(Debug, Clone)]
pub struct BuiltinPasswordContext {
    pub env: BuiltinEnv,
    pub mcp_id: i64,
    pub username: String,
    pub password: String,
    /// What the admin allowed for this MCP.
    pub permissions: Vec<String>,
    /// Other addresses of the same account that the admin lets agents act as.
    pub aliases: Vec<String>,
    /// What the admin entered for the provider's `settings`. A blank one is left out.
    pub settings: BTreeMap<String, String>,
}

/// A file a tool linked to, such as a mail attachment.
#[derive(Debug, Clone)]
pub struct BuiltinFile {
    pub filename: String,
    pub content_type: String,
    /// The bytes in the pieces they arrived in, so that a large file is held in memory once.
    pub content: Vec<Bytes>,
}

/// What a call would do, in MyMCPs' own words, for the person asked to
/// approve it. Nothing in it is written by the agent: names and current
/// values are read from the provider, and the agent's arguments only appear
/// as the values they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalSummary {
    /// One sentence, such as `Change the daily budget of campaign "Spring sale"`.
    pub title: String,
    /// What the call sets, and for a change, the value it replaces.
    pub details: Vec<ApprovalDetail>,
    /// What the person should weigh before deciding, such as a budget multiplied by 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDetail {
    pub label: String,
    pub value: String,
    /// The current value this one replaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

impl ApprovalDetail {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            before: None,
        }
    }

    pub fn replacing(mut self, before: impl Into<String>) -> Self {
        self.before = Some(before.into());
        self
    }
}

/// How a tool checks the arguments an agent passed. The implementation for
/// validators of `mymcps-vine` is in [`crate::tool_input`].
pub trait ToolInput<C>: Send + Sync {
    /// The arguments once they passed, or the sentence telling the agent
    /// about the first one that is wrong.
    fn validate(&self, arguments: Value, context: &C) -> Result<Value, String>;

    /// The arguments as the validator enforces them, as a JSON Schema, to
    /// compare with the one the tool advertises.
    fn json_schema(&self) -> Value;
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type RunFn<C> = Arc<dyn Fn(Value, Arc<C>) -> BoxFuture<BuiltinResult<Value>> + Send + Sync>;
type DescribeFn<C> =
    Arc<dyn Fn(Value, Arc<C>) -> BoxFuture<BuiltinResult<Option<ApprovalSummary>>> + Send + Sync>;
type VerifyFn<C> = Arc<dyn Fn(Arc<C>) -> BoxFuture<BuiltinResult<()>> + Send + Sync>;
type DownloadFn<C> =
    Arc<dyn Fn(Value, Arc<C>) -> BoxFuture<BuiltinResult<BuiltinFile>> + Send + Sync>;
type UploadFn<C> =
    Arc<dyn Fn(Value, Arc<C>) -> BoxFuture<BuiltinResult<BuiltinUploadTarget>> + Send + Sync>;

/// One tool of a built-in MCP.
pub struct BuiltinTool<C> {
    pub name: &'static str,
    pub description: String,
    /// The arguments as the agent reads them.
    pub input_schema: Value,
    /// The arguments as `run` checks them. Both must describe the same ones.
    pub input: Arc<dyn ToolInput<C>>,
    /// The tool needs at least one of these provider scopes, or of these
    /// permissions for a password sign-in. Empty when any authorization works.
    pub requires_any_scope: Vec<&'static str>,
    /// Changes data at the provider. Unavailable until the admin allows write access.
    pub write: bool,
    /// A person approves each call before it runs, until the admin decides
    /// otherwise for this MCP. For the tools that commit money or go live.
    pub asks_approval: bool,
    run: RunFn<C>,
    describe: Option<DescribeFn<C>>,
}

impl<C: Send + Sync + 'static> BuiltinTool<C> {
    /// A tool whose `run` only ever sees arguments that passed `input`,
    /// deserialized into `I`. Return JSON-serializable data, and
    /// [`BuiltinError::Tool`] for expected failures.
    pub fn new<I, F, Fut>(
        name: &'static str,
        description: impl Into<String>,
        input_schema: Value,
        input: impl ToolInput<C> + 'static,
        run: F,
    ) -> Self
    where
        I: DeserializeOwned + Send + 'static,
        F: Fn(I, Arc<C>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BuiltinResult<Value>> + Send + 'static,
    {
        let run: RunFn<C> =
            Arc::new(
                move |validated, context| match serde_json::from_value::<I>(validated) {
                    Ok(input) => Box::pin(run(input, context)),
                    Err(error) => Box::pin(async move { Err(mismatched_input(name, error)) }),
                },
            );
        Self {
            name,
            description: description.into(),
            input_schema,
            input: Arc::new(input),
            requires_any_scope: Vec::new(),
            write: false,
            asks_approval: false,
            run,
            describe: None,
        }
    }

    pub fn requires_any_scope(mut self, scopes: &[&'static str]) -> Self {
        self.requires_any_scope = scopes.to_vec();
        self
    }

    pub fn write(mut self) -> Self {
        self.write = true;
        self
    }

    pub fn asks_approval(mut self) -> Self {
        self.asks_approval = true;
        self
    }

    /// Says what `run` would do with this input, for the person asked to
    /// approve the call. Without it, or when it returns `None`, they are
    /// shown the arguments.
    pub fn describe<I, F, Fut>(mut self, describe: F) -> Self
    where
        I: DeserializeOwned + Send + 'static,
        F: Fn(I, Arc<C>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BuiltinResult<Option<ApprovalSummary>>> + Send + 'static,
    {
        let name = self.name;
        self.describe = Some(Arc::new(
            move |validated, context| match serde_json::from_value::<I>(validated) {
                Ok(input) => Box::pin(describe(input, context)),
                Err(error) => Box::pin(async move { Err(mismatched_input(name, error)) }),
            },
        ));
        self
    }

    fn validated(&self, arguments: Map<String, Value>, context: &C) -> BuiltinResult<Value> {
        self.input
            .validate(Value::Object(arguments), context)
            .map_err(BuiltinError::Tool)
    }

    /// Check the arguments, then run the tool.
    pub async fn run(
        &self,
        arguments: Map<String, Value>,
        context: Arc<C>,
    ) -> BuiltinResult<Value> {
        let validated = self.validated(arguments, &context)?;
        (self.run)(validated, context).await
    }

    /// Check the arguments like `run` and say what the call would do,
    /// without doing it. `None` when the tool has nothing to add to its arguments.
    pub async fn describe_call(
        &self,
        arguments: Map<String, Value>,
        context: Arc<C>,
    ) -> BuiltinResult<Option<ApprovalSummary>> {
        let validated = self.validated(arguments, &context)?;
        match &self.describe {
            Some(describe) => describe(validated, context).await,
            None => Ok(None),
        }
    }
}

/// A validator let through something its tool cannot read: the two disagree,
/// which is a bug of the tool and not a mistake of the agent.
fn mismatched_input(tool: &str, error: serde_json::Error) -> BuiltinError {
    BuiltinError::internal(format!("{tool} accepted arguments it cannot read: {error}"))
}

/// A built-in MCP, apart from its sign-in.
pub struct BuiltinProvider<C> {
    pub key: &'static str,
    /// Provider name used in messages, such as "Strava".
    pub name: &'static str,
    pub tools: Vec<BuiltinTool<C>>,
    /// What the admin enters besides the sign-in. Tools read it from their context.
    pub settings: Vec<BuiltinSettingField>,
    verify: VerifyFn<C>,
    download: Option<DownloadFn<C>>,
    upload: Option<UploadFn<C>>,
}

impl<C: Send + Sync + 'static> BuiltinProvider<C> {
    /// `verify` is one cheap authenticated request proving the saved sign-in
    /// still works.
    pub fn new<F, Fut>(
        key: &'static str,
        name: &'static str,
        tools: Vec<BuiltinTool<C>>,
        verify: F,
    ) -> Self
    where
        F: Fn(Arc<C>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BuiltinResult<()>> + Send + 'static,
    {
        Self {
            key,
            name,
            tools,
            settings: Vec::new(),
            verify: Arc::new(move |context| Box::pin(verify(context))),
            download: None,
            upload: None,
        }
    }

    pub fn settings(mut self, settings: Vec<BuiltinSettingField>) -> Self {
        self.settings = settings;
        self
    }

    /// Serves a file one of the tools handed out as a temporary signed link.
    /// `reference` is what the tool put in the link, unchanged.
    pub fn download<F, Fut>(mut self, download: F) -> Self
    where
        F: Fn(Value, Arc<C>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BuiltinResult<BuiltinFile>> + Send + 'static,
    {
        self.download = Some(Arc::new(move |reference, context| {
            Box::pin(download(reference, context))
        }));
        self
    }

    /// Says where to keep the file sent to a temporary signed link one of
    /// the tools handed out. `reference` is what the tool put in the link,
    /// unchanged. Fails with [`BuiltinError::Tool`] when the link may no
    /// longer be used.
    pub fn upload<F, Fut>(mut self, upload: F) -> Self
    where
        F: Fn(Value, Arc<C>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BuiltinResult<BuiltinUploadTarget>> + Send + 'static,
    {
        self.upload = Some(Arc::new(move |reference, context| {
            Box::pin(upload(reference, context))
        }));
        self
    }

    pub fn tool(&self, name: &str) -> Option<&BuiltinTool<C>> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    pub async fn verify(&self, context: Arc<C>) -> BuiltinResult<()> {
        (self.verify)(context).await
    }

    pub fn has_download(&self) -> bool {
        self.download.is_some()
    }

    pub fn has_upload(&self) -> bool {
        self.upload.is_some()
    }

    pub async fn download_file(
        &self,
        reference: Value,
        context: Arc<C>,
    ) -> BuiltinResult<BuiltinFile> {
        match &self.download {
            Some(download) => download(reference, context).await,
            None => Err(BuiltinError::tool(format!(
                "{} has no files to download",
                self.name
            ))),
        }
    }

    pub async fn upload_target(
        &self,
        reference: Value,
        context: Arc<C>,
    ) -> BuiltinResult<BuiltinUploadTarget> {
        match &self.upload {
            Some(upload) => upload(reference, context).await,
            None => Err(BuiltinError::tool(format!("{} takes no files", self.name))),
        }
    }
}

/// A built-in MCP with its kind of sign-in. Its tools only ever run with the
/// context they were written for.
pub enum BuiltinMcpDefinition {
    Oauth {
        provider: BuiltinProvider<BuiltinToolContext>,
        oauth: BuiltinOauthConfig,
    },
    Password {
        provider: BuiltinProvider<BuiltinPasswordContext>,
        password: BuiltinPasswordConfig,
    },
}

/// What is known of a tool without its sign-in: enough to list it and to
/// decide whether a call needs approval.
#[derive(Debug, Clone)]
pub struct BuiltinToolInfo<'a> {
    pub name: &'static str,
    pub description: &'a str,
    pub input_schema: &'a Value,
    pub requires_any_scope: &'a [&'static str],
    pub write: bool,
    pub asks_approval: bool,
}

impl BuiltinMcpDefinition {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Oauth { provider, .. } => provider.key,
            Self::Password { provider, .. } => provider.key,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Oauth { provider, .. } => provider.name,
            Self::Password { provider, .. } => provider.name,
        }
    }

    pub fn oauth(&self) -> Option<&BuiltinOauthConfig> {
        match self {
            Self::Oauth { oauth, .. } => Some(oauth),
            Self::Password { .. } => None,
        }
    }

    pub fn password(&self) -> Option<&BuiltinPasswordConfig> {
        match self {
            Self::Oauth { .. } => None,
            Self::Password { password, .. } => Some(password),
        }
    }

    pub fn settings(&self) -> &[BuiltinSettingField] {
        match self {
            Self::Oauth { provider, .. } => &provider.settings,
            Self::Password { provider, .. } => &provider.settings,
        }
    }

    pub fn has_download(&self) -> bool {
        match self {
            Self::Oauth { provider, .. } => provider.has_download(),
            Self::Password { provider, .. } => provider.has_download(),
        }
    }

    pub fn has_upload(&self) -> bool {
        match self {
            Self::Oauth { provider, .. } => provider.has_upload(),
            Self::Password { provider, .. } => provider.has_upload(),
        }
    }

    /// Every tool, in the order the provider lists them.
    pub fn tools(&self) -> Vec<BuiltinToolInfo<'_>> {
        fn info<C>(tool: &BuiltinTool<C>) -> BuiltinToolInfo<'_> {
            BuiltinToolInfo {
                name: tool.name,
                description: &tool.description,
                input_schema: &tool.input_schema,
                requires_any_scope: &tool.requires_any_scope,
                write: tool.write,
                asks_approval: tool.asks_approval,
            }
        }
        match self {
            Self::Oauth { provider, .. } => provider.tools.iter().map(info).collect(),
            Self::Password { provider, .. } => provider.tools.iter().map(info).collect(),
        }
    }

    pub fn tool(&self, name: &str) -> Option<BuiltinToolInfo<'_>> {
        self.tools().into_iter().find(|tool| tool.name == name)
    }

    /// The JSON Schema each tool's validator enforces, by tool name, to
    /// compare with the schemas the tools advertise.
    pub fn enforced_schemas(&self) -> Vec<(&'static str, Value)> {
        match self {
            Self::Oauth { provider, .. } => provider
                .tools
                .iter()
                .map(|tool| (tool.name, tool.input.json_schema()))
                .collect(),
            Self::Password { provider, .. } => provider
                .tools
                .iter()
                .map(|tool| (tool.name, tool.input.json_schema()))
                .collect(),
        }
    }
}

/// The built-in MCPs of an instance, by key. The server registers the three
/// providers; a test registers what it needs.
#[derive(Clone, Default)]
pub struct BuiltinRegistry {
    definitions: Arc<Vec<Arc<BuiltinMcpDefinition>>>,
}

impl BuiltinRegistry {
    pub fn new(definitions: Vec<BuiltinMcpDefinition>) -> Self {
        Self {
            definitions: Arc::new(definitions.into_iter().map(Arc::new).collect()),
        }
    }

    /// The definition for a `builtin_key` column, or `None` for a key no
    /// registered provider has.
    pub fn get(&self, key: Option<&str>) -> Option<&Arc<BuiltinMcpDefinition>> {
        let key = key?;
        self.definitions
            .iter()
            .find(|definition| definition.key() == key)
    }

    pub fn keys(&self) -> Vec<&'static str> {
        self.definitions
            .iter()
            .map(|definition| definition.key())
            .collect()
    }
}

impl std::fmt::Debug for BuiltinRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_list().entries(self.keys()).finish()
    }
}
