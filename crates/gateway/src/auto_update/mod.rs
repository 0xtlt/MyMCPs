//! In-process job for instance-wide Deno npm MCP auto-updates.
//!
//! MyMCPs is a single process with SQLite, so this runs inside the web
//! server rather than a separate worker. The server starts it on boot and
//! settings saves call [`McpAutoUpdateScheduler::resync`] so cron/enablement
//! changes take effect without a restart.
//!
//! The job only reloads npm MCPs that already track `latest`. Pinned versions
//! and HTTP MCPs are never touched. Expressions are 5-field cron in UTC
//! (`0 2 * * *` = 02:00 UTC). Tests skip scheduling (`NODE_ENV=test`).

pub mod cron;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mymcps_core::models::InstanceSetting;
use mymcps_core::redaction::sanitize_diagnostic;
use mymcps_upstream::Upstream;
use mymcps_vine as vine;
use tokio::task::JoinHandle;

use cron::{CronPattern, DEFAULT_MCP_AUTO_UPDATE_CRON, parse_five_field_cron};

/// The longest the job sleeps before it looks at the time again, so that a
/// clock that was set, or a machine that slept, does not hold a run back.
const MAX_DELAY: Duration = Duration::from_secs(30);

/// The time the job runs by. The server reads the time of the machine; a
/// test hands in a clock it moves forward itself.
#[async_trait]
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Utc>;

    /// Wait until `duration` has passed on this clock.
    async fn sleep(&self, duration: Duration);
}

struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// The job of one server, replaced whenever the instance settings change.
pub struct McpAutoUpdateScheduler {
    upstream: Arc<Upstream>,
    clock: Arc<dyn Clock>,
    job: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for McpAutoUpdateScheduler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpAutoUpdateScheduler")
            .field("scheduled", &self.is_scheduled())
            .finish_non_exhaustive()
    }
}

impl McpAutoUpdateScheduler {
    /// A scheduler with no job yet: see [`start`](Self::start).
    pub fn new(upstream: Arc<Upstream>) -> Self {
        Self::with_clock(upstream, Arc::new(SystemClock))
    }

    /// A scheduler that runs by `clock`.
    pub fn with_clock(upstream: Arc<Upstream>, clock: Arc<dyn Clock>) -> Self {
        Self {
            upstream,
            clock,
            job: Mutex::new(None),
        }
    }

    fn should_schedule(&self) -> bool {
        !self.upstream.core().config.is_test()
    }

    fn replace_job(&self, job: Option<JoinHandle<()>>) {
        let previous = std::mem::replace(
            &mut *self.job.lock().unwrap_or_else(PoisonError::into_inner),
            job,
        );
        // Only the timer stops: a run that has started goes on to its end.
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    /// Whether a job waits for its next run.
    pub fn is_scheduled(&self) -> bool {
        self.job
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|job| !job.is_finished())
    }

    /// Replace the running job with whatever instance settings currently say.
    /// Stops the job when auto-update is off or the cron expression is invalid.
    pub async fn resync(&self) -> Result<(), sqlx::Error> {
        self.stop();

        if !self.should_schedule() {
            return Ok(());
        }

        let settings = InstanceSetting::current(&*self.upstream.core().db).await?;
        if !settings.mcp_auto_update_enabled {
            return Ok(());
        }

        let expression = match vine::js::trim(&settings.mcp_auto_update_cron) {
            "" => DEFAULT_MCP_AUTO_UPDATE_CRON,
            expression => expression,
        };
        let Some(cron) = parse_five_field_cron(expression) else {
            tracing::error!(
                cron = expression,
                "Ignoring invalid MCP auto-update cron expression"
            );
            return Ok(());
        };

        // The schedule counts from now, whenever the task gets to run.
        let job = tokio::spawn(run_job(
            self.upstream.clone(),
            self.clock.clone(),
            cron,
            self.clock.now(),
        ));
        self.replace_job(Some(job));
        Ok(())
    }

    /// Start (or refresh) the job from current instance settings. Called once
    /// when the server is ready.
    pub async fn start(&self) -> Result<(), sqlx::Error> {
        self.resync().await
    }

    /// Stop the job on shutdown so its timer does not outlive the server.
    pub fn stop(&self) {
        self.replace_job(None);
    }
}

impl Drop for McpAutoUpdateScheduler {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A run under way. The next tick may start one again once this is dropped,
/// however the run ended.
struct Run(Arc<AtomicBool>);

impl Drop for Run {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Wait for each time the expression names after `after`, and update the
/// npm MCPs then.
async fn run_job(
    upstream: Arc<Upstream>,
    clock: Arc<dyn Clock>,
    cron: CronPattern,
    mut after: DateTime<Utc>,
) {
    // A tick is skipped when the previous run is still reloading caches.
    let running = Arc::new(AtomicBool::new(false));

    // An expression may name a date that never comes: the job then ends.
    while let Some(next) = cron.next_after(after) {
        loop {
            let now = clock.now();
            if now >= next {
                break;
            }
            let wait = (next - now).to_std().unwrap_or_default();
            clock.sleep(wait.min(MAX_DELAY)).await;
        }

        if !running.swap(true, Ordering::AcqRel) {
            let upstream = upstream.clone();
            let run = Run(running.clone());
            // A task of its own: replacing the job leaves the run alone.
            tokio::spawn(async move {
                let _run = run;
                tracing::info!("Running scheduled npm MCP updates");
                match upstream.update_latest_tracking_mcps().await {
                    Ok(result) => tracing::info!(
                        updated = result.updated,
                        skipped = result.skipped,
                        failed = ?result.failed,
                        "Scheduled npm MCP updates finished"
                    ),
                    Err(error) => tracing::error!(
                        error = %sanitize_diagnostic(&error.to_string()),
                        "Scheduled npm MCP updates failed"
                    ),
                }
            });
        }

        // The times that passed meanwhile are not caught up on.
        after = clock.now().max(next);
    }
}
