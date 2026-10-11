//! The cron expressions of `tests/unit/mcp_npm_update.spec.ts`, compared
//! with what Croner answers in the Node app, and the job that updates npm
//! MCPs on that schedule (`app/services/mcp_auto_update_scheduler.ts`),
//! which the Node tests never started.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use mymcps_core::models::{InstanceSetting, Mcp, McpStatus, McpTransport};
use mymcps_core::{Core, Db, Environment, TestCore};
use mymcps_gateway::auto_update::cron::{
    DEFAULT_MCP_AUTO_UPDATE_CRON, is_valid_five_field_cron, parse_five_field_cron,
};
use mymcps_gateway::auto_update::{Clock, McpAutoUpdateScheduler};
use mymcps_upstream::{NpmUpdateRuntime, Upstream, UpstreamError};
use serde_json::Value;
use support::*;
use tokio::sync::{Semaphore, watch};

// five-field cron validation

#[test]
fn accepts_the_default_daily_02_00_utc_expression() {
    assert_eq!(DEFAULT_MCP_AUTO_UPDATE_CRON, "0 2 * * *");
    assert!(is_valid_five_field_cron("0 2 * * *"));
    assert!(is_valid_five_field_cron("  30 14 * * 1  "));
}

#[test]
fn rejects_empty_6_field_and_unparsable_expressions() {
    assert!(!is_valid_five_field_cron(""));
    assert!(!is_valid_five_field_cron("* * * *"));
    assert!(!is_valid_five_field_cron("0 2 * * * *"));
    assert!(!is_valid_five_field_cron("not a cron"));
    assert!(!is_valid_five_field_cron("99 2 * * *"));
}

/// What Croner 10 answers in Node 24 for each expression: whether
/// `isValidFiveFieldCron` takes it, and its next runs in UTC after
/// 2026-10-07T12:34:56Z.
const REFERENCE: &str = include_str!("fixtures/croner_reference.json");

/// The same for expressions put together at random from every form a field
/// takes, most of them wrong: `[expression, next runs]`, where an
/// expression Croner refuses has none.
const FUZZ: &str = include_str!("fixtures/croner_fuzz.json");

/// Where an expression is read, or runs, otherwise than Croner says.
fn difference(pattern: &str, expected_runs: Option<Vec<&str>>, runs: usize) -> Option<String> {
    let parsed = parse_five_field_cron(pattern);
    if parsed.is_some() != expected_runs.is_some() {
        return Some(format!("{pattern:?}: valid {}", parsed.is_some()));
    }
    let (cron, expected) = (parsed?, expected_runs?);

    let mut after = Utc.with_ymd_and_hms(2026, 10, 7, 12, 34, 56).unwrap();
    let mut next_runs = Vec::new();
    while next_runs.len() < runs {
        let Some(next) = cron.next_after(after) else {
            break;
        };
        next_runs.push(next.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
        after = next;
    }
    (next_runs != expected).then(|| format!("{pattern:?}: {next_runs:?} for {expected:?}"))
}

fn runs(listed: &Value) -> Vec<&str> {
    listed
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run.as_str().unwrap())
        .collect()
}

#[test]
fn takes_the_expressions_croner_takes_and_runs_when_it_would() {
    let reference: Value = serde_json::from_str(REFERENCE).unwrap();
    let cases = reference.as_array().unwrap();
    assert!(cases.len() > 150);

    let differences: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            let valid = case["valid"].as_bool().unwrap();
            difference(
                case["pattern"].as_str().unwrap(),
                valid.then(|| runs(&case["next"])),
                4,
            )
        })
        .collect();
    assert!(differences.is_empty(), "{differences:#?}");
}

#[test]
fn reads_random_expressions_as_croner_does() {
    let fuzz: Value = serde_json::from_str(FUZZ).unwrap();
    let cases = fuzz.as_array().unwrap();
    assert_eq!(cases.len(), 1300);
    assert_eq!(cases.iter().filter(|case| !case[1].is_null()).count(), 700);

    let differences: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            difference(
                case[0].as_str().unwrap(),
                (!case[1].is_null()).then(|| runs(&case[1])),
                5,
            )
        })
        .collect();
    assert!(differences.is_empty(), "{differences:#?}");
}

#[test]
fn reads_an_expression_the_way_croner_does_where_that_is_its_own() {
    let from = Utc.with_ymd_and_hms(2026, 10, 7, 12, 34, 56).unwrap();
    let next = |expression: &str| {
        parse_five_field_cron(expression)
            .unwrap()
            .next_after(from)
            .map(|run| run.format("%a %Y-%m-%d %H:%M").to_string())
    };

    // `?` leaves a field open, but not as `*` does: with a day of the week
    // beside it, either may match.
    assert_eq!(next("0 0 * * 1").as_deref(), Some("Mon 2026-10-12 00:00"));
    assert_eq!(next("0 0 ? * 1").as_deref(), Some("Thu 2026-10-08 00:00"));
    // The day of the month or the day of the week, and both with `+`.
    assert_eq!(next("0 0 13 * 5").as_deref(), Some("Fri 2026-10-09 00:00"));
    assert_eq!(next("0 0 13 * +5").as_deref(), Some("Fri 2026-11-13 00:00"));
    // The last day, the last weekday, the weekday nearest to a date.
    assert_eq!(next("0 0 L * *").as_deref(), Some("Sat 2026-10-31 00:00"));
    assert_eq!(next("0 0 LW * *").as_deref(), Some("Fri 2026-10-30 00:00"));
    assert_eq!(next("0 0 31W * *").as_deref(), Some("Fri 2026-10-30 00:00"));
    assert_eq!(next("0 0 1W 11 *").as_deref(), Some("Mon 2026-11-02 00:00"));
    // The second Friday, the last Sunday under either of its numbers.
    assert_eq!(
        next("0 0 * * FRI#2").as_deref(),
        Some("Fri 2026-10-09 00:00")
    );
    assert_eq!(next("0 0 * * 0L").as_deref(), Some("Sun 2026-10-25 00:00"));
    assert_eq!(next("0 0 * * 7L").as_deref(), Some("Sun 2026-10-25 00:00"));
    // A leap day, and a date that never comes.
    assert_eq!(next("0 0 29 2 *").as_deref(), Some("Tue 2028-02-29 00:00"));
    assert_eq!(next("0 0 31 2 *"), None);

    // A run is the first whole minute past the time asked from.
    let cron = parse_five_field_cron("* * * * *").unwrap();
    let at = |second: u32, millisecond: u32| {
        Utc.with_ymd_and_hms(2026, 10, 7, 12, 34, second).unwrap()
            + chrono::Duration::milliseconds(millisecond.into())
    };
    let minute = |minute: u32| Utc.with_ymd_and_hms(2026, 10, 7, 12, minute, 0).unwrap();
    assert_eq!(cron.next_after(at(59, 999)), Some(minute(35)));
    assert_eq!(cron.next_after(at(0, 0)), Some(minute(35)));
    assert_eq!(cron.next_after(at(0, 1)), Some(minute(35)));
    assert_eq!(cron.next_after(minute(35)), Some(minute(36)));
}

#[test]
fn refuses_a_date_where_an_expression_is_expected() {
    // Croner would run once at such a date. The schedule is a recurring one.
    for date in [
        "Oct 7 2027 02:00 UTC",
        "2027-10-07 02:00:00 * *",
        "0 2:30 * * *",
        ":0 2 * * *",
    ] {
        assert!(!is_valid_five_field_cron(date), "{date}");
    }
}

// The job

/// A clock that only moves when the test says so.
struct TestClock(watch::Sender<DateTime<Utc>>);

impl TestClock {
    /// A Wednesday, a little after half past twelve.
    fn new() -> Arc<Self> {
        Arc::new(Self(watch::Sender::new(
            Utc.with_ymd_and_hms(2026, 10, 7, 12, 34, 56).unwrap(),
        )))
    }

    fn advance(&self, by: chrono::Duration) {
        self.0.send_modify(|now| *now += by);
    }

    fn set(&self, hour: u32, minute: u32, second: u32, days_later: i64) {
        let day = Utc
            .with_ymd_and_hms(2026, 10, 7, hour, minute, second)
            .unwrap();
        self.0
            .send_replace(day + chrono::Duration::days(days_later));
    }
}

#[async_trait::async_trait]
impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.borrow()
    }

    async fn sleep(&self, duration: Duration) {
        let deadline = self.now() + duration;
        let mut now = self.0.subscribe();
        let _ = now.wait_for(|now| *now >= deadline).await;
    }
}

/// Which MCPs were reloaded, and when.
type Reloads = Arc<Mutex<Vec<(i64, DateTime<Utc>)>>>;

/// Stands in for the two steps of an update: it keeps which MCP was
/// reloaded and when, and finds every MCP ready.
struct Runtime {
    core: Arc<Core>,
    clock: Arc<TestClock>,
    reloaded: Reloads,
    /// A reload takes one permit, and waits while there is none.
    may_finish: Arc<Semaphore>,
    /// How many updates went through both of their steps.
    finished: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NpmUpdateRuntime for Runtime {
    async fn reload(&self, mcp: &Mcp) -> Result<(), UpstreamError> {
        self.reloaded
            .lock()
            .unwrap()
            .push((mcp.id, self.clock.now()));
        self.may_finish.acquire().await.unwrap().forget();
        Ok(())
    }

    async fn probe(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        mcp.status = McpStatus::Ready;
        mcp.last_error = None;
        mcp.save(&*self.core.db).await?;
        self.finished.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A server that is not under test, with one npm MCP that tracks `latest`.
struct Scheduled {
    scheduler: McpAutoUpdateScheduler,
    clock: Arc<TestClock>,
    reloaded: Reloads,
    may_finish: Arc<Semaphore>,
    finished: Arc<AtomicUsize>,
    latest: Mcp,
    core: TestCore,
}

impl Scheduled {
    async fn new(environment: Environment) -> Self {
        let core = TestCore::with_config(|config| config.environment = environment).await;
        let clock = TestClock::new();
        let reloaded: Reloads = Arc::default();
        let may_finish = Arc::new(Semaphore::new(Semaphore::MAX_PERMITS));
        let finished = Arc::new(AtomicUsize::new(0));
        let upstream = Upstream::builder(core.core.clone(), Default::default())
            .npm_update_runtime(Runtime {
                core: core.core.clone(),
                clock: clock.clone(),
                reloaded: reloaded.clone(),
                may_finish: may_finish.clone(),
                finished: finished.clone(),
            })
            .build();

        let admin = create_admin(&core.db).await;
        let npm = |name: &'static str, version: Option<&'static str>| {
            move |mcp: &mut Mcp| {
                mcp.name = name.into();
                mcp.transport = McpTransport::Npm;
                mcp.http_url = None;
                mcp.npm_package = Some(format!("@example/{}", Mcp::slugify(name)));
                mcp.npm_version = version.map(str::to_owned);
            }
        };
        let latest = create_mcp(&core.db, admin.id, npm("Latest MCP", Some("latest"))).await;
        create_mcp(&core.db, admin.id, npm("Pinned MCP", Some("2.0.0"))).await;
        create_mcp(&core.db, admin.id, |mcp| mcp.name = "HTTP MCP".into()).await;

        Self {
            scheduler: McpAutoUpdateScheduler::with_clock(upstream, clock.clone()),
            clock,
            reloaded,
            may_finish,
            finished,
            latest,
            core,
        }
    }

    fn db(&self) -> &Db {
        &self.core.db
    }

    /// Save the settings of the instance, as its Settings page does.
    async fn configure(&self, enabled: bool, cron: &str) {
        let mut settings = InstanceSetting::current(&**self.db()).await.unwrap();
        settings.mcp_auto_update_enabled = enabled;
        settings.mcp_auto_update_cron = cron.to_owned();
        settings.save(&**self.db()).await.unwrap();
    }

    fn runs(&self) -> usize {
        self.reloaded.lock().unwrap().len()
    }

    /// Wait until the job has started `runs` runs.
    async fn ran(&self, runs: usize) {
        for _ in 0..1000 {
            if self.runs() >= runs {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(self.runs(), runs);
    }

    /// Give the job the time to do what it should not, and see that it did not.
    async fn still_ran(&self, runs: usize) {
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(self.runs(), runs);
    }
}

#[tokio::test]
async fn skips_scheduling_under_test() {
    let server = Scheduled::new(Environment::Test).await;
    server.configure(true, "* * * * *").await;

    server.scheduler.start().await.unwrap();

    assert!(!server.scheduler.is_scheduled());
    server.clock.advance(chrono::Duration::hours(1));
    server.still_ran(0).await;
}

#[tokio::test]
async fn schedules_nothing_until_auto_update_is_turned_on() {
    let server = Scheduled::new(Environment::Production).await;
    // The settings of a new instance: off, every day at 02:00 UTC.
    let settings = InstanceSetting::current(&**server.db()).await.unwrap();
    assert!(!settings.mcp_auto_update_enabled);
    assert_eq!(settings.mcp_auto_update_cron, "0 2 * * *");

    server.scheduler.start().await.unwrap();

    assert!(!server.scheduler.is_scheduled());
    server.clock.advance(chrono::Duration::days(2));
    server.still_ran(0).await;
}

#[tokio::test]
async fn reloads_the_npm_mcps_that_track_latest_when_the_time_comes() {
    let server = Scheduled::new(Environment::Production).await;
    server.configure(true, "*/10 * * * *").await;

    let logs = CapturedLogs::start();
    server.scheduler.start().await.unwrap();
    assert!(server.scheduler.is_scheduled());

    // 12:38:56: not yet.
    server.clock.advance(chrono::Duration::minutes(4));
    server.still_ran(0).await;

    // 12:40:00, on the minute.
    server.clock.advance(chrono::Duration::seconds(64));
    server.ran(1).await;
    assert_eq!(
        *server.reloaded.lock().unwrap(),
        [(
            server.latest.id,
            Utc.with_ymd_and_hms(2026, 10, 7, 12, 40, 0).unwrap()
        )]
    );

    // Pinned versions and HTTP MCPs are never touched.
    server.clock.advance(chrono::Duration::minutes(10));
    server.ran(2).await;
    assert!(
        server
            .reloaded
            .lock()
            .unwrap()
            .iter()
            .all(|(id, _)| *id == server.latest.id)
    );
    let versions: Vec<Option<String>> =
        sqlx::query_scalar("select `npm_version` from `mcps` order by `id` asc")
            .fetch_all(&**server.db())
            .await
            .unwrap();
    assert_eq!(
        versions,
        [Some("latest".to_owned()), Some("2.0.0".to_owned()), None]
    );

    for _ in 0..1000 {
        if logs
            .text()
            .matches("Scheduled npm MCP updates finished")
            .count()
            == 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let logged = logs.text();
    assert_eq!(
        logged.matches("Running scheduled npm MCP updates").count(),
        2
    );
    assert!(
        logged.contains("Scheduled npm MCP updates finished updated=1 skipped=1 failed=[]"),
        "{logged}"
    );
}

#[tokio::test]
async fn does_not_catch_up_on_the_runs_it_missed() {
    let server = Scheduled::new(Environment::Production).await;
    server.configure(true, "*/5 * * * *").await;
    server.scheduler.start().await.unwrap();

    // The machine slept for three hours: one run, not thirty-six.
    server.clock.advance(chrono::Duration::hours(3));
    server.ran(1).await;
    server.still_ran(1).await;

    server.clock.advance(chrono::Duration::minutes(5));
    server.ran(2).await;
}

#[tokio::test]
async fn runs_every_day_at_two_when_no_expression_is_set() {
    let server = Scheduled::new(Environment::Production).await;
    server.configure(true, "   ").await;
    server.scheduler.start().await.unwrap();
    assert!(server.scheduler.is_scheduled());

    server.clock.set(1, 59, 59, 1);
    server.still_ran(0).await;

    server.clock.set(2, 0, 0, 1);
    server.ran(1).await;

    server.clock.set(23, 0, 0, 1);
    server.still_ran(1).await;
    server.clock.set(2, 0, 1, 2);
    server.ran(2).await;
}

#[tokio::test]
async fn ignores_an_expression_it_cannot_read() {
    let server = Scheduled::new(Environment::Production).await;
    // The Settings page refuses such an expression: this one was written
    // to the database by other means.
    server.configure(true, "every day at two").await;

    let logs = CapturedLogs::start();
    server.scheduler.start().await.unwrap();

    assert!(!server.scheduler.is_scheduled());
    let logged = logs.text();
    assert!(logged.contains("ERROR"), "{logged}");
    assert!(logged.contains("Ignoring invalid MCP auto-update cron expression"));
    assert!(logged.contains("every day at two"));
    server.clock.advance(chrono::Duration::days(2));
    server.still_ran(0).await;
}

#[tokio::test]
async fn follows_the_settings_when_they_are_saved() {
    let server = Scheduled::new(Environment::Production).await;
    server.configure(true, "0 13 * * *").await;
    server.scheduler.start().await.unwrap();

    // Before 13:00 comes, the schedule is changed to every half hour.
    server.configure(true, "*/30 * * * *").await;
    server.scheduler.resync().await.unwrap();
    assert!(server.scheduler.is_scheduled());
    server.clock.set(12, 45, 0, 0);
    server.still_ran(0).await;
    server.clock.set(13, 0, 0, 0);
    // One job runs, not the one that was replaced as well.
    server.ran(1).await;
    server.still_ran(1).await;
    server.clock.set(13, 30, 0, 0);
    server.ran(2).await;

    // Turned off: the job stops without a restart.
    server.configure(false, "*/30 * * * *").await;
    server.scheduler.resync().await.unwrap();
    assert!(!server.scheduler.is_scheduled());
    server.clock.advance(chrono::Duration::days(1));
    server.still_ran(2).await;

    // And on again.
    server.configure(true, "*/30 * * * *").await;
    server.scheduler.resync().await.unwrap();
    server.clock.advance(chrono::Duration::minutes(30));
    server.ran(3).await;
}

#[tokio::test]
async fn skips_a_run_while_the_previous_one_is_still_reloading() {
    let server = Scheduled::new(Environment::Production).await;
    server.configure(true, "*/5 * * * *").await;
    // The reload of the next run does not finish until the test lets it.
    server.may_finish.forget_permits(Semaphore::MAX_PERMITS);
    let logs = CapturedLogs::start();
    server.scheduler.start().await.unwrap();

    server.clock.set(12, 40, 0, 0);
    server.ran(1).await;
    for minute in [45, 50, 55] {
        server.clock.set(12, minute, 0, 0);
        server.still_ran(1).await;
    }
    // The times that came meanwhile started nothing.
    assert_eq!(
        logs.text()
            .matches("Running scheduled npm MCP updates")
            .count(),
        1
    );

    // Once it is done, the next time is a run again.
    server.may_finish.add_permits(Semaphore::MAX_PERMITS);
    for _ in 0..1000 {
        if server.finished.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(server.finished.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(60)).await;
    server.clock.set(13, 0, 0, 0);
    server.ran(2).await;
    assert_eq!(
        logs.text()
            .matches("Running scheduled npm MCP updates")
            .count(),
        2
    );
}

#[tokio::test]
async fn stops_with_the_server() {
    let server = Scheduled::new(Environment::Production).await;
    server.configure(true, "*/5 * * * *").await;
    server.scheduler.start().await.unwrap();
    assert!(server.scheduler.is_scheduled());

    server.scheduler.stop();

    assert!(!server.scheduler.is_scheduled());
    server.clock.advance(chrono::Duration::hours(1));
    server.still_ran(0).await;

    // Starting again is a new job, and dropping the scheduler stops it.
    server.scheduler.start().await.unwrap();
    assert!(server.scheduler.is_scheduled());
    let Scheduled {
        scheduler,
        clock,
        reloaded,
        ..
    } = server;
    drop(scheduler);
    clock.advance(chrono::Duration::hours(1));
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(reloaded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn waits_for_nothing_when_the_expression_names_a_date_that_never_comes() {
    let server = Scheduled::new(Environment::Production).await;
    server.configure(true, "0 0 31 2 *").await;

    server.scheduler.start().await.unwrap();

    for _ in 0..1000 {
        if !server.scheduler.is_scheduled() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!server.scheduler.is_scheduled());
    server.still_ran(0).await;
}
