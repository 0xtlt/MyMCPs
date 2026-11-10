//! What the tests share: a connected MCP in front of a fake Search Console.

#![allow(dead_code)]

use mymcps_builtin::{
    BuiltinEnv, BuiltinMcpDefinition, BuiltinProvider, BuiltinResult, BuiltinToolContext,
};
use mymcps_core::TestCore;
use mymcps_google_search_console::testing::{FakeSearchConsole, search_console_context};
use serde_json::Value;

pub fn provider(definition: &BuiltinMcpDefinition) -> &BuiltinProvider<BuiltinToolContext> {
    match definition {
        BuiltinMcpDefinition::Oauth { provider, .. } => provider,
        BuiltinMcpDefinition::Password { .. } => panic!("Search Console signs in with OAuth"),
    }
}

/// A connected Search Console MCP whose requests reach a fake Google.
pub struct Google {
    pub fake: FakeSearchConsole,
    pub env: BuiltinEnv,
    pub definition: BuiltinMcpDefinition,
    pub core: TestCore,
}

impl Google {
    pub async fn new(fake: FakeSearchConsole) -> Self {
        let core = TestCore::new().await;
        let env = BuiltinEnv::new(core.core.clone()).with_fetcher(fake.fetcher());
        Self {
            fake,
            env,
            definition: mymcps_google_search_console::definition(),
            core,
        }
    }

    /// Call a tool as the runtime does. `settings` is what the admin entered.
    pub async fn call(
        &self,
        tool: &str,
        arguments: Value,
        settings: &[(&str, &str)],
    ) -> BuiltinResult<Value> {
        let tool = provider(&self.definition)
            .tool(tool)
            .unwrap_or_else(|| panic!("no tool {tool}"));
        tool.run(
            arguments.as_object().cloned().unwrap_or_default(),
            search_console_context(self.env.clone(), settings),
        )
        .await
    }

    /// The message of a call that fails as a tool error.
    pub async fn refusal(&self, tool: &str, arguments: Value, settings: &[(&str, &str)]) -> String {
        let error = self.call(tool, arguments, settings).await.unwrap_err();
        assert!(error.is_tool_error(), "{error}");
        error.to_string()
    }
}
