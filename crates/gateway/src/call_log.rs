//! The log of the tool calls agents make through the gateway.
//!
//! A call is recorded after it was answered: [`McpCallLogService::record`]
//! only queues the write, so that a slow or failing database never delays or
//! alters the answer an agent gets. What a record keeps of the call is
//! decided by the capture level of the instance when it is written.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mymcps_core::models::{
    AccessToken, CallErrorCategory, CallOutcome, InstanceSetting, Mcp, McpCallLog, McpLogLevel,
};
use mymcps_core::redaction::{sanitize_diagnostic, sanitize_mcp_diagnostic};
use mymcps_core::{Core, Timestamp};
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::js::slice_utf16;
use crate::validators::mcp_call_log::LOGGED_MCP_SLUG;

const PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// How many records may wait to be written. Past that, calls are answered
/// faster than the database stores them, and new records are dropped.
pub const MAX_PENDING_WRITES: usize = 1_000;

/// Records written in one transaction. One by one, each record paid for a
/// transaction of its own, and a burst of calls filled the queue faster than
/// it was written down.
const WRITE_BATCH: usize = 100;

/// The size of the arguments or of the response a record keeps in full.
pub const MAX_CAPTURE_BYTES: usize = 64 * 1024;

/// Eight thousand characters leaves room for JSON escaping while keeping the
/// complete wrapper below the 64 KiB field budget.
const CAPTURE_PREVIEW_CHARS: usize = 8 * 1024;

/// One tool call, as the gateway saw it.
#[derive(Debug, Clone, Default)]
pub struct McpCallLogInput {
    pub access_token: AccessToken,
    pub caller_ip: Option<String>,
    /// The MCP the call was for, when the token may use it.
    pub mcp: Option<Mcp>,
    /// The slug the caller named, when it is not one of an MCP the token may use.
    pub mcp_slug: Option<String>,
    pub requested_tool_name: String,
    pub tool_name: Option<String>,
    pub args: Option<Value>,
    pub response: Option<Value>,
    pub outcome: CallOutcome,
    pub error_category: Option<CallErrorCategory>,
    /// What went wrong, as text: the message of an error. It is redacted
    /// before it is stored.
    pub error_summary: Option<String>,
    pub duration: Duration,
}

/// The summary of an error in a form that is safe to store, without the
/// credentials of the MCP the call was for.
pub fn sanitize_error_summary(
    core: &Core,
    value: Option<&str>,
    mcp: Option<&Mcp>,
) -> Option<String> {
    let value = value?;
    Some(match mcp {
        Some(mcp) => sanitize_mcp_diagnostic(&core.encryption, value, mcp),
        None => sanitize_diagnostic(value),
    })
}

/// The slug of an MCP the token may not use comes straight from the caller.
/// It is only stored when it could be the slug of a real MCP.
fn loggable_mcp_slug(slug: Option<&str>) -> Option<String> {
    let slug = slug.map(Value::from);
    match LOGGED_MCP_SLUG.validate(&slug) {
        Ok(Value::String(loggable)) => Some(loggable),
        _ => None,
    }
}

/// Database errors may quote the statement with its values, which here are
/// the captured arguments and responses. Log the driver's code and a
/// redacted excerpt instead of the error itself.
fn warn_persistence_failure(core: &Core, error: &sqlx::Error, mcp: Option<&Mcp>, message: &str) {
    let code = error
        .as_database_error()
        .and_then(|error| error.code())
        .map(|code| slice_utf16(&code, 64).to_string());
    let error = sanitize_error_summary(core, Some(&error.to_string()), mcp);
    tracing::warn!(
        code = code.as_deref(),
        error = error.as_deref(),
        "{message}"
    );
}

/// The JSON a record keeps of the arguments or of the response of a call. A
/// value too large to keep is replaced by its size and its beginning.
pub fn serialize_captured_value(value: &Value) -> String {
    let serialized = value.to_string();
    let original_bytes = serialized.len();
    if original_bytes <= MAX_CAPTURE_BYTES {
        return serialized;
    }

    let preview = slice_utf16(&serialized, CAPTURE_PREVIEW_CHARS);
    let mut preview_json = Value::from(preview).to_string();
    // JavaScript cuts between the two halves of a character outside the
    // Basic Multilingual Plane, and writes the half it kept as an escape.
    if let Some(split) = serialized[preview.len()..].chars().next()
        && mymcps_vine::js::utf16_len(preview) < CAPTURE_PREVIEW_CHARS
    {
        let mut units = [0u16; 2];
        let high = split.encode_utf16(&mut units)[0];
        preview_json.pop();
        preview_json.push_str(&format!("\\u{high:04x}\""));
    }
    format!("{{\"truncated\":true,\"originalBytes\":{original_bytes},\"preview\":{preview_json}}}")
}

#[derive(Debug, Default)]
struct Queue {
    waiting: VecDeque<McpCallLogInput>,
    /// Records queued or being written.
    pending: usize,
    /// Records ever queued.
    queued: u64,
    /// Whether a task is writing the queue down.
    writing: bool,
}

#[derive(Debug)]
struct Inner {
    core: Arc<Core>,
    queue: Mutex<Queue>,
    /// Records written so far, whether the write went through or not.
    written: watch::Sender<u64>,
    last_pruned_at: Mutex<Option<Instant>>,
}

impl Inner {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The call log of one instance: its write queue and its pruning.
#[derive(Debug, Clone)]
pub struct McpCallLogService {
    inner: Arc<Inner>,
}

impl McpCallLogService {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            inner: Arc::new(Inner {
                core,
                queue: Mutex::default(),
                written: watch::Sender::new(0),
                last_pruned_at: Mutex::new(None),
            }),
        }
    }

    pub async fn settings(&self) -> Result<InstanceSetting, sqlx::Error> {
        InstanceSetting::current(&*self.inner.core.db).await
    }

    /// Queue the record of a call and return at once. Records are written in
    /// the order they were queued, those that wait together in one transaction.
    pub fn record(&self, input: McpCallLogInput) {
        let Ok(runtime) = Handle::try_current() else {
            tracing::warn!("MCP call log has no runtime to write with; record was dropped");
            return;
        };

        let mut queue = self.inner.queue();
        if queue.pending >= MAX_PENDING_WRITES {
            tracing::warn!(
                max_pending_writes = MAX_PENDING_WRITES,
                "MCP call log queue is full; record was dropped"
            );
            return;
        }

        queue.pending += 1;
        queue.queued += 1;
        queue.waiting.push_back(input);
        if !queue.writing {
            queue.writing = true;
            runtime.spawn(write_queue(self.inner.clone()));
        }
    }

    /// Wait until every record queued so far is written.
    pub async fn flush(&self) {
        let queued = self.inner.queue().queued;
        let mut written = self.inner.written.subscribe();
        // The sender lives as long as this service: the wait cannot fail.
        let _ = written.wait_for(|written| *written >= queued).await;
    }

    /// Delete the records older than the retention of the instance, and say
    /// how many there were. Runs at most once an hour unless `force` is set.
    pub async fn prune_expired(&self, force: bool) -> u64 {
        {
            let mut last_pruned_at = self
                .inner
                .last_pruned_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !force && last_pruned_at.is_some_and(|at| at.elapsed() < PRUNE_INTERVAL) {
                return 0;
            }
            *last_pruned_at = Some(Instant::now());
        }

        let core = &self.inner.core;
        let pruned = async {
            let settings = InstanceSetting::current(&*core.db).await?;
            let cutoff = Timestamp::now() - chrono::Duration::days(settings.mcp_log_retention_days);
            let deleted = sqlx::query("delete from `mcp_call_logs` where `created_at` < ?")
                .bind(cutoff)
                .execute(&*core.db)
                .await?;
            Ok::<_, sqlx::Error>(deleted.rows_affected())
        }
        .await;
        match pruned {
            Ok(deleted) => deleted,
            Err(error) => {
                warn_persistence_failure(
                    core,
                    &error,
                    None,
                    "Expired MCP call logs could not be pruned",
                );
                0
            }
        }
    }
}

/// Write the queue down until it is empty.
async fn write_queue(inner: Arc<Inner>) {
    loop {
        let batch: Vec<McpCallLogInput> = {
            let mut queue = inner.queue();
            let count = queue.waiting.len().min(WRITE_BATCH);
            if count == 0 {
                queue.writing = false;
                return;
            }
            queue.waiting.drain(..count).collect()
        };
        let count = batch.len();

        // Each batch is written by a task of its own: one that panics takes
        // that task down, and the queue goes on.
        let core = inner.core.clone();
        let write = tokio::spawn(async move { persist_batch(&core, &batch).await });
        if write.await.is_err() {
            tracing::warn!("MCP call log could not be persisted");
        }

        {
            let mut queue = inner.queue();
            queue.pending = queue.pending.saturating_sub(count);
        }
        inner
            .written
            .send_modify(|written| *written += count as u64);
    }
}

fn warn_record_lost(core: &Core, error: &sqlx::Error, input: &McpCallLogInput) {
    warn_persistence_failure(
        core,
        error,
        input.mcp.as_ref(),
        "MCP call log could not be persisted",
    );
}

/// Write the records that waited together, in one transaction. A record
/// that cannot be written is reported and does not stop the others.
async fn persist_batch(core: &Core, batch: &[McpCallLogInput]) {
    let settings = match InstanceSetting::current(&*core.db).await {
        Ok(settings) => settings,
        Err(error) => {
            for input in batch {
                warn_record_lost(core, &error, input);
            }
            return;
        }
    };
    if settings.mcp_log_level == McpLogLevel::Off {
        return;
    }
    let mut logs: Vec<McpCallLog> = batch
        .iter()
        .map(|input| record_of(core, &settings, input))
        .collect();

    // A record alone needs no transaction around its own.
    if let ([log], [input]) = (logs.as_mut_slice(), batch) {
        if let Err(error) = log.insert(&*core.db).await {
            warn_record_lost(core, &error, input);
        }
        return;
    }

    let mut transaction = match core.db.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            for input in batch {
                warn_record_lost(core, &error, input);
            }
            return;
        }
    };
    for (log, input) in logs.iter_mut().zip(batch) {
        if let Err(error) = log.insert(&mut *transaction).await {
            warn_record_lost(core, &error, input);
        }
    }
    if let Err(error) = transaction.commit().await {
        for input in batch {
            warn_record_lost(core, &error, input);
        }
    }
}

/// The row of a call, as the logging level of the instance allows it.
fn record_of(core: &Core, settings: &InstanceSetting, input: &McpCallLogInput) -> McpCallLog {
    let arguments_captured = matches!(
        settings.mcp_log_level,
        McpLogLevel::Arguments | McpLogLevel::Responses
    );
    let response_captured = settings.mcp_log_level == McpLogLevel::Responses;
    let mcp = input.mcp.as_ref();
    McpCallLog {
        access_token_id: Some(input.access_token.id),
        access_token_name: input.access_token.name.clone(),
        access_token_prefix: input.access_token.token_prefix.clone(),
        caller_ip: input
            .caller_ip
            .as_deref()
            .map(|caller_ip| slice_utf16(caller_ip, 64).to_string()),
        mcp_id: mcp.map(|mcp| mcp.id),
        mcp_name: mcp.map(|mcp| mcp.name.clone()),
        mcp_slug: match mcp {
            Some(mcp) => Some(mcp.slug.clone()),
            None => loggable_mcp_slug(input.mcp_slug.as_deref()),
        },
        requested_tool_name: slice_utf16(&input.requested_tool_name, 512).to_string(),
        tool_name: input
            .tool_name
            .as_deref()
            .map(|tool_name| slice_utf16(tool_name, 254).to_string()),
        outcome: input.outcome,
        error_category: input.error_category,
        error_summary: sanitize_error_summary(core, input.error_summary.as_deref(), mcp),
        arguments: input
            .args
            .as_ref()
            .filter(|_| arguments_captured)
            .map(serialize_captured_value),
        arguments_captured,
        response: input
            .response
            .as_ref()
            .filter(|_| response_captured)
            .map(serialize_captured_value),
        response_captured,
        duration_ms: (input.duration.as_secs_f64() * 1000.0).round() as i64,
        ..Default::default()
    }
}
