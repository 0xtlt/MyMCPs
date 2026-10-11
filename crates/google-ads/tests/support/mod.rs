//! What the tests of the Google Ads MCP share: a connected MCP in front of a
//! fake Google, and a Google that answers from a script.
//!
//! In the TypeScript tests a tool was called through the gateway, which
//! loaded the MCP from the database. Here a test calls the tool itself, with
//! the context the runtime would have built: `Google::new(fake).await`, then
//! `google.call("list_campaigns", json!({ ... }), &[("loginCustomerId", "...")])`.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use http::StatusCode;
use mymcps_builtin::{
    ApprovalSummary, BuiltinEnv, BuiltinMcpDefinition, BuiltinProvider, BuiltinResult,
    BuiltinToolContext,
};
use mymcps_core::TestCore;
use mymcps_google_ads::testing::{FakeGoogleAds, google_ads_context};
use mymcps_net::{CannedResponse, Fetcher, SentRequest};
use serde_json::Value;

/// The tools and hooks of a definition: Google Ads signs in with OAuth.
pub fn provider(definition: &BuiltinMcpDefinition) -> &BuiltinProvider<BuiltinToolContext> {
    match definition {
        BuiltinMcpDefinition::Oauth { provider, .. } => provider,
        BuiltinMcpDefinition::Password { .. } => panic!("Google Ads signs in with OAuth"),
    }
}

/// A connected Google Ads MCP whose requests reach a fake Google.
pub struct Google {
    pub fake: FakeGoogleAds,
    pub env: BuiltinEnv,
    pub definition: BuiltinMcpDefinition,
    pub core: TestCore,
}

impl Google {
    /// `Google::new(FakeGoogleAds::new())` for a Google that gives its default answers.
    pub async fn new(fake: FakeGoogleAds) -> Self {
        let core = TestCore::new().await;
        let env = BuiltinEnv::new(core.core.clone()).with_fetcher(fake.fetcher());
        Self {
            fake,
            env,
            definition: mymcps_google_ads::definition(),
            core,
        }
    }

    /// What a tool runs with. `settings` is what the admin entered for the MCP.
    pub fn context(&self, settings: &[(&str, &str)]) -> Arc<BuiltinToolContext> {
        google_ads_context(self.env.clone(), settings)
    }

    /// Call a tool as the runtime does, once it has checked that the MCP may.
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
            self.context(settings),
        )
        .await
    }

    /// What a call would do, as the person asked to approve it reads it. The
    /// runtime asks for it before it holds a call, and does not run the tool.
    pub async fn describe(
        &self,
        tool: &str,
        arguments: Value,
        settings: &[(&str, &str)],
    ) -> BuiltinResult<Option<ApprovalSummary>> {
        let tool = provider(&self.definition)
            .tool(tool)
            .unwrap_or_else(|| panic!("no tool {tool}"));
        tool.describe_call(
            arguments.as_object().cloned().unwrap_or_default(),
            self.context(settings),
        )
        .await
    }
}

/// A Google that gives the answers of a script, one for each request, in
/// order, and remembers what it was asked.
pub struct ScriptedGoogle {
    answers: Arc<Mutex<VecDeque<CannedResponse>>>,
    requests: Arc<Mutex<Vec<SentRequest>>>,
    pub fetcher: Fetcher,
}

impl ScriptedGoogle {
    /// `answers` is a list of `{ "status": 200, "body": { ... } }`, or of
    /// `{ "status": 502, "text": "<html>" }` for a body that is not JSON.
    pub fn new(answers: &[Value]) -> Self {
        let answers: VecDeque<CannedResponse> = answers
            .iter()
            .map(|answer| {
                let status =
                    StatusCode::from_u16(answer["status"].as_u64().unwrap() as u16).unwrap();
                match answer.get("text") {
                    Some(text) => {
                        CannedResponse::new(status).body(text.as_str().unwrap().to_owned())
                    }
                    None => CannedResponse::json(status, &answer["body"]),
                }
            })
            .collect();
        let answers = Arc::new(Mutex::new(answers));
        let requests: Arc<Mutex<Vec<SentRequest>>> = Arc::default();
        let fetcher = Fetcher::offline().answering({
            let (answers, requests) = (answers.clone(), requests.clone());
            move |request| {
                requests.lock().unwrap().push(request.clone());
                answers.lock().unwrap().pop_front()
            }
        });
        Self {
            answers,
            requests,
            fetcher,
        }
    }

    pub fn requests(&self) -> Vec<SentRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// How many answers of the script nothing asked for.
    pub fn unasked(&self) -> usize {
        self.answers.lock().unwrap().len()
    }
}
