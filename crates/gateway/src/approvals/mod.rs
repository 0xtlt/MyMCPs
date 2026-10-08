//! Tool calls that wait for a person: the gate a call goes through before
//! it runs, the requests it leaves for the Approvals pages, and what those
//! pages read back and decide.

mod summary;

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mymcps_core::crypto::{random_base64url, sha256_hex};
use mymcps_core::models::{
    AccessToken, ApprovalDecision, ApprovalRequest, CallErrorCategory, Mcp, McpTransport, User,
};
use mymcps_core::redaction::sanitize_diagnostic;
use mymcps_core::{Core, Timestamp};
use mymcps_upstream::approvals::{ToolApprovalMode, tool_approval_mode};
use mymcps_upstream::{Upstream, UpstreamError};
use mymcps_vine::js::json_stringify;
use serde_json::{Map, Value, json};
use sqlx::{QueryBuilder, Sqlite};

use crate::validators::approvals::SAVED_APPROVAL_SUMMARY;

pub use summary::{ArgumentDetails, SavedApprovalSummary, argument_details, arguments_summary};

/// How long a person has to decide.
pub const APPROVAL_PENDING_HOURS: i64 = 24;
/// How long the agent has to run a call once it is approved.
pub const APPROVAL_GRANT_HOURS: i64 = 24;

/// An agent cannot bury a request under others, nor fill the table.
const MAX_PENDING_PER_TOKEN: i64 = 20;
/// Kept encrypted and shown whole on the page of the request.
const MAX_ARGUMENT_BYTES: usize = 256 * 1024;
/// Decided and expired requests stay listed this long.
const KEPT_DAYS: i64 = 30;
const PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Whether a call may run now.
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalGate {
    Open,
    Held(HeldCall),
}

impl ApprovalGate {
    pub fn is_held(&self) -> bool {
        matches!(self, Self::Held(_))
    }

    pub fn held(&self) -> Option<&HeldCall> {
        match self {
            Self::Held(held) => Some(held),
            Self::Open => None,
        }
    }
}

/// A held call comes with what the agent is told and how the call log files it.
#[derive(Debug, Clone, PartialEq)]
pub struct HeldCall {
    /// `ApprovalRequired`, `ApprovalDenied` or `ToolError`.
    pub category: CallErrorCategory,
    pub reason: &'static str,
    /// The result of the call: one text, with `isError`.
    pub result: Value,
}

impl HeldCall {
    /// The text the agent reads.
    pub fn text(&self) -> &str {
        self.result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// A tool call on its way through the gate.
#[derive(Debug)]
pub struct GatedCall<'a> {
    pub access_token: &'a AccessToken,
    /// Read again when asking the MCP about the call renewed its OAuth
    /// access token: the call then runs with the row that was read.
    pub mcp: &'a mut Mcp,
    pub tool_name: &'a str,
    pub args: Option<&'a Map<String, Value>>,
}

/// What a person decides of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Deny,
}

impl Decision {
    /// The `decision` the page submits.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "approve" => Some(Self::Approve),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// Why the gate could not say whether a call may run. The gateway answers
/// such a call as one that failed.
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    /// The MCP could not be asked about the call it would have to run.
    #[error(transparent)]
    Upstream(#[from] UpstreamError),
}

/// The call cannot be put to a person, and the agent is told why.
struct ApprovalRefusal(String);

enum RequestError {
    Refused(ApprovalRefusal),
    Failed(ApprovalError),
}

impl From<ApprovalRefusal> for RequestError {
    fn from(refusal: ApprovalRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<sqlx::Error> for RequestError {
    fn from(error: sqlx::Error) -> Self {
        Self::Failed(error.into())
    }
}

impl From<UpstreamError> for RequestError {
    /// What a built-in MCP says to the agent about a call it would refuse
    /// is said now, so that nobody is asked to approve that call.
    fn from(error: UpstreamError) -> Self {
        if error.is_builtin_tool_error() {
            Self::Refused(ApprovalRefusal(error.to_string()))
        } else {
            Self::Failed(error.into())
        }
    }
}

fn sorted(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        Value::Object(entries) => {
            // `Array.prototype.sort` orders strings by their UTF-16 code units.
            let mut keys: Vec<&String> = entries.keys().collect();
            keys.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), sorted(&entries[key])))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// Identifies the arguments of a call whatever order their keys came in. An
/// approval is for one hash: any other value in the arguments is another call.
pub fn arguments_hash(args: Option<&Map<String, Value>>) -> String {
    let mut arguments = args.map_or_else(
        || Value::Object(Map::new()),
        |args| sorted(&Value::Object(args.clone())),
    );
    // An object lists its integer-like keys first, however it was built:
    // the hashes the Node app stored were made of that order.
    mymcps_mcp::json::js_key_order(&mut arguments);
    sha256_hex(&json_stringify(&arguments))
}

fn held(category: CallErrorCategory, reason: &'static str, text: String) -> ApprovalGate {
    ApprovalGate::Held(HeldCall {
        category,
        reason,
        result: json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
    })
}

/// The calls of one access token that are at the gate, which they pass one
/// at a time.
type Turns = Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>;

/// A place in the line of one access token. The line is forgotten when the
/// last call leaves it.
struct Queued<'a> {
    turns: &'a Turns,
    access_token_id: i64,
    line: Arc<tokio::sync::Mutex<()>>,
}

impl<'a> Queued<'a> {
    fn join(turns: &'a Turns, access_token_id: i64) -> Self {
        let line = turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(access_token_id)
            .or_default()
            .clone();
        Self {
            turns,
            access_token_id,
            line,
        }
    }
}

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        let mut turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
        // A call joins the line while holding the lock taken above, so the
        // count cannot grow under this check: two is the map and this call.
        if Arc::strong_count(&self.line) == 2 {
            turns.remove(&self.access_token_id);
        }
    }
}

struct Inner {
    core: Arc<Core>,
    upstream: Arc<Upstream>,
    turns: Turns,
    last_pruned_at: Mutex<Option<Instant>>,
}

/// The approvals of one instance: the line each access token waits in at the
/// gate, and when requests were last pruned.
#[derive(Clone)]
pub struct ApprovalService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ApprovalService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApprovalService")
            .finish_non_exhaustive()
    }
}

impl ApprovalService {
    /// The approvals of the instance `upstream` works on. The gate asks the
    /// upstream which tools ask, and what a call would do.
    pub fn new(upstream: Arc<Upstream>) -> Self {
        Self {
            inner: Arc::new(Inner {
                core: upstream.core().clone(),
                upstream,
                turns: Mutex::default(),
                last_pruned_at: Mutex::new(None),
            }),
        }
    }

    fn core(&self) -> &Core {
        &self.inner.core
    }

    /// The page where a person reads and decides a request. `None` without `APP_URL`.
    pub fn url(&self, request: &ApprovalRequest) -> Option<String> {
        let app_url = self.core().config.public_oauth_app_url()?;
        Some(format!("{app_url}/approvals/{}", request.public_id))
    }

    /// Decide whether a tool call runs now. A tool set to ask is held until a
    /// person approved this very call: same token, MCP, tool and arguments. The
    /// approval is spent by the call it lets through.
    pub async fn gate(&self, mut call: GatedCall<'_>) -> Result<ApprovalGate, ApprovalError> {
        let mode = tool_approval_mode(self.inner.upstream.builtins(), call.mcp, call.tool_name);
        if mode == ToolApprovalMode::Auto {
            return Ok(ApprovalGate::Open);
        }
        let access_token_id = call.access_token.id;
        self.in_turn(access_token_id, self.decided(&mut call)).await
    }

    // The gateway serves parallel requests. A call that asks first reads what
    // is already waiting, then adds to it: two calls of the same access token
    // doing that at once would both add, past the limit, or twice for the
    // same call.
    /// Run `task` once the earlier tasks of the same access token have finished.
    async fn in_turn<T>(&self, access_token_id: i64, task: impl Future<Output = T>) -> T {
        let queued = Queued::join(&self.inner.turns, access_token_id);
        let _turn = queued.line.lock().await;
        task.await
    }

    /// What the last request for this very call says, or a new request for it.
    async fn decided(&self, call: &mut GatedCall<'_>) -> Result<ApprovalGate, ApprovalError> {
        let hash = arguments_hash(call.args);
        let latest: Option<ApprovalRequest> = sqlx::query_as(
            "select * from `approval_requests` \
             where `access_token_id` = ? and `mcp_id` = ? and `tool_name` = ? \
             and `arguments_hash` = ? and `consumed_at` is null and `expires_at` > ? \
             order by `id` desc limit 1",
        )
        .bind(call.access_token.id)
        .bind(call.mcp.id)
        .bind(call.tool_name)
        .bind(&hash)
        .bind(Timestamp::now())
        .fetch_optional(&*self.core().db)
        .await?;

        if let Some(latest) = &latest {
            match latest.status {
                ApprovalDecision::Approved => {
                    if self.consume(latest).await? {
                        return Ok(ApprovalGate::Open);
                    }
                }
                ApprovalDecision::Denied => {
                    // Said once. The same call made again is a new request, for the day
                    // the person changes their mind.
                    self.consume(latest).await?;
                    return Ok(held(
                        CallErrorCategory::ApprovalDenied,
                        "A person denied this call",
                        format!(
                            "Denied: a person refused this call to {} on {} in MyMCPs, and it was not run. Do not make it again unless they ask you to.",
                            call.tool_name, call.mcp.name
                        ),
                    ));
                }
                ApprovalDecision::Pending => return Ok(self.waiting(latest, call, false)),
            }
        }

        match self.request(call, hash).await {
            Ok(request) => Ok(self.waiting(&request, call, true)),
            Err(RequestError::Refused(ApprovalRefusal(message))) => Ok(held(
                CallErrorCategory::ToolError,
                "The call was refused before asking for approval",
                message,
            )),
            Err(RequestError::Failed(error)) => Err(error),
        }
    }

    fn waiting(
        &self,
        request: &ApprovalRequest,
        call: &GatedCall<'_>,
        is_new: bool,
    ) -> ApprovalGate {
        let (tool_name, mcp_name) = (call.tool_name, &call.mcp.name);
        let Some(url) = self.url(request) else {
            return held(
                CallErrorCategory::ApprovalRequired,
                "Approval links need APP_URL",
                format!(
                    "{tool_name} on {mcp_name} needs the approval of a person, and it was not run. There is no link to give them, because this MyMCPs instance does not know its public address (APP_URL): ask them to open the Approvals page of MyMCPs and decide the call there, then call {tool_name} again with exactly the same arguments."
                ),
            );
        };

        let (reason, opening) = if is_new {
            ("Waiting for approval", "Approval required")
        } else {
            ("Still waiting for approval", "Still waiting for approval")
        };
        let expires_at = request
            .expires_at
            .as_datetime()
            .format("%Y-%m-%dT%H:%M:%SZ");
        held(
            CallErrorCategory::ApprovalRequired,
            reason,
            [
                format!("{opening}: {tool_name} on {mcp_name} was not run."),
                format!(
                    "A person has to approve this exact call in MyMCPs first. Give them this link: {url}"
                ),
                format!(
                    "They sign in, read what the call would do, and approve or deny it. The link works until {expires_at}."
                ),
                format!(
                    "Once they have approved, call {tool_name} again with exactly the same arguments and it runs. Other arguments are another call, which needs its own approval."
                ),
            ]
            .join("\n"),
        )
    }

    /// Put a call to a person. Fails when it is not one to put to them.
    async fn request(
        &self,
        call: &mut GatedCall<'_>,
        arguments_hash: String,
    ) -> Result<ApprovalRequest, RequestError> {
        let (tool_name, mcp_name) = (call.tool_name, call.mcp.name.clone());
        let serialized = json_stringify(&Value::Object(call.args.cloned().unwrap_or_default()));
        if serialized.len() > MAX_ARGUMENT_BYTES {
            return Err(ApprovalRefusal(format!(
                "{tool_name} on {mcp_name} needs the approval of a person, who cannot be shown arguments of more than {} KB. Make the call smaller.",
                MAX_ARGUMENT_BYTES / 1024
            ))
            .into());
        }

        let core = self.core();
        let now = Timestamp::now();
        let waiting: i64 = sqlx::query_scalar(
            "select count(*) from `approval_requests` \
             where `access_token_id` = ? and `status` = 'pending' and `expires_at` > ?",
        )
        .bind(call.access_token.id)
        .bind(now)
        .fetch_one(&*core.db)
        .await?;
        if waiting >= MAX_PENDING_PER_TOKEN {
            return Err(ApprovalRefusal(format!(
                "{MAX_PENDING_PER_TOKEN} calls of this access token already wait for approval in MyMCPs. Ask the person to decide them before asking for more."
            ))
            .into());
        }

        let summary = self.summarize(call).await?;
        let summary = serde_json::to_value(&summary)
            .map(|summary| json_stringify(&summary))
            .unwrap_or_default();
        let mut request = ApprovalRequest {
            public_id: random_base64url(24),
            mcp_id: call.mcp.id,
            access_token_id: call.access_token.id,
            tool_name: tool_name.to_owned(),
            arguments: core.encrypt_secret(Some(&serialized)).unwrap_or_default(),
            arguments_hash,
            summary: core.encrypt_secret(Some(&summary)).unwrap_or_default(),
            status: ApprovalDecision::Pending,
            expires_at: now + chrono::Duration::hours(APPROVAL_PENDING_HOURS),
            ..Default::default()
        };
        request.insert(&*core.db).await?;
        Ok(request)
    }

    /// Read the call for the person who decides. A built-in tool checks the
    /// arguments and describes the change itself. For any other tool the
    /// arguments are listed as they are, beside what its MCP says the tool does.
    async fn summarize(
        &self,
        call: &mut GatedCall<'_>,
    ) -> Result<SavedApprovalSummary, RequestError> {
        let upstream = &self.inner.upstream;
        if call.mcp.transport == McpTransport::Builtin {
            let described = upstream
                .describe_builtin_call(call.mcp, call.tool_name, call.args.cloned())
                .await
                .map_err(UpstreamError::from)?;
            if let Some(described) = described {
                return Ok(SavedApprovalSummary::interpreted(described));
            }
            let definition = upstream
                .require_builtin_mcp(call.mcp)
                .map_err(UpstreamError::from)?;
            let tool = definition.tool(call.tool_name);
            return Ok(arguments_summary(
                call.mcp,
                call.tool_name,
                call.args,
                tool.as_ref().map(|tool| tool.description),
            ));
        }

        let tools = upstream.probe(call.mcp).await?;
        let Some(tool) = tools.iter().find(|tool| tool.name == call.tool_name) else {
            return Err(ApprovalRefusal(format!(
                "{} has no tool named \"{}\"",
                call.mcp.name, call.tool_name
            ))
            .into());
        };
        Ok(arguments_summary(
            call.mcp,
            call.tool_name,
            call.args,
            tool.description.as_deref(),
        ))
    }

    /// Mark a decided request as acted on by the agent. Only one of two calls
    /// made at the same moment gets the approval.
    async fn consume(&self, request: &ApprovalRequest) -> Result<bool, sqlx::Error> {
        let consumed = sqlx::query(
            "update `approval_requests` set `consumed_at` = ? \
             where `id` = ? and `status` = ? and `consumed_at` is null",
        )
        .bind(Timestamp::now())
        .bind(request.id)
        .bind(request.status)
        .execute(&*self.core().db)
        .await?;
        Ok(consumed.rows_affected() == 1)
    }

    /// Record what a person decided. False when the request was decided in the
    /// meantime or has expired. An approval gives the agent a day to run the call.
    pub async fn decide(
        &self,
        request: &ApprovalRequest,
        decision: Decision,
        user: &User,
    ) -> Result<bool, sqlx::Error> {
        let now = Timestamp::now();
        let mut update = QueryBuilder::<Sqlite>::new("update `approval_requests` set `status` = ");
        update
            .push_bind(match decision {
                Decision::Approve => ApprovalDecision::Approved,
                Decision::Deny => ApprovalDecision::Denied,
            })
            .push(", `decided_by` = ")
            .push_bind(user.id)
            .push(", `decided_at` = ")
            .push_bind(now)
            .push(", `updated_at` = ")
            .push_bind(now);
        if decision == Decision::Approve {
            update
                .push(", `expires_at` = ")
                .push_bind(now + chrono::Duration::hours(APPROVAL_GRANT_HOURS));
        }
        update
            .push(" where `id` = ")
            .push_bind(request.id)
            .push(" and `status` = 'pending' and `expires_at` > ")
            .push_bind(now);
        let decided = update.build().execute(&*self.core().db).await?;
        Ok(decided.rows_affected() == 1)
    }

    /// The arguments of the call, exactly as the agent sent them.
    pub fn arguments(&self, request: &ApprovalRequest) -> Option<Value> {
        let serialized = self.core().decrypt_secret(Some(&request.arguments))?;
        mymcps_mcp::json::parse(&serialized).ok()
    }

    /// The arguments of the call as the page of a request shows them: the
    /// same text for everyone, two spaces to a level.
    pub fn arguments_text(&self, request: &ApprovalRequest) -> Option<String> {
        self.arguments(request)
            .filter(|arguments| !arguments.is_null())
            .map(|arguments| mymcps_mcp::json::to_string_pretty(&arguments))
    }

    /// `None` when the summary can no longer be read, after `APP_KEY` changed for instance.
    pub fn summary(&self, request: &ApprovalRequest) -> Option<SavedApprovalSummary> {
        let serialized = self.core().decrypt_secret(Some(&request.summary))?;
        let parsed: Value = serde_json::from_str(&serialized).ok()?;
        SAVED_APPROVAL_SUMMARY.validate_as(&parsed).ok()
    }

    /// The requests a user may read and decide: an administrator all of them, a
    /// member the ones their own access tokens made. A request shows the
    /// arguments of a call, which the call log only shows to administrators.
    ///
    /// The query selects the rows of `approval_requests` and ends inside its
    /// `where`: a caller narrows it with `.push(" and ...")`, then orders and
    /// limits it.
    pub fn visible_to(user: &User) -> QueryBuilder<Sqlite> {
        Self::visible_rows("select `approval_requests`.*", user)
    }

    fn visible_rows(select: &str, user: &User) -> QueryBuilder<Sqlite> {
        let mut query = QueryBuilder::<Sqlite>::new(select);
        query.push(" from `approval_requests` where ");
        if user.is_admin() {
            query.push("1 = 1");
        } else {
            query
                .push(
                    "exists (select 1 from `access_tokens` \
                     where `access_tokens`.`id` = `approval_requests`.`access_token_id` \
                     and `access_tokens`.`created_by` = ",
                )
                .push_bind(user.id)
                .push(")");
        }
        query
    }

    /// How many requests wait for a decision this user can make.
    pub async fn pending_count(&self, user: &User) -> Result<i64, sqlx::Error> {
        let mut query = Self::visible_rows("select count(*)", user);
        query
            .push(" and `status` = 'pending' and `expires_at` > ")
            .push_bind(Timestamp::now());
        query.build_query_scalar().fetch_one(&*self.core().db).await
    }

    /// Delete the requests nobody can act on any more, a month after they
    /// ended. Runs at most once an hour unless `force` is set.
    pub async fn prune_expired(&self, force: bool) {
        {
            let mut last_pruned_at = self
                .inner
                .last_pruned_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !force && last_pruned_at.is_some_and(|at| at.elapsed() < PRUNE_INTERVAL) {
                return;
            }
            *last_pruned_at = Some(Instant::now());
        }

        let pruned = sqlx::query("delete from `approval_requests` where `expires_at` < ?")
            .bind(Timestamp::now() - chrono::Duration::days(KEPT_DAYS))
            .execute(&*self.core().db)
            .await;
        if let Err(error) = pruned {
            tracing::warn!(
                error = %sanitize_diagnostic(&error.to_string()),
                "Expired approval requests could not be pruned"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use mymcps_core::TestCore;
    use tokio::sync::Notify;

    use super::*;

    async fn service() -> (ApprovalService, TestCore) {
        let core = TestCore::new().await;
        let upstream = Upstream::builder(core.core.clone(), Default::default()).build();
        (ApprovalService::new(upstream), core)
    }

    fn lines(service: &ApprovalService) -> usize {
        service.inner.turns.lock().unwrap().len()
    }

    #[tokio::test]
    async fn runs_the_tasks_of_an_access_token_one_at_a_time_in_the_order_they_came() {
        let (service, _core) = service().await;
        let events = Mutex::new(Vec::new());
        let task = |name: u32| {
            let events = &events;
            service.in_turn(7, async move {
                events.lock().unwrap().push(format!("{name} starts"));
                // Long enough for the others to run, if they could.
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
                events.lock().unwrap().push(format!("{name} ends"));
                name
            })
        };

        let results = tokio::join!(task(1), task(2), task(3), task(4));

        assert_eq!(results, (1, 2, 3, 4));
        assert_eq!(
            *events.lock().unwrap(),
            [
                "1 starts", "1 ends", "2 starts", "2 ends", "3 starts", "3 ends", "4 starts",
                "4 ends",
            ]
        );
        // Nothing is kept of a line once it is empty.
        assert_eq!(lines(&service), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn never_runs_two_tasks_of_an_access_token_at_once() {
        let (service, _core) = service().await;
        // For each of two access tokens: how many of its tasks run, and the
        // most that ever ran at once.
        let counters: Arc<[(AtomicUsize, AtomicUsize); 2]> = Arc::default();

        let tasks: Vec<_> = (0..64_usize)
            .map(|index| {
                let (service, counters) = (service.clone(), counters.clone());
                tokio::spawn(async move {
                    let token = index % 2;
                    service
                        .in_turn(token as i64, async {
                            let (running, most_at_once) = &counters[token];
                            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                            most_at_once.fetch_max(now, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_micros(200)).await;
                            running.fetch_sub(1, Ordering::SeqCst);
                        })
                        .await;
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }

        for (_, most_at_once) in counters.iter() {
            assert_eq!(most_at_once.load(Ordering::SeqCst), 1);
        }
        assert_eq!(lines(&service), 0);
    }

    #[tokio::test]
    async fn lets_the_tasks_of_another_access_token_run_meanwhile() {
        let (service, _core) = service().await;
        let release = Notify::new();
        let entered = Notify::new();

        let waiting = service.in_turn(1, async {
            entered.notify_one();
            release.notified().await;
            "first"
        });
        let other = async {
            entered.notified().await;
            // The line of token 1 is held, and token 2 does not wait in it.
            let answer = service.in_turn(2, async { "other" }).await;
            assert_eq!(lines(&service), 1);
            release.notify_one();
            answer
        };

        assert_eq!(tokio::join!(waiting, other), ("first", "other"));
        assert_eq!(lines(&service), 0);
    }

    #[tokio::test]
    async fn gives_the_turn_to_the_next_task_when_one_is_given_up() {
        let (service, _core) = service().await;

        // A task that never ends, dropped while it holds the turn.
        let stuck = service.in_turn(1, std::future::pending::<()>());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), stuck)
                .await
                .is_err()
        );

        let next = tokio::time::timeout(Duration::from_secs(5), service.in_turn(1, async { 2 }));
        assert_eq!(next.await.unwrap(), 2);
        assert_eq!(lines(&service), 0);
    }
}
