//! Port of `tests/functional/logs_analytics.spec.ts`, of the Logs page case
//! of `hardening_gateway_mcp.spec.ts`, and of what the server does in
//! `tests/browser/analytics_custom_range.spec.ts` and `date_formatting.spec.ts`.
//!
//! The specs asserted the props handed to the React pages. These assert the
//! same facts on the HTML the server now renders.

mod support;

use chrono::{DateTime, Duration, DurationRound, TimeZone, Utc};
use chrono_tz::Europe::Paris;
use http::StatusCode;
use mymcps_core::Timestamp;
use mymcps_core::models::{CallErrorCategory, CallOutcome, InstanceSetting, McpLogLevel};
use mymcps_web::routes::analytics::{Range, Unit, Zone, buckets, range_config, resolve_time_zone};
use mymcps_web::testing::TestApp;
use mymcps_web::testing::factories::{create_admin, create_mcp, create_member};
use serde_json::{Value, json};
use support::*;

fn instant(iso: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(iso)
        .unwrap_or_else(|_| panic!("{iso} is not an instant"))
        .with_timezone(&Utc)
}

/// An instant as the address of a custom range carries it.
fn address_instant(time: DateTime<Utc>) -> String {
    time.format("%Y-%m-%dT%H:%MZ").to_string()
}

async fn turn_logging_off(app: &TestApp) {
    let mut settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    settings.mcp_log_level = McpLogLevel::Off;
    settings.save(&*app.core.db).await.unwrap();
}

/// The value of the first field of this name in a page.
fn field_value<'a>(page: &'a str, name: &str) -> &'a str {
    let field = between(page, &format!("name=\"{name}\""), ">");
    between(field, "value=\"", "\"")
}

/// The periods of the timeline, as (label, calls, errors).
fn timeline_of(page: &str) -> Vec<(String, String, String)> {
    table_rows(page, "<table class=\"visually-hidden\">")
        .into_iter()
        .map(|cells| (cells[0].clone(), cells[1].clone(), cells[2].clone()))
        .collect()
}

/// The figure under a label of the metric strip.
fn metric<'a>(page: &'a str, label: &str) -> &'a str {
    between(
        page,
        &format!("<dt class=\"metric__label\">{label}</dt><dd class=\"metric__value\">"),
        "</dd>",
    )
}

#[tokio::test]
async fn restricts_logs_and_analytics_to_admins() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;

    let anonymous = app.get("/logs").send().await;
    assert_eq!(anonymous.status, StatusCode::FOUND);
    assert_eq!(anonymous.redirect_path().as_deref(), Some("/login"));

    for path in ["/logs", "/analytics"] {
        let refused = app.get(path).login_as(&member).send().await;
        assert_eq!(refused.status, StatusCode::FOUND, "{path}");
        assert_eq!(refused.redirect_path().as_deref(), Some("/"), "{path}");
        assert_eq!(
            refused.flashed("error"),
            Some(json!("Admin access required"))
        );
    }

    let logs = app.get("/logs").login_as(&admin).send().await;
    assert_eq!(logs.status, StatusCode::OK);
    assert!(
        logs.text()
            .contains("<h1 class=\"page-header__title\">MCP call logs</h1>")
    );
    let analytics = app.get("/analytics").login_as(&admin).send().await;
    assert_eq!(analytics.status, StatusCode::OK);
    assert!(
        analytics
            .text()
            .contains("<h1 class=\"page-header__title\">MCP analytics</h1>")
    );
}

#[tokio::test]
async fn filters_logs_and_returns_the_selected_detail() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |token| {
        token.name = "Agent token".into();
        token.token_prefix = "mcp_agent".into();
    })
    .await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Search MCP".into();
        mcp.slug = "search".into();
    })
    .await;
    create_mcp_call_log(&app, &token, Some(&mcp), |log| {
        log.requested_tool_name = "search__ok".into();
    })
    .await;
    let failed = create_mcp_call_log(&app, &token, Some(&mcp), |log| {
        log.outcome = CallOutcome::Error;
        log.requested_tool_name = "search__fail".into();
        log.tool_name = Some("fail".into());
        log.error_category = Some(CallErrorCategory::ToolError);
        log.error_summary = Some("Upstream tool returned an error".into());
        log.caller_ip = Some("192.0.2.12".into());
        log.arguments_captured = true;
        log.arguments = Some(json!({ "query": "private value" }).to_string());
        log.response_captured = true;
        log.response =
            Some(json!({ "content": [{ "type": "text", "text": "private result" }] }).to_string());
    })
    .await;

    let response = app
        .get(&format!(
            "/logs?range=all&outcome=error&mcp=search&token=mcp_agent&logId={}",
            failed.id
        ))
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();

    // The filters, as the chips and the form show them.
    for checked in [
        "name=\"range\" value=\"all\" checked",
        "name=\"outcome\" value=\"error\" checked",
        "name=\"mcp\" value=\"search\" checked",
        "name=\"token\" value=\"mcp_agent\" checked",
    ] {
        assert!(page.contains(checked), "{checked}");
    }
    for chip in [
        "Outcome: Error",
        "MCP: Search MCP",
        "Access token: Agent token",
    ] {
        assert!(page.contains(chip), "{chip}");
    }
    assert!(page.contains("popovertarget=\"filter-range\">All retained"));

    // One call passes them, on a page of 25.
    assert!(page.contains("<span class=\"pagination__range\">1–1 of 1</span>"));
    assert!(page.contains("25 per page"));
    let rows = table_rows(
        &page,
        "<table class=\"table table--dense table--interactive\">",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0][1..],
        ["Search MCP", "fail", "Agent token", "Error", "25 ms"]
    );
    assert!(page.contains(&format!("id=\"log-{}\"", failed.id)));
    assert!(!page.contains("search__ok"));

    // Its details are open, with what was captured.
    let panel = between(
        &page,
        "<dialog class=\"drawer\" id=\"call-details\"",
        "</dialog>",
    );
    assert!(panel.contains("data-open"));
    assert!(panel.contains(&format!("data-dialog-trigger=\"#log-{}\"", failed.id)));
    let details = text_of(panel);
    for shown in [
        "search__fail",
        "Agent token (mcp_agent…)",
        "192.0.2.12",
        "tool error",
        "Upstream tool returned an error",
        "\"query\": \"private value\"",
        "\"text\": \"private result\"",
    ] {
        assert!(details.contains(shown), "{shown}");
    }
    assert!(panel.contains(&format!("href=\"/mcps/{}/edit\"", mcp.id)));
    // The call as JSON, for the copy button.
    let copied: Value = serde_json::from_str(&text_of(between(
        panel,
        "<pre id=\"call-json\" hidden>",
        "</pre>",
    )))
    .unwrap();
    assert_eq!(copied["id"], json!(failed.id));
    assert_eq!(copied["callerIp"], json!("192.0.2.12"));
    assert_eq!(copied["errorCategory"], json!("tool_error"));
    assert_eq!(copied["arguments"], json!({ "query": "private value" }));
    assert_eq!(
        copied["response"],
        json!({ "content": [{ "type": "text", "text": "private result" }] })
    );
}

#[tokio::test]
async fn validates_and_normalizes_log_query_parameters_with_vine() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let blank_filters = app
        .get("/logs?outcome=&mcp=&token=")
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(blank_filters.status, StatusCode::OK);
    let page = blank_filters.text();
    assert!(page.contains("name=\"range\" value=\"24h\" checked"));
    for all in ["outcome", "mcp", "token"] {
        assert!(
            page.contains(&format!("name=\"{all}\" value=\"\" checked")),
            "{all}"
        );
    }
    assert!(!page.contains("chip--active"));
    assert!(!page.contains("name=\"pageSize\""));
    assert_eq!(field_value(&page, "timeZone"), "UTC");

    let selected_page_size = app
        .get("/logs?pageSize=10&timeZone=Europe/Paris")
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(selected_page_size.status, StatusCode::OK);
    let page = selected_page_size.text();
    assert_eq!(field_value(&page, "timeZone"), "Europe/Paris");
    assert_eq!(field_value(&page, "pageSize"), "10");

    for (query, field) in [("range=invalid", "range"), ("pageSize=30", "pageSize")] {
        let invalid = app
            .get(&format!("/logs?{query}"))
            .login_as(&admin)
            .send()
            .await;
        assert_eq!(invalid.status, StatusCode::FOUND, "{query}");
        let errors = invalid.flashed("errors").unwrap();
        assert!(errors.get(field).is_some(), "{query}: {errors}");
    }
}

#[tokio::test]
async fn builds_the_mcp_filter_of_the_logs_page_from_the_mcps_that_exist() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |_| {}).await;
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Search MCP".into();
        mcp.slug = "search".into();
    })
    .await;
    create_mcp_call_log(&app, &token, None, |log| {
        log.mcp_slug = Some("made-up-by-a-caller".into());
        log.requested_tool_name = "made-up-by-a-caller__tool".into();
        log.tool_name = Some("tool".into());
        log.outcome = CallOutcome::Error;
        log.error_category = Some(CallErrorCategory::DisallowedMcp);
        log.duration_ms = 1;
    })
    .await;

    let response = app.get("/logs?range=all").login_as(&admin).send().await;
    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();
    assert!(page.contains("1–1 of 1"));
    let menu = between(&page, "id=\"filter-mcp\"", "</div>");
    assert_eq!(menu.matches("name=\"mcp\"").count(), 2);
    assert!(menu.contains("name=\"mcp\" value=\"\" checked>All MCPs"));
    assert!(menu.contains("name=\"mcp\" value=\"search\">Search MCP"));
    // The slug a caller made up is in the list of calls, not among the filters.
    assert!(!menu.contains("made-up-by-a-caller"));
    assert!(page.contains("made-up-by-a-caller"));
}

#[tokio::test]
async fn calculates_analytics_metrics_buckets_and_breakdowns() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |token| {
        token.name = "Analytics token".into();
    })
    .await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Analytics MCP".into();
        mcp.slug = "analytics-mcp".into();
    })
    .await;
    create_mcp_call_log(&app, &token, Some(&mcp), |log| log.duration_ms = 100).await;
    create_mcp_call_log(&app, &token, Some(&mcp), |log| {
        log.outcome = CallOutcome::Error;
        log.error_category = Some(CallErrorCategory::ToolError);
        log.duration_ms = 300;
    })
    .await;

    let response = app
        .get("/analytics?range=7d&timeZone=Europe/Paris")
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();

    assert!(
        page.contains("range=7d&amp;timeZone=Europe%2FParis\" aria-current=\"true\">7 days</a>")
    );
    assert_eq!(field_value(&page, "timeZone"), "Europe/Paris");
    assert!(page.contains("Successful and failed tool-call attempts in Europe/Paris (UTC+0"));
    assert_eq!(metric(&page, "Total calls"), "2");
    assert_eq!(metric(&page, "Success rate"), "50.0%");
    assert_eq!(metric(&page, "Error rate"), "50.0%");
    assert_eq!(metric(&page, "Average duration"), "200 ms");
    assert_eq!(
        table_rows(&page, "id=\"top-mcps-title\""),
        [["Analytics MCP", "2", "1", "200 ms"]]
    );
    assert_eq!(
        table_rows(&page, "id=\"top-tools-title\""),
        [["echo", "2", "1", "200 ms"]]
    );
    assert_eq!(
        table_rows(&page, "id=\"top-tokens-title\""),
        [["Analytics token", "2", "1", "200 ms"]]
    );

    // The last of the seven days is today in Paris, and holds both calls.
    let today = Utc::now().with_timezone(&Paris);
    let timeline = timeline_of(&page);
    assert_eq!(timeline.len(), 7);
    assert_eq!(
        timeline[6],
        (
            today.format("%b %-d").to_string(),
            "2".to_string(),
            "1".to_string()
        )
    );
    assert!(
        timeline[..6]
            .iter()
            .all(|(_, calls, errors)| calls == "0" && errors == "0")
    );
    let (_, zone) = resolve_time_zone(Some("Europe/Paris"));
    let config = range_config(Range::Days7, zone, None, None, Utc::now());
    let midnight = Paris
        .from_local_datetime(&today.date_naive().and_hms_opt(0, 0, 0).unwrap())
        .unwrap();
    assert_eq!(
        buckets(&config).last().unwrap().start.utc(),
        midnight.with_timezone(&Utc)
    );
}

#[tokio::test]
async fn filters_analytics_by_an_exact_custom_time_range() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |token| {
        token.name = "Custom range token".into();
    })
    .await;
    let custom_start = Utc::now().duration_trunc(Duration::hours(1)).unwrap() - Duration::hours(6)
        + Duration::minutes(30);
    let custom_end = custom_start + Duration::hours(4);
    for (created_at, duration_ms, outcome) in [
        (
            custom_start - Duration::minutes(1),
            50,
            CallOutcome::Success,
        ),
        (custom_start, 100, CallOutcome::Success),
        (custom_end - Duration::minutes(1), 300, CallOutcome::Error),
        (custom_end, 500, CallOutcome::Success),
    ] {
        create_mcp_call_log(&app, &token, None, |log| {
            log.duration_ms = duration_ms;
            log.outcome = outcome;
            log.created_at = Timestamp::from(created_at);
        })
        .await;
    }

    let start = address_instant(custom_start);
    let end = address_instant(custom_end);
    let response = app
        .get(&format!(
            "/analytics?range=custom&start={}&end={}&timeZone=Europe%2FParis",
            start.replace(':', "%3A"),
            end.replace(':', "%3A")
        ))
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();

    assert!(page.contains("aria-current=\"page\">Custom</a>"));
    assert_eq!(metric(&page, "Total calls"), "2");
    assert_eq!(metric(&page, "Success rate"), "50.0%");
    assert_eq!(metric(&page, "Error rate"), "50.0%");
    assert_eq!(metric(&page, "Average duration"), "200 ms");

    // Four hours, by the hour, from the start to the minute.
    let timeline = timeline_of(&page);
    let local = |time: DateTime<Utc>| time.with_timezone(&Paris);
    assert_eq!(
        timeline
            .iter()
            .map(|(_, calls, _)| calls.as_str())
            .collect::<Vec<_>>(),
        ["1", "0", "0", "1"]
    );
    assert_eq!(
        timeline[0].0,
        local(custom_start).format("%H:%M").to_string()
    );
    assert_eq!(
        timeline[3].0,
        local(custom_end - Duration::hours(1))
            .format("%H:%M")
            .to_string()
    );
    // The range itself, as the dialog reads it back.
    assert_eq!(field_value(&page, "start"), start);
    assert_eq!(field_value(&page, "end"), end);
    assert_eq!(
        field_value(&page, "startDate"),
        local(custom_start).format("%Y-%m-%d").to_string()
    );
    assert_eq!(
        field_value(&page, "startTime"),
        local(custom_start).format("%H:%M").to_string()
    );
    assert_eq!(
        field_value(&page, "endDate"),
        local(custom_end).format("%Y-%m-%d").to_string()
    );
    assert_eq!(
        field_value(&page, "endTime"),
        local(custom_end).format("%H:%M").to_string()
    );
}

#[tokio::test]
async fn keeps_custom_instants_exact_across_daylight_saving_transitions() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let (_, zone) = resolve_time_zone(Some("Europe/Paris"));
    let analytics = |start: &str, end: &str| {
        let path = format!(
            "/analytics?range=custom&start={}&end={}&timeZone=Europe%2FParis",
            start.replace(':', "%3A").replace('+', "%2B"),
            end.replace(':', "%3A").replace('+', "%2B")
        );
        let (app, admin) = (&app, &admin);
        async move { app.get(&path).login_as(admin).send().await }
    };

    for (start, end, local_start, local_end, dates, times) in [
        (
            "2026-03-29T00:30Z",
            "2026-03-29T01:30Z",
            "2026-03-29T01:30+01:00",
            "2026-03-29T03:30+02:00",
            ("2026-03-29", "2026-03-29"),
            ("01:30", "03:30"),
        ),
        (
            "2026-10-25T00:30Z",
            "2026-10-25T01:30Z",
            "2026-10-25T02:30+02:00",
            "2026-10-25T02:30+01:00",
            ("2026-10-25", "2026-10-25"),
            ("02:30", "02:30"),
        ),
    ] {
        let config = range_config(Range::Custom, zone, Some(start), Some(end), Utc::now());
        assert_eq!(config.range, Range::Custom);
        assert_eq!(config.start.to_iso(), local_start);
        assert_eq!(config.end.to_iso(), local_end);
        assert_eq!(buckets(&config).len(), 1);

        let response = analytics(start, end).await;
        assert_eq!(response.status, StatusCode::OK);
        let page = response.text();
        assert!(page.contains("aria-current=\"page\">Custom</a>"), "{start}");
        assert_eq!(field_value(&page, "timeZone"), "Europe/Paris");
        assert_eq!(
            (
                field_value(&page, "startDate"),
                field_value(&page, "endDate")
            ),
            dates
        );
        assert_eq!(
            (
                field_value(&page, "startTime"),
                field_value(&page, "endTime")
            ),
            times
        );
        assert_eq!(
            (field_value(&page, "start"), field_value(&page, "end")),
            (start, end)
        );
    }

    // A range the page cannot use is answered with the last 7 days.
    for (start, end) in [
        // No offset, in the hour the clocks skip.
        ("2026-03-29T02:30", "2026-03-29T03:30"),
        // Half a second past the boundary.
        ("2026-03-29T00:30:00.500Z", "2026-03-29T01:30:00.500Z"),
        ("not-a-date+01:00", "2026-03-29T03:30+02:00"),
        ("2026-03-29T03:30+02:00", "2026-03-29T01:30+01:00"),
        ("2025-01-01T00:00Z", "2026-01-02T00:00Z"),
        ("2026-03-29T00:30:00.0001Z", "2026-03-29T01:30Z"),
    ] {
        let response = analytics(start, end).await;
        assert_eq!(response.status, StatusCode::OK, "{start}");
        let page = response.text();
        assert!(
            page.contains(
                "range=7d&amp;timeZone=Europe%2FParis\" aria-current=\"true\">7 days</a>"
            ),
            "{start}"
        );
        assert!(
            !page.contains("aria-current=\"page\">Custom</a>"),
            "{start}"
        );
        let config = range_config(Range::Custom, zone, Some(start), Some(end), Utc::now());
        assert_eq!(
            (config.range, config.unit, config.count),
            (Range::Days7, Unit::Day, 7)
        );
    }
}

/// A clock, a zone, a range and the two bounds of a custom one, then the answer.
type RangeCase = (
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
);

/// What `analytics_controller.ts` answered, with Luxon, for some six hundred
/// clocks, zones and ranges: `rangeConfig` and its buckets were run in Node
/// with `Settings.now` fixed, and each answer written as
/// `zone range unit count start end firstBucket lastBucket sumOfBucketBounds`.
#[test]
fn cuts_periods_and_buckets_as_luxon_did() {
    let cases: Vec<RangeCase> =
        serde_json::from_str(include_str!("fixtures/analytics_ranges.json")).unwrap();
    assert!(cases.len() > 400);

    for (now, zone_input, range, start, end, expected) in cases {
        let (time_zone, zone) = resolve_time_zone(zone_input.as_deref());
        let config = range_config(
            Range::parse(&range).unwrap(),
            zone,
            start.as_deref(),
            end.as_deref(),
            instant(&now),
        );
        let buckets = buckets(&config);
        let edge = |bucket: &mymcps_web::routes::analytics::Bucket| {
            format!(
                "{}@{}-{}",
                bucket.label,
                bucket.start.seconds(),
                bucket.end.seconds()
            )
        };
        let sum: i64 = buckets
            .iter()
            .map(|bucket| bucket.start.seconds() + bucket.end.seconds())
            .sum();
        let answer = format!(
            "{time_zone} {} {} {} {} {} {} {} {sum}",
            config.range.as_str(),
            config.unit.as_str(),
            config.count,
            config.start.to_iso(),
            config.end.to_iso(),
            edge(&buckets[0]),
            edge(buckets.last().unwrap()),
        );
        assert_eq!(
            answer, expected,
            "now {now}, zone {zone_input:?}, range {range}, from {start:?} to {end:?}"
        );
        assert_eq!(buckets.len(), config.count);
    }
}

#[tokio::test]
async fn shows_times_in_the_zone_of_the_page() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |token| {
        token.name = "Date log token".into();
    })
    .await;
    create_mcp_call_log(&app, &token, None, |log| {
        log.requested_tool_name = "date__format".into();
        log.created_at = Timestamp::from(instant("2035-09-06T23:30:00Z"));
    })
    .await;

    let paris = app
        .get("/logs?range=all&timeZone=Europe%2FParis")
        .login_as(&admin)
        .send()
        .await
        .text();
    // Day first, to the second, on the clock of the zone that was asked for.
    assert!(
        paris.contains("<time datetime=\"2035-09-06T23:30:00.000Z\">07/09/2035, 01:30:00</time>")
    );
    assert!(paris.contains("Times shown in <span data-timezone-label>Europe/Paris (UTC+0"));

    let utc = app
        .get("/logs?range=all")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(
        utc.contains("<time datetime=\"2035-09-06T23:30:00.000Z\">06/09/2035, 23:30:00</time>")
    );
    assert!(utc.contains("<span data-timezone-label>UTC (UTC+00:00)</span>"));

    // A zone nobody knows is UTC.
    let unknown = app
        .get("/logs?range=all&timeZone=Mars%2FOlympus")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert_eq!(field_value(&unknown, "timeZone"), "UTC");
    assert!(unknown.contains("06/09/2035, 23:30:00"));
    assert_eq!(Zone::parse("Mars/Olympus"), None);
}

#[tokio::test]
async fn pages_through_the_calls() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |_| {}).await;
    let now = Utc::now();
    for index in 0..30 {
        create_mcp_call_log(&app, &token, None, |log| {
            log.requested_tool_name = format!("test__tool_{index:02}");
            log.tool_name = None;
            log.created_at = Timestamp::from(now - Duration::minutes(index));
        })
        .await;
    }
    let table = "<table class=\"table table--dense table--interactive\">";

    let first = app.get("/logs").login_as(&admin).send().await.text();
    assert!(first.contains("<span class=\"pagination__range\">1–25 of 30</span>"));
    let rows = table_rows(&first, table);
    assert_eq!(rows.len(), 25);
    // The most recent first; a call without a tool shows the name that was asked for.
    assert_eq!(rows[0][2], "test__tool_00");
    assert_eq!(rows[24][2], "test__tool_24");
    assert!(first.contains("aria-disabled=\"true\" aria-label=\"Previous page\""));
    assert!(first.contains("href=\"/logs?page=2\" aria-label=\"Next page\""));

    let second = app
        .get("/logs?pageSize=10&page=2&timeZone=Europe%2FParis")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(second.contains("<span class=\"pagination__range\">11–20 of 30</span>"));
    let rows = table_rows(&second, table);
    assert_eq!(rows.len(), 10);
    assert_eq!(rows[0][2], "test__tool_10");
    assert!(second.contains(
        "href=\"/logs?pageSize=10&amp;timeZone=Europe%2FParis\" aria-label=\"Previous page\""
    ));
    assert!(second.contains(
        "href=\"/logs?pageSize=10&amp;timeZone=Europe%2FParis&amp;page=3\" aria-label=\"Next page\""
    ));
    // A row opens the details of its call, and closing them comes back to this page.
    assert!(second.contains("&amp;page=2&amp;logId="));
    assert!(second.contains(
        "data-dialog-return=\"/logs?pageSize=10&amp;timeZone=Europe%2FParis&amp;page=2\""
    ));
    // Every page size the old page offered.
    for size in [10, 25, 50, 100] {
        assert!(second.contains(&format!(">{size} per page</a>")), "{size}");
    }
    assert!(
        second.contains(
            "href=\"/logs?pageSize=50&amp;timeZone=Europe%2FParis\" aria-checked=\"false\""
        )
    );

    // Past the last page there is nothing to list.
    let beyond = app.get("/logs?page=9").login_as(&admin).send().await.text();
    assert!(beyond.contains("No calls in this view"));
    let huge = app
        .get("/logs?page=99999999999999999999999")
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(huge.status, StatusCode::OK);
    assert!(huge.text().contains("No calls in this view"));
}

#[tokio::test]
async fn leaves_out_the_calls_before_the_period() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |_| {}).await;
    let now = Utc::now();
    for (name, age) in [
        ("an_hour_old", Duration::hours(1)),
        ("two_days_old", Duration::days(2)),
        ("ten_days_old", Duration::days(10)),
    ] {
        create_mcp_call_log(&app, &token, None, |log| {
            log.tool_name = Some(name.into());
            log.created_at = Timestamp::from(now - age);
        })
        .await;
    }
    let tools = |page: String| -> Vec<String> {
        table_rows(
            &page,
            "<table class=\"table table--dense table--interactive\">",
        )
        .into_iter()
        .map(|cells| cells[2].clone())
        .collect()
    };

    let day = app.get("/logs").login_as(&admin).send().await.text();
    assert_eq!(tools(day), ["an_hour_old"]);
    let week = app
        .get("/logs?range=7d")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert_eq!(tools(week), ["an_hour_old", "two_days_old"]);
    for range in ["30d", "all"] {
        let page = app
            .get(&format!("/logs?range={range}"))
            .login_as(&admin)
            .send()
            .await
            .text();
        assert_eq!(
            tools(page),
            ["an_hour_old", "two_days_old", "ten_days_old"],
            "{range}"
        );
    }
}

#[tokio::test]
async fn says_when_no_call_matches_and_when_logging_is_off() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let empty = app.get("/logs").login_as(&admin).send().await.text();
    assert!(empty.contains("<p class=\"empty-state__title\">No calls in this view</p>"));
    assert!(empty.contains("Change the filters or make a tool call through the MCP gateway."));
    assert!(!empty.contains("<table"));
    assert!(!empty.contains("Call logging is off"));
    // Nothing is filtered on: the empty state has no filter to clear.
    assert!(!empty.contains("class=\"button button--secondary\" href=\"/logs"));

    // A filter on a value that is in no menu stays in the form, and can be cleared.
    let filtered = app
        .get("/logs?range=7d&outcome=error&mcp=gone")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(filtered.contains("No calls in this view"));
    assert!(filtered.contains("Outcome: Error"));
    assert!(filtered.contains("MCP: gone"));
    assert!(filtered.contains("name=\"mcp\" value=\"gone\" checked>gone"));
    assert!(
        filtered.contains(
            "class=\"button button--secondary\" href=\"/logs?range=7d\">Clear filters</a>"
        )
    );
    assert!(
        filtered
            .contains("href=\"/logs?range=7d&amp;mcp=gone\" aria-label=\"Clear outcome filter\"")
    );
    assert!(
        filtered
            .contains("href=\"/logs?range=7d&amp;outcome=error\" aria-label=\"Clear MCP filter\"")
    );

    turn_logging_off(&app).await;
    let logs = app.get("/logs").login_as(&admin).send().await.text();
    assert!(logs.contains("<p class=\"banner__title\">Call logging is off</p>"));
    assert!(logs.contains(
        "Existing records remain available until retention removes them. Enable logging in Settings to capture new calls."
    ));
    let analytics = app.get("/analytics").login_as(&admin).send().await.text();
    assert!(analytics.contains("<p class=\"banner__title\">Call logging is off</p>"));
    assert!(analytics.contains(
        "Analytics includes retained records only. Enable logging in Settings to collect new data."
    ));
}

#[tokio::test]
async fn answers_the_page_script_with_the_part_it_asked_for() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |_| {}).await;
    let call = create_mcp_call_log(&app, &token, None, |log| {
        log.arguments_captured = true;
        log.response_captured = true;
    })
    .await;
    let fetch = |path: String, target: &'static str| {
        let (app, admin) = (&app, &admin);
        async move {
            app.get(&path)
                .login_as(admin)
                .header("x-requested-with", "fetch")
                .header("x-fragment", target)
                .send()
                .await
        }
    };

    // A change of filter: the filters and the list, without the page around them.
    let list = fetch("/logs?range=7d".into(), "logs").await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.header("cache-control"), Some("no-store"));
    let content = list.text();
    assert!(content.starts_with("<div class=\"toolbar toolbar--tight\">"));
    assert!(content.contains("id=\"logs-results\""));
    assert!(content.contains(&format!("id=\"log-{}\"", call.id)));
    assert!(!content.contains("<html") && !content.contains("id=\"call-details\""));

    // A row: the content of the panel.
    let details = fetch(format!("/logs?range=7d&logId={}", call.id), "call-details").await;
    assert_eq!(details.status, StatusCode::OK);
    assert_eq!(details.header("cache-control"), Some("no-store"));
    let content = details.text();
    assert!(content.starts_with("<div class=\"drawer__header\">"));
    assert!(
        content.contains("<h2 class=\"drawer__title\" id=\"call-details-title\">Call details</h2>")
    );
    assert!(content.contains("<span class=\"badge badge--success\">Success</span>"));
    // Capture was on, and the call had neither arguments nor an answer.
    assert!(content.contains("No arguments supplied"));
    assert!(content.contains("No MCP response received"));
    // The MCP of the call is not registered: there is nothing to open.
    assert!(!content.contains("Open MCP"));
    assert!(content.contains("Copy as JSON"));
    assert!(!content.contains("<html") && !content.contains("id=\"logs-results\""));

    // A call the retention removed meanwhile.
    let gone = fetch("/logs?logId=987654".into(), "call-details").await;
    assert_eq!(gone.status, StatusCode::OK);
    assert!(gone.text().contains("This call is no longer in the log"));

    // The live refresh takes the list out of the whole page.
    let refresh = fetch(format!("/logs?range=7d&logId={}", call.id), "logs-results").await;
    let page = refresh.text();
    assert!(page.contains("<html") && page.contains("id=\"logs-results\""));
    assert!(page.contains("aria-current=\"true\""));

    // Without script the same address is the page, with the panel open.
    let plain = app
        .get(&format!("/logs?range=7d&logId={}", call.id))
        .login_as(&admin)
        .send()
        .await
        .text();
    let panel = between(
        &plain,
        "<dialog class=\"drawer\" id=\"call-details\"",
        "</dialog>",
    );
    assert!(panel.contains("data-open") && panel.contains("data-dialog-return=\"/logs?range=7d\""));
    assert!(!panel.contains("Arguments were not captured"));
    // And a call that is gone leaves the panel closed.
    let closed = app
        .get("/logs?logId=987654")
        .login_as(&admin)
        .send()
        .await
        .text();
    let panel = between(
        &closed,
        "<dialog class=\"drawer\" id=\"call-details\"",
        "</dialog>",
    );
    assert!(!panel.contains("data-open"));
}

#[tokio::test]
async fn tells_what_was_not_captured_of_a_call() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |_| {}).await;
    let metadata_only = create_mcp_call_log(&app, &token, None, |log| {
        log.outcome = CallOutcome::Error;
        log.error_category = Some(CallErrorCategory::UpstreamException);
    })
    .await;
    let unreadable = create_mcp_call_log(&app, &token, None, |log| {
        log.arguments_captured = true;
        log.arguments = Some("not json <b>".into());
    })
    .await;

    let page = app
        .get(&format!("/logs?logId={}", metadata_only.id))
        .login_as(&admin)
        .send()
        .await
        .text();
    let panel = between(
        &page,
        "<dialog class=\"drawer\" id=\"call-details\"",
        "</dialog>",
    );
    for shown in [
        "<p class=\"banner__title\">upstream exception</p><p>No error summary available.</p>",
        "<p class=\"banner__title\">Arguments were not captured</p><p>The metadata logging level was active for this call.</p>",
        "<p class=\"banner__title\">Response was not captured</p><p>Response capture was not active for this call.</p>",
        "<dt>MCP</dt><dd>Unknown</dd>",
        "<dt>Caller IP</dt><dd>Unknown</dd>",
        "<dt>Duration</dt><dd>25 ms</dd>",
    ] {
        assert!(panel.contains(shown), "{shown}");
    }

    // What is not JSON is shown as it was stored, and never as markup.
    let page = app
        .get(&format!("/logs?logId={}", unreadable.id))
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(page.contains("id=\"call-arguments\">not json &lt;b&gt;</pre>"));
    assert!(page.contains("&quot;arguments&quot;: &quot;not json &lt;b&gt;&quot;"));
}

#[tokio::test]
async fn shows_an_empty_period_and_follows_the_viewers_time_zone() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let response = app.get("/analytics").login_as(&admin).send().await;
    let page = response.text();
    // Seven days of UTC, until the page script names the viewer's zone.
    assert!(page.contains(
        "href=\"/analytics?range=7d&amp;timeZone=UTC\" aria-current=\"true\">7 days</a>"
    ));
    let sync = between(&page, "data-timezone-sync hidden>", "</form>");
    assert!(sync.contains("name=\"range\" value=\"7d\""));
    assert!(sync.contains("name=\"timeZone\" value=\"UTC\" data-timezone"));
    assert_eq!(metric(&page, "Total calls"), "0");
    assert_eq!(metric(&page, "Success rate"), "0.0%");
    assert_eq!(metric(&page, "Error rate"), "0.0%");
    assert_eq!(metric(&page, "Average duration"), "0 ms");
    assert!(page.contains("<p class=\"empty-state__title\">No calls in this period</p>"));
    assert!(
        page.contains("Choose another time range or make a tool call through the MCP gateway.")
    );
    assert!(!page.contains("line-chart") && !page.contains("Top MCPs"));
    // Live is a switch, off when the page loads.
    assert!(page.contains("aria-pressed=\"false\" aria-labelledby=\"live-label\" data-live-refresh=\"#analytics-data\" data-live-interval=\"30000\""));
    assert!(page.contains("<a class=\"button button--secondary\" href=\"/logs\">View logs</a>"));

    let day = app
        .get("/analytics?range=24h&timeZone=UTC")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(day.contains("aria-current=\"true\">24 hours</a>"));

    for query in [
        "range=1y",
        "range[]=7d",
        &format!("timeZone={}", "x".repeat(101)),
    ] {
        let invalid = app
            .get(&format!("/analytics?{query}"))
            .login_as(&admin)
            .send()
            .await;
        assert_eq!(invalid.status, StatusCode::FOUND, "{query}");
        assert!(invalid.flashed("errors").is_some(), "{query}");
    }
}

#[tokio::test]
async fn draws_the_timeline_on_the_server() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |_| {}).await;
    let now = Utc::now();
    for hours in 0..3 {
        create_mcp_call_log(&app, &token, None, |log| {
            log.created_at = Timestamp::from(now - Duration::hours(hours));
        })
        .await;
    }

    let response = app
        .get("/analytics?range=24h&timeZone=UTC")
        .login_as(&admin)
        .send()
        .await;
    let page = response.text();
    assert!(page.contains(
        "<h2 class=\"card__title\" id=\"timeline-title\">Calls and errors over time</h2>"
    ));
    assert!(page.contains("Successful and failed tool-call attempts in UTC (UTC+00:00)."));
    let chart = between(&page, "<div class=\"line-chart\">", "</table>");
    assert!(chart.contains("<svg class=\"line-chart__svg\" viewBox=\"0 0 23 4\" preserveAspectRatio=\"none\" role=\"img\" aria-label=\"Calls and errors over the selected time range\">"));
    assert_eq!(chart.matches("class=\"line-chart__hit\"").count(), 24);
    assert_eq!(chart.matches("class=\"line-chart__point\"").count(), 48);
    // The last three hours hold one call each.
    let timeline = timeline_of(&page);
    assert_eq!(timeline.len(), 24);
    assert_eq!(timeline[23].0, now.format("%H:00").to_string());
    assert_eq!(
        timeline[20..]
            .iter()
            .map(|(_, calls, _)| calls.as_str())
            .collect::<Vec<_>>(),
        ["0", "1", "1", "1"]
    );
    // The policy forbids inline styles and scripts: the chart needs neither.
    assert!(!page.contains(" style=") && !page.contains("<style"));
    assert_eq!(page.matches("<script").count(), 1);
}

#[tokio::test]
async fn applies_and_preserves_a_custom_date_and_time_range() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let submit = |query: String, script: bool| {
        let (app, admin) = (&app, &admin);
        async move {
            let request = app.get(&format!("/analytics?{query}")).login_as(admin);
            if script {
                request.header("x-requested-with", "fetch").send().await
            } else {
                request.send().await
            }
        }
    };

    // The dialog is in the page, on the period the page shows.
    let page = submit("range=24h&timeZone=UTC".into(), false).await.text();
    let dialog = between(
        &page,
        "<dialog class=\"dialog dialog--sm\" id=\"custom-range\"",
        "</dialog>",
    );
    assert!(!dialog.contains("data-open"));
    assert!(dialog.contains(
        "<h2 class=\"dialog__title\" id=\"custom-range-title\">Custom analytics range</h2>"
    ));
    assert!(dialog.contains("Choose exact times in UTC (UTC+00:00)"));
    assert!(dialog.contains("<form method=\"get\" action=\"/analytics\" data-async>"));
    for field in ["startDate", "endDate", "startTime", "endTime"] {
        assert_eq!(
            dialog.matches(&format!("name=\"{field}\"")).count(),
            1,
            "{field}"
        );
    }
    assert!(dialog.contains(">Apply range</button>"));

    // Applied as it is, it becomes the same period as two instants in UTC.
    let fields = |page: &str| {
        [
            "range",
            "timeZone",
            "start",
            "end",
            "startDate",
            "endDate",
            "startTime",
            "endTime",
        ]
        .map(|name| {
            format!(
                "{name}={}",
                field_value(between(page, "id=\"custom-range\"", "</dialog>"), name)
                    .replace(':', "%3A")
                    .replace('/', "%2F")
            )
        })
        .join("&")
    };
    let applied = submit(fields(&page), false).await;
    assert_eq!(applied.status, StatusCode::FOUND);
    let location = applied.location().unwrap().to_string();
    let hour = Utc::now().duration_trunc(Duration::hours(1)).unwrap();
    assert_eq!(
        location,
        format!(
            "/analytics?range=custom&start={}&end={}&timeZone=UTC",
            address_instant(hour - Duration::hours(23)).replace(':', "%3A"),
            address_instant(hour + Duration::hours(1)).replace(':', "%3A"),
        )
    );
    // The page script is told where to go instead of being redirected.
    let scripted = submit(fields(&page), true).await;
    assert_eq!(scripted.status, StatusCode::NO_CONTENT);
    assert_eq!(scripted.header("x-location"), Some(location.as_str()));

    // A link someone shared keeps its instants and its zone, whatever the
    // zone of who opens it: nothing in the page asks the script to resubmit.
    let shared = submit(
        "range=custom&start=2026-10-24T00%3A30Z&end=2026-10-24T01%3A30Z&timeZone=Europe%2FParis"
            .into(),
        false,
    )
    .await
    .text();
    assert!(shared.contains("aria-current=\"page\">Custom</a>"));
    assert!(!shared.contains("data-timezone-sync"));
    assert!(!shared.contains("data-timezone>"));
    assert_eq!(field_value(&shared, "start"), "2026-10-24T00:30Z");
    assert_eq!(field_value(&shared, "end"), "2026-10-24T01:30Z");
    assert_eq!(field_value(&shared, "timeZone"), "Europe/Paris");
    assert!(shared.contains("Choose exact times in Europe/Paris (UTC+0"));

    // 02:30 happens twice in Paris on 25 October 2026: typed as a start it is
    // the first one, typed as an end the second one.
    let folded = "range=custom&timeZone=Europe%2FParis&start=2026-10-24T00%3A30Z&end=2026-10-24T01%3A30Z\
        &startDate=2026-10-25&startTime=02%3A30&endDate=2026-10-25&endTime=02%3A30";
    let applied = submit(folded.into(), false).await;
    assert_eq!(
        applied.location(),
        Some(
            "/analytics?range=custom&start=2026-10-25T00%3A30Z&end=2026-10-25T01%3A30Z&timeZone=Europe%2FParis"
        )
    );
    // Opened again on that range and applied without a change, it stays the same.
    let reapplied = "range=custom&timeZone=Europe%2FParis&start=2026-10-25T00%3A30Z&end=2026-10-25T01%3A30Z\
        &startDate=2026-10-25&startTime=02%3A30&endDate=2026-10-25&endTime=02%3A30";
    assert_eq!(
        submit(reapplied.into(), false).await.location(),
        applied.location()
    );
}

#[tokio::test]
async fn refuses_a_custom_range_it_cannot_use() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let dialog_fields = |start_date: &str, start_time: &str, end_date: &str, end_time: &str| {
        format!(
            "/analytics?range=custom&timeZone=Europe%2FParis&startDate={start_date}&startTime={}&endDate={end_date}&endTime={}",
            start_time.replace(':', "%3A"),
            end_time.replace(':', "%3A")
        )
    };

    for (path, message) in [
        (
            dialog_fields("2026-10-01", "00:00", "2026-10-08", ""),
            "Choose both a start and an end time.",
        ),
        (
            dialog_fields("", "00:00", "2026-10-08", "00:00"),
            "Choose both a start and an end time.",
        ),
        // 02:30 does not exist in Paris on 29 March 2026.
        (
            dialog_fields("2026-03-29", "02:30", "2026-03-29", "04:00"),
            "Enter valid dates and times.",
        ),
        (
            dialog_fields("2026-10-01", "noon", "2026-10-08", "00:00"),
            "Enter valid dates and times.",
        ),
        (
            dialog_fields("2026-10-08", "00:00", "2026-10-01", "00:00"),
            "End time must be after start time.",
        ),
        (
            dialog_fields("2025-01-01", "00:00", "2026-01-02", "00:00"),
            "Custom ranges can span up to 365 days.",
        ),
    ] {
        // To the page script: the form again, with what is wrong under the end time.
        let scripted = app
            .get(&path)
            .login_as(&admin)
            .header("x-requested-with", "fetch")
            .send()
            .await;
        assert_eq!(
            scripted.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{message}"
        );
        let form = scripted.text();
        assert!(
            form.starts_with("<form method=\"get\" action=\"/analytics\" data-async>"),
            "{message}"
        );
        assert!(
            form.contains(&format!(
                "<p class=\"field__error\" id=\"custom-range-error\">{message}</p>"
            )),
            "{message}"
        );
        assert!(form.contains("aria-invalid=\"true\" aria-describedby=\"custom-range-error\""));

        // Without script: the page, with the dialog open on the same message.
        let plain = app.get(&path).login_as(&admin).send().await;
        assert_eq!(plain.status, StatusCode::UNPROCESSABLE_ENTITY, "{message}");
        let page = plain.text();
        let dialog = between(
            &page,
            "<dialog class=\"dialog dialog--sm\" id=\"custom-range\"",
            "</dialog>",
        );
        assert!(
            dialog.contains("data-open") && dialog.contains(message),
            "{message}"
        );
        assert!(page.contains("<html"));
    }

    // What was typed is still in the fields.
    let typed = app
        .get(&dialog_fields("2026-10-08", "09:15", "2026-10-01", "10:45"))
        .login_as(&admin)
        .send()
        .await
        .text();
    assert_eq!(field_value(&typed, "startDate"), "2026-10-08");
    assert_eq!(field_value(&typed, "startTime"), "09:15");
    assert_eq!(field_value(&typed, "endDate"), "2026-10-01");
    assert_eq!(field_value(&typed, "endTime"), "10:45");

    // "Custom" without script: the last 7 days, and the dialog to choose from.
    let asked = app
        .get("/analytics?range=custom&timeZone=Europe%2FParis")
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(asked.status, StatusCode::OK);
    let page = asked.text();
    let dialog = between(
        &page,
        "<dialog class=\"dialog dialog--sm\" id=\"custom-range\"",
        "</dialog>",
    );
    assert!(dialog.contains("data-open"));
    assert!(
        dialog.contains("data-dialog-return=\"/analytics?range=7d&amp;timeZone=Europe%2FParis\"")
    );
    assert!(!dialog.contains("field__error"));
    assert!(page.contains("aria-current=\"true\">7 days</a>"));
    // The viewer's zone still applies, and keeps the dialog open.
    let sync = between(&page, "data-timezone-sync hidden>", "</form>");
    assert!(sync.contains("name=\"range\" value=\"custom\""));
}
