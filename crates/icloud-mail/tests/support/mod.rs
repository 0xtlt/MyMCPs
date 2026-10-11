//! What the tests of the tools share: an instance, the account behind the
//! fake servers, and MCPs signed in to it.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use bytes::Bytes;
use mymcps_builtin::{
    BuiltinEnv, BuiltinError, BuiltinMcpDefinition, BuiltinPasswordContext, BuiltinProvider,
    BuiltinResult, BuiltinUploadTarget,
};
use mymcps_core::TestCore;
use mymcps_icloud_mail::testing::{FakeIcloud, FakeOptions, PASSWORD, USERNAME};
use mymcps_icloud_mail::{HtmlConversion, MailServers};
use serde_json::Value;

pub const PERMISSIONS: [&str; 4] = ["read", "draft", "send", "organize"];

pub struct Scenario {
    pub core: TestCore,
    pub icloud: FakeIcloud,
    definition: BuiltinMcpDefinition,
    env: BuiltinEnv,
    next_mcp_id: AtomicI64,
}

impl Scenario {
    pub async fn new() -> Self {
        Self::with(FakeOptions::default()).await
    }

    pub async fn with(options: FakeOptions) -> Self {
        Self::on(TestCore::new().await, options).await
    }

    pub async fn on(core: TestCore, options: FakeOptions) -> Self {
        let icloud = FakeIcloud::start_with(options).await;
        let env = BuiltinEnv::new(core.core.clone()).with_extension(icloud.servers());
        Self {
            core,
            icloud,
            definition: mymcps_icloud_mail::definition(),
            env,
            next_mcp_id: AtomicI64::new(1),
        }
    }

    /// Shorten the time an HTML message may take to convert, for the MCPs made from now on.
    pub fn html_timeout(&mut self, timeout: std::time::Duration) {
        self.env = self.env.clone().with_extension(HtmlConversion { timeout });
    }

    pub fn provider(&self) -> &BuiltinProvider<BuiltinPasswordContext> {
        match &self.definition {
            BuiltinMcpDefinition::Password { provider, .. } => provider,
            BuiltinMcpDefinition::Oauth { .. } => panic!("iCloud Mail signs in with a password"),
        }
    }

    pub fn definition(&self) -> &BuiltinMcpDefinition {
        &self.definition
    }

    /// A built-in iCloud Mail MCP as the setup form leaves it.
    pub fn mcp(&self, permissions: &[&str]) -> Arc<BuiltinPasswordContext> {
        self.mcp_with_aliases(permissions, &[])
    }

    pub fn mcp_with_aliases(
        &self,
        permissions: &[&str],
        aliases: &[&str],
    ) -> Arc<BuiltinPasswordContext> {
        Arc::new(BuiltinPasswordContext {
            env: self.env.clone(),
            mcp_id: self.next_mcp_id.fetch_add(1, Ordering::Relaxed),
            username: USERNAME.to_owned(),
            password: PASSWORD.to_owned(),
            permissions: permissions
                .iter()
                .map(|permission| (*permission).to_owned())
                .collect(),
            aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
            settings: BTreeMap::new(),
        })
    }

    /// An MCP that reaches other servers than the fake ones, such as one that cannot be found.
    pub fn mcp_on(
        &self,
        permissions: &[&str],
        servers: MailServers,
    ) -> Arc<BuiltinPasswordContext> {
        let mut mcp = (*self.mcp(permissions)).clone();
        mcp.env = mcp.env.with_extension(servers);
        Arc::new(mcp)
    }

    pub fn full_access(&self) -> Arc<BuiltinPasswordContext> {
        self.mcp(&PERMISSIONS)
    }

    /// Call a tool the way the runtime does once it allowed the call.
    pub async fn call(
        &self,
        mcp: &Arc<BuiltinPasswordContext>,
        tool: &str,
        arguments: Value,
    ) -> BuiltinResult<Value> {
        let arguments = arguments
            .as_object()
            .cloned()
            .expect("arguments are an object");
        self.provider()
            .tool(tool)
            .unwrap_or_else(|| panic!("no tool {tool}"))
            .run(arguments, mcp.clone())
            .await
    }

    /// The data a call returns.
    pub async fn data(
        &self,
        mcp: &Arc<BuiltinPasswordContext>,
        tool: &str,
        arguments: Value,
    ) -> Value {
        self.call(mcp, tool, arguments)
            .await
            .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
    }

    /// The sentence the agent reads when a call fails.
    pub async fn refusal(
        &self,
        mcp: &Arc<BuiltinPasswordContext>,
        tool: &str,
        arguments: Value,
    ) -> String {
        match self.call(mcp, tool, arguments).await {
            Ok(data) => panic!("{tool} succeeded: {data}"),
            Err(error) => {
                assert!(
                    error.is_tool_error(),
                    "{tool} failed with an error the agent is not told: {error}"
                );
                error.to_string()
            }
        }
    }

    /// Keep a file the way its upload link would, for the tests that are not about the link.
    pub async fn upload_file(
        &self,
        mcp: &BuiltinPasswordContext,
        filename: &str,
        content: impl Into<Vec<u8>>,
        content_type: Option<&str>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let target = BuiltinUploadTarget {
            id: id.clone(),
            filename: filename.to_owned(),
            content_type: content_type.map(str::to_owned),
            max_bytes: 20_000_000,
        };
        let body = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from(content.into()))]);
        self.env
            .uploads
            .save(mcp.mcp_id, &target, body)
            .await
            .expect("the upload is kept");
        id
    }
}

/// The text of an error, whichever kind it is.
pub fn message(error: BuiltinError) -> String {
    error.to_string()
}

/// Equal, keys in the same order: the order is what an agent reads.
#[track_caller]
pub fn assert_same_json(actual: &Value, expected: &Value) {
    assert_eq!(actual, expected);
    assert_eq!(
        actual.to_string(),
        expected.to_string(),
        "the keys are in another order"
    );
}

/// The values of one key of each object of a list.
pub fn pluck(list: &Value, key: &str) -> Vec<Value> {
    list.as_array()
        .into_iter()
        .flatten()
        .map(|entry| entry[key].clone())
        .collect()
}
