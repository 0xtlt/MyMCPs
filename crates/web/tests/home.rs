//! The home page. The Node app's was three links; this one is the "01 Home"
//! screen of the redesign, so these tests are new: what each kind of person
//! is shown, and where each figure comes from.

mod support;

use std::collections::HashMap;

use chrono::{Duration, Utc};
use http::StatusCode;
use mymcps_core::Timestamp;
use mymcps_core::models::{
    CallErrorCategory, CallOutcome, InstanceSetting, McpLogLevel, McpStatus, McpTransport,
    TokenSource, User,
};
use mymcps_web::routes::analytics::Zone;
use mymcps_web::routes::home::{HomeQuery, load};
use mymcps_web::testing::TestApp;
use mymcps_web::testing::factories::{create_admin, create_invite, create_mcp, create_member};
use mymcps_web::views::home::home_page;
use mymcps_web::views::shell::PageContext;
use serde_json::{Map, json};
use support::*;

async fn change_settings(app: &TestApp, change: impl FnOnce(&mut InstanceSetting)) {
    let mut settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    change(&mut settings);
    settings.save(&*app.core.db).await.unwrap();
}

/// An instance with something of everything the page reports on.
async fn populate(app: &TestApp, admin: &User) {
    let now = Timestamp::now();
    // Calls of the period before the chart outlive the default retention only here.
    change_settings(app, |settings| settings.mcp_log_retention_days = 60).await;

    let notion = create_mcp(app, admin.id, |mcp| {
        mcp.name = "Notion".into();
        mcp.slug = "notion".into();
    })
    .await;
    create_mcp(app, admin.id, |mcp| {
        mcp.name = "Strava".into();
        mcp.slug = "strava".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("strava".into());
        mcp.status = McpStatus::Draft;
        mcp.oauth_required = true;
    })
    .await;
    create_mcp(app, admin.id, |mcp| {
        mcp.name = "Sentry".into();
        mcp.slug = "sentry".into();
        mcp.status = McpStatus::Error;
        mcp.last_error = Some("OAuth token rejected (HTTP 401)".into());
    })
    .await;
    create_mcp(app, admin.id, |mcp| {
        mcp.name = "Retired".into();
        mcp.slug = "retired".into();
        mcp.enabled = false;
    })
    .await;

    let claude = create_stored_access_token(app, admin.id, |token| {
        token.name = "Claude Code".into();
        token.source = TokenSource::Oauth;
        token.expires_at = Some(now + Duration::hours(1));
        token.oauth_refresh_expires_at = Some(now + Duration::days(30));
        token.last_used_at = Some(now - Duration::minutes(6));
    })
    .await;
    create_stored_access_token(app, admin.id, |token| {
        token.name = "n8n workflows".into();
        token.expires_at = Some(now + Duration::days(3));
        token.last_used_at = Some(now - Duration::days(2));
    })
    .await;
    create_stored_access_token(app, admin.id, |token| {
        token.name = "CI runner".into();
        token.expires_at = Some(now - Duration::days(7));
    })
    .await;
    create_stored_access_token(app, admin.id, |token| {
        token.name = "Old laptop".into();
        token.revoked_at = Some(now - Duration::days(1));
    })
    .await;

    // The latest call is the last one made.
    for (duration_ms, failed) in [
        (100, false),
        (200, true),
        (300, false),
        (400, true),
        (500, false),
    ] {
        create_mcp_call_log(app, &claude, Some(&notion), |log| {
            log.requested_tool_name = "notion__search".into();
            log.tool_name = Some("search".into());
            log.duration_ms = duration_ms;
            if failed {
                log.outcome = CallOutcome::Error;
                log.error_category = Some(CallErrorCategory::ToolError);
            }
        })
        .await;
    }
    for _ in 0..4 {
        create_mcp_call_log(app, &claude, Some(&notion), |log| {
            log.created_at = now - Duration::days(20);
        })
        .await;
    }

    create_member(app).await;
    create_invite(app, admin.id, |_| {}).await;
    create_invite(app, admin.id, |invite| {
        invite.expires_at = now - Duration::days(1);
    })
    .await;
}

/// The value and the footer of the KPI card with this label, as text.
fn kpi(page: &str, label: &str) -> (String, String) {
    let card = between(
        page,
        &format!("<p class=\"kpi__label\">{label}</p>"),
        "</div>",
    );
    (
        text_of(between(card, "<p class=\"kpi__value\">", "</p>")),
        text_of(between(card, "<p class=\"kpi__footer\">", "</p>")),
    )
}

/// The figure of the context bar under this label.
fn stat<'a>(page: &'a str, label: &str) -> &'a str {
    let value = between(
        page,
        &format!("<dt class=\"stat__label\">{label}</dt><dd class=\"stat__value\""),
        "</dd>",
    );
    &value[value.find('>').unwrap() + 1..]
}

/// The rows of the card whose title has this id: title, subtitle and badge.
fn rows_of(page: &str, title_id: &str) -> Vec<(String, String, String)> {
    let card = between(page, &format!("id=\"{title_id}\""), "</section>");
    card.split("<a class=\"list-row")
        .skip(1)
        .map(|row| {
            let badge = match row.find("<span class=\"badge") {
                Some(start) => text_of(between(&row[start..], ">", "</span>")),
                None => String::new(),
            };
            (
                text_of(
                    between(row, "<span class=\"list-row__title", "</span>")
                        .split_once('>')
                        .unwrap()
                        .1,
                ),
                text_of(between(
                    row,
                    "<span class=\"list-row__subtitle\">",
                    "</span></span>",
                )),
                badge,
            )
        })
        .collect()
}

#[tokio::test]
async fn shows_an_administrator_the_whole_dashboard() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    populate(&app, &admin).await;

    let response = app.get("/").login_as(&admin).send().await;
    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();
    assert!(page.contains("<title>Home · MyMCPs</title>"));

    // The context bar: the period and its figures.
    let bar = between(&page, "<div class=\"context-bar hide-mobile\">", "</dl>");
    assert!(bar.contains("popovertarget=\"home-range\">Last 7 days"));
    assert!(
        bar.contains("role=\"menuitemradio\" aria-checked=\"true\" href=\"/\">Last 7 days</a>")
    );
    assert!(bar.contains("aria-checked=\"false\" href=\"/?range=24h\">Last 24 hours</a>"));
    assert!(bar.contains("aria-checked=\"false\" href=\"/?range=30d\">Last 30 days</a>"));
    assert_eq!(stat(&page, "Tool calls"), "5");
    assert_eq!(stat(&page, "Success rate"), "60.0%");
    assert_eq!(stat(&page, "Avg. duration"), "300 ms");
    assert_eq!(stat(&page, "Tools exposed"), "—");

    // The greeting, which the page script turns into the one of the hour.
    assert!(page.contains(
        "<h1 class=\"hero__greeting\" data-greeting=\"Welcome back\">Welcome back, Test.<span>3 MCPs behind one endpoint.</span></h1>"
    ));

    // The gateway endpoint.
    let card = between(
        &page,
        "<div class=\"hero-card\">",
        "<div class=\"attention\">",
    );
    assert!(card.contains("<span class=\"badge badge--success\">Online</span>"));
    assert!(card.contains(
        "<span class=\"copy-field__value\" id=\"gateway-url\">http://localhost:3333/mcp</span>"
    ));
    assert!(card.contains("data-copy-target=\"#gateway-url\" aria-label=\"Copy gateway URL\""));
    assert!(card.contains("<a class=\"chip\" href=\"/settings\">Tool mode: eager</a>"));
    assert!(card.contains("popovertarget=\"home-client\">Client: Claude Code"));
    assert!(card.contains("aria-checked=\"false\" href=\"/?client=codex\">Codex</a>"));
    assert!(card.contains("aria-checked=\"true\" href=\"/\">Claude Code</a>"));
    assert!(card.contains("aria-checked=\"false\" href=\"/?client=cursor\">Cursor</a>"));
    assert!(card.contains(
        "<a class=\"button button--primary\" href=\"/tokens?install=1&amp;client=claude\">"
    ));
    assert!(card.contains("Install in a client</a>"));

    // What needs someone, each with where it is dealt with.
    let attention = between(&page, "<div class=\"attention\">", "</section>");
    let items: Vec<(&str, String)> = attention
        .split("<a class=\"attention__item\" href=\"")
        .skip(1)
        .map(|item| {
            (
                item.split('"').next().unwrap(),
                text_of(&item[item.find('>').unwrap() + 1..]),
            )
        })
        .collect();
    let strava: i64 = sqlx::query_scalar("select `id` from `mcps` where `slug` = 'strava'")
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    let sentry: i64 = sqlx::query_scalar("select `id` from `mcps` where `slug` = 'sentry'")
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(
        items,
        [
            (
                format!("/mcps/{strava}/edit").as_str(),
                "Strava needs authorization".to_string()
            ),
            (
                format!("/mcps/{sentry}/edit").as_str(),
                "Sentry is in error".to_string()
            ),
            ("/tokens", "1 token expires this week".to_string()),
            (
                "/logs?range=7d&amp;outcome=error",
                "2 errors in the last 7 days".to_string()
            ),
        ]
    );

    // The four figures.
    assert!(page.contains("<div class=\"grid grid--4 grid--keep-2\">"));
    assert_eq!(
        kpi(&page, "MCPs"),
        ("4".into(), "+4in the last 30 daysin 30 days".into())
    );
    assert_eq!(
        kpi(&page, "Tools exposed"),
        ("—".into(), "listed on first use".into())
    );
    assert_eq!(
        kpi(&page, "Active tokens"),
        ("2".into(), "2used this week".into())
    );
    assert_eq!(
        kpi(&page, "Teammates"),
        ("2".into(), "1pending invite".into())
    );

    // The activity of the last 14 days, against the 14 days before.
    let activity = between(
        &page,
        "<section class=\"card\" id=\"home-activity\"",
        "</section>",
    );
    assert!(activity.contains("<p class=\"card__subtitle\">Tool calls · last 14 days</p>"));
    assert!(activity.contains("<a class=\"button hide-mobile\" href=\"/logs\">Open logs</a>"));
    assert_eq!(
        text_of(between(activity, "<p class=\"chart-total\">", "</p>")),
        "5+25%including 2 errors"
    );
    let bars: Vec<&str> = activity
        .split("class=\"bar-chart__bar\" data-pct=\"")
        .skip(1)
        .map(|bar| bar.split('"').next().unwrap())
        .collect();
    assert_eq!(bars.len(), 14);
    assert!(bars[..13].iter().all(|share| *share == "0") && bars[13] == "100");
    let today = Utc::now();
    assert!(activity.contains(&format!("title=\"{}: 5 calls\"", today.format("%-d %b"))));
    assert_eq!(
        text_of(between(
            activity,
            "<div class=\"chart-axis hide-mobile\" aria-hidden=\"true\">",
            "</div>"
        )),
        format!(
            "{}{}{}",
            (today - Duration::days(13)).format("%-d %b"),
            (today - Duration::days(7)).format("%-d %b"),
            today.format("%-d %b")
        )
    );
    // The five latest calls, each opening its details in the Logs page.
    let recent = rows_of(&page, "home-activity");
    assert_eq!(recent.len(), 5);
    assert_eq!(
        recent[0],
        (
            "notion__search".into(),
            "Notion · Claude Code".into(),
            "Success".into()
        )
    );
    assert_eq!(recent[1].2, "Error");
    assert_eq!(
        activity
            .matches("href=\"/logs?range=all&amp;logId=")
            .count(),
        5
    );
    assert_eq!(activity.matches("data-format=\"relative\"").count(), 5);

    // The MCPs that need someone first, then the others.
    assert_eq!(
        rows_of(&page, "home-mcps-title"),
        [
            (
                "Strava".to_string(),
                "Built-in · awaiting OAuth".to_string(),
                "draft".to_string()
            ),
            (
                "Sentry".into(),
                "OAuth token rejected (HTTP 401)".into(),
                "error".into()
            ),
            ("Notion".into(), "HTTP".into(), "ready".into()),
            ("Retired".into(), "HTTP · disabled".into(), "ready".into()),
        ]
    );
    // The tokens, the most recently used first.
    let tokens = rows_of(&page, "home-tokens-title");
    assert_eq!(
        tokens
            .iter()
            .map(|(name, _, badge)| (name.as_str(), badge.as_str()))
            .collect::<Vec<_>>(),
        [
            ("Claude Code", "Active"),
            ("n8n workflows", "Expiring"),
            ("Old laptop", "Revoked"),
            ("CI runner", "Expired"),
        ]
    );
    assert!(tokens[0].1.starts_with("OAuth · used "));
    assert!(tokens[1].1.starts_with("Manual · expires "));
    assert!(tokens[2].1.starts_with("Manual · revoked "));
    assert!(tokens[3].1.starts_with("Manual · expired "));
    // Nothing says when an MCP was last tested.
    assert!(!page.contains("tested"));
}

#[tokio::test]
async fn shows_a_member_nothing_of_the_call_log() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    populate(&app, &admin).await;
    let member = create_member(&app).await;

    let response = app.get("/").login_as(&member).send().await;
    assert_eq!(response.status, StatusCode::OK);
    let page = response.text();

    // The home of a member: its header, not the greeting over the figures.
    assert!(page.contains("<h1 class=\"page-header__title\">Home</h1>"));
    assert!(page.contains(
        "Signed in as Test User. Register upstream MCPs and issue access tokens for your agents."
    ));
    for hidden in [
        "context-bar",
        "hero__greeting",
        "Gateway activity",
        "home-activity",
        "bar-chart",
        "Teammates",
        "errors in the last",
        "notion__search",
        "data-timezone",
        "href=\"/logs",
        "href=\"/analytics",
        "href=\"/settings\">Tool mode",
    ] {
        assert!(!page.contains(hidden), "{hidden}");
    }

    // What a member also sees on the MCPs and Access tokens pages.
    assert!(page.contains(
        "<span class=\"copy-field__value\" id=\"gateway-url\">http://localhost:3333/mcp</span>"
    ));
    assert!(page.contains("<span class=\"chip\">Tool mode: eager</span>"));
    assert!(page.contains("href=\"/tokens?install=1&amp;client=claude\""));
    let attention = text_of(between(&page, "<div class=\"attention\">", "</section>"));
    assert_eq!(
        attention,
        "Strava needs authorization Sentry is in error 1 token expires this week"
    );
    assert!(page.contains("<div class=\"grid grid--3\">"));
    assert_eq!(kpi(&page, "MCPs").0, "4");
    assert_eq!(kpi(&page, "Active tokens").0, "2");
    assert_eq!(rows_of(&page, "home-mcps-title").len(), 4);
    assert_eq!(rows_of(&page, "home-tokens-title").len(), 4);
}

#[tokio::test]
async fn invites_to_add_the_first_mcp() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;

    let page = app.get("/").login_as(&admin).send().await.text();
    assert!(page.contains("Welcome back, Test.<span>Add your first MCP to get started.</span>"));
    assert!(page.contains("<p class=\"empty-state__title\">No MCPs yet</p>"));
    assert!(page.contains(
        "<p class=\"empty-state__description\">Create an MCP to start routing agent traffic.</p>"
    ));
    assert!(page.contains("<a class=\"button button--primary\" href=\"/mcps/new\">"));
    for absent in [
        "hero-card",
        "kpi__label",
        "home-activity",
        "home-mcps-title",
    ] {
        assert!(!page.contains(absent), "{absent}");
    }

    let page = app.get("/").login_as(&member).send().await.text();
    assert!(page.contains("<h1 class=\"page-header__title\">Home</h1>"));
    assert!(page.contains("<p class=\"empty-state__title\">No MCPs yet</p>"));
    assert!(page.contains("href=\"/mcps/new\""));
    assert!(!page.contains("hero-card") && !page.contains("kpi__label"));
}

#[tokio::test]
async fn reads_the_period_and_the_client_from_the_address() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = create_stored_access_token(&app, admin.id, |_| {}).await;
    let mcp = create_mcp(&app, admin.id, |_| {}).await;
    let now = Timestamp::now();
    for (age, failed) in [
        (Duration::hours(1), true),
        (Duration::days(3), true),
        (Duration::days(10), false),
    ] {
        create_mcp_call_log(&app, &token, Some(&mcp), |log| {
            log.created_at = now - age;
            log.duration_ms = 40;
            if failed {
                log.outcome = CallOutcome::Error;
            }
        })
        .await;
    }
    let home = |query: &'static str| {
        let (app, admin) = (&app, &admin);
        async move {
            app.get(&format!("/{query}"))
                .login_as(admin)
                .send()
                .await
                .text()
        }
    };

    let day = home("?range=24h&client=cursor").await;
    assert!(day.contains("popovertarget=\"home-range\">Last 24 hours"));
    assert_eq!(stat(&day, "Tool calls"), "1");
    assert_eq!(stat(&day, "Success rate"), "0.0%");
    assert!(day.contains("href=\"/logs?range=24h&amp;outcome=error\""));
    assert!(day.contains("1 error in the last 24 hours"));
    // Each menu keeps the choice of the other.
    assert!(day.contains("aria-checked=\"false\" href=\"/?client=cursor\">Last 7 days</a>"));
    assert!(day.contains(
        "aria-checked=\"true\" href=\"/?range=24h&amp;client=cursor\">Last 24 hours</a>"
    ));
    assert!(day.contains("popovertarget=\"home-client\">Client: Cursor"));
    assert!(day.contains("aria-checked=\"false\" href=\"/?range=24h\">Claude Code</a>"));
    assert!(day.contains("href=\"/tokens?install=1&amp;client=cursor\""));

    let week = home("").await;
    assert_eq!(stat(&week, "Tool calls"), "2");
    assert!(week.contains("2 errors in the last 7 days"));

    let month = home("?range=30d&client=codex").await;
    assert_eq!(stat(&month, "Tool calls"), "3");
    assert_eq!(stat(&month, "Success rate"), "33.3%");
    assert_eq!(stat(&month, "Avg. duration"), "40 ms");
    assert!(month.contains("2 errors in the last 30 days"));
    assert!(month.contains("popovertarget=\"home-client\">Client: Codex"));

    // The query string of a page that sent the person here is not this page's.
    let elsewhere = home("?range=all&outcome=error&client=vim&page=3").await;
    assert!(elsewhere.contains("popovertarget=\"home-range\">Last 7 days"));
    assert!(elsewhere.contains("popovertarget=\"home-client\">Client: Claude Code"));
    assert_eq!(stat(&elsewhere, "Tool calls"), "2");
}

#[tokio::test]
async fn shows_no_rate_without_calls_to_compute_it_from() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |_| {}).await;

    // No call in the period.
    let page = app.get("/").login_as(&admin).send().await.text();
    assert_eq!(stat(&page, "Tool calls"), "0");
    assert_eq!(stat(&page, "Success rate"), "—");
    assert_eq!(stat(&page, "Avg. duration"), "—");
    assert!(!page.contains("errors in the last"));
    let activity = between(
        &page,
        "<section class=\"card\" id=\"home-activity\"",
        "</section>",
    );
    assert!(
        activity.contains("<p class=\"empty-state__title\">No tool calls in the last 14 days</p>")
    );
    assert!(activity.contains("Make a tool call through the MCP gateway to see it here."));
    assert!(!activity.contains("bar-chart") && !activity.contains("list-row"));

    // Logging off: what was recorded before is counted, and says nothing of now.
    let token = create_stored_access_token(&app, admin.id, |_| {}).await;
    create_mcp_call_log(&app, &token, Some(&mcp), |_| {}).await;
    change_settings(&app, |settings| settings.mcp_log_level = McpLogLevel::Off).await;
    let page = app.get("/").login_as(&admin).send().await.text();
    assert_eq!(stat(&page, "Tool calls"), "1");
    assert!(page.contains(
        "<dt class=\"stat__label\">Success rate</dt><dd class=\"stat__value\" title=\"Call logging is off\">—</dd>"
    ));
    assert!(page.contains(
        "<dt class=\"stat__label\">Avg. duration</dt><dd class=\"stat__value\" title=\"Call logging is off\">—</dd>"
    ));
    // One call, and nothing before it to compare with: no change badge.
    let activity = between(
        &page,
        "<section class=\"card\" id=\"home-activity\"",
        "</section>",
    );
    assert_eq!(
        text_of(between(activity, "<p class=\"chart-total\">", "</p>")),
        "1including 0 errors"
    );

    // Logging off and nothing recorded: the card says why it is empty.
    sqlx::query("delete from `mcp_call_logs`")
        .execute(&*app.core.db)
        .await
        .unwrap();
    let page = app.get("/").login_as(&admin).send().await.text();
    assert!(page.contains("Call logging is off. Enable logging in Settings to capture new calls."));
    assert!(
        page.contains("<a class=\"button button--secondary\" href=\"/settings\">Open settings</a>")
    );
}

/// A page context as the request of this person would build it.
fn context_of(app: &TestApp, user: &User, pending_approvals: i64) -> PageContext {
    PageContext {
        user: Some(user.clone()),
        path: "/".into(),
        csrf: "token".into(),
        nonce: "nonce".into(),
        flash_success: None,
        flash_error: None,
        pending_approvals,
        app_url: app.core.config.public_app_url(),
        app_url_configured: app.core.config.public_oauth_app_url().is_some(),
        sidebar_collapsed: false,
        is_fetch: false,
    }
}

#[tokio::test]
async fn counts_the_tools_of_the_mcps_that_listed_them() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let first = create_mcp(&app, admin.id, |_| {}).await;
    let second = create_mcp(&app, admin.id, |_| {}).await;
    let never_listed = create_mcp(&app, admin.id, |_| {}).await;
    let disabled = create_mcp(&app, admin.id, |mcp| mcp.enabled = false).await;
    let render = |counts: HashMap<i64, usize>, pending_approvals: i64| {
        let (app, admin) = (&app, &admin);
        async move {
            let home = load(
                &app.state,
                admin,
                HomeQuery::read(&Map::new()),
                Zone::Utc,
                pending_approvals,
                &counts,
            )
            .await
            .unwrap();
            home_page(&context_of(app, admin, pending_approvals), &home).into_string()
        }
    };

    // What the server answers until an MCP is asked for its tools.
    assert!(app.state.known_tool_counts().is_empty());
    let page = render(HashMap::new(), 0).await;
    assert_eq!(
        kpi(&page, "Tools exposed"),
        ("—".into(), "listed on first use".into())
    );
    assert_eq!(stat(&page, "Tools exposed"), "—");
    assert!(!page.contains("wait for approval"));

    let counts = HashMap::from([(first.id, 1_200), (second.id, 34), (disabled.id, 500)]);
    let page = render(counts, 2).await;
    assert_eq!(
        kpi(&page, "Tools exposed"),
        ("1,234".into(), "across 2 enabled MCPs2 enabled MCPs".into())
    );
    assert_eq!(stat(&page, "Tools exposed"), "1,234");
    // Tool calls that wait for this person, as the navigation counts them.
    assert!(page.contains("<a class=\"attention__item\" href=\"/approvals\">"));
    assert!(page.contains("2 tool calls wait for approval"));

    let page = render(HashMap::from([(never_listed.id, 7)]), 1).await;
    assert_eq!(
        kpi(&page, "Tools exposed"),
        ("7".into(), "across 1 enabled MCP1 enabled MCP".into())
    );
    assert!(page.contains("1 tool call waits for approval"));
}

#[tokio::test]
async fn sums_up_the_ready_mcps_when_there_are_many() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    for index in 0..5 {
        create_mcp(&app, admin.id, |mcp| {
            mcp.name = format!("Ready {index}");
            mcp.enabled = index != 4;
        })
        .await;
    }

    let page = app.get("/").login_as(&admin).send().await.text();
    assert!(page.contains("<span>4 MCPs behind one endpoint.</span>"));
    assert_eq!(
        rows_of(&page, "home-mcps-title"),
        [(
            "5 MCPs".to_string(),
            "4 enabled · 1 disabled".to_string(),
            "ready".to_string()
        )]
    );
    assert!(!page.contains("<div class=\"attention\">"));
    // No token yet: the card says how to get one.
    assert!(page.contains("<p class=\"empty-state__title\">No access tokens yet</p>"));
    assert!(page.contains("href=\"/tokens/new\""));

    let draft = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Draft".into();
        mcp.transport = McpTransport::Npm;
        mcp.status = McpStatus::Draft;
    })
    .await;
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Broken".into();
        mcp.status = McpStatus::Error;
    })
    .await;
    let page = app.get("/").login_as(&admin).send().await.text();
    assert_eq!(
        rows_of(&page, "home-mcps-title"),
        [
            (
                "Broken".to_string(),
                "HTTP · connection failed".to_string(),
                "error".to_string()
            ),
            (
                "Draft".into(),
                "npm · not tested yet".into(),
                "draft".into()
            ),
            (
                "5 other MCPs".into(),
                "4 enabled · 1 disabled".into(),
                "ready".into()
            ),
        ]
    );
    assert!(page.contains(&format!(
        "<a class=\"list-row\" href=\"/mcps/{}/edit\">",
        draft.id
    )));
    // A draft is not an error: only the broken one asks for attention.
    assert_eq!(
        text_of(between(&page, "<div class=\"attention\">", "</section>")),
        "Broken is in error"
    );

    // One enabled MCP, and none.
    sqlx::query("update `mcps` set `enabled` = 0 where `name` != 'Ready 0'")
        .execute(&*app.core.db)
        .await
        .unwrap();
    let page = app.get("/").login_as(&admin).send().await.text();
    assert!(page.contains("<span>1 MCP behind one endpoint.</span>"));
    sqlx::query("update `mcps` set `enabled` = 0")
        .execute(&*app.core.db)
        .await
        .unwrap();
    let page = app.get("/").login_as(&admin).send().await.text();
    assert!(page.contains("<span>No MCP is enabled right now.</span>"));
}

#[tokio::test]
async fn cuts_the_activity_in_the_days_of_the_viewer() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |_| {}).await;
    let token = create_stored_access_token(&app, admin.id, |_| {}).await;
    create_mcp_call_log(&app, &token, Some(&mcp), |_| {}).await;
    let last_day = |markup: &str| {
        text_of(between(
            markup,
            "<div class=\"chart-axis hide-mobile\" aria-hidden=\"true\">",
            "</div>",
        ))
    };
    // Fourteen hours ahead of UTC: another day for most of the day.
    let kiritimati = Utc::now().with_timezone(&chrono_tz::Pacific::Kiritimati);

    // Drawn in UTC first, with the form the page script names the viewer's zone through.
    let page = app.get("/").login_as(&admin).send().await.text();
    assert!(page.contains(
        "<form method=\"get\" action=\"/\" data-async data-async-target=\"#home-activity\" data-timezone-sync hidden><input type=\"hidden\" name=\"timeZone\" value=\"UTC\" data-timezone></form>"
    ));
    assert!(last_day(&page).ends_with(&Utc::now().format("%-d %b").to_string()));

    // The script's answer: the inside of the card, in the days of that zone.
    let answer = app
        .get("/?timeZone=Pacific%2FKiritimati")
        .login_as(&admin)
        .header("x-requested-with", "fetch")
        .header("x-fragment", "home-activity")
        .send()
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.header("cache-control"), Some("no-store"));
    let card = answer.text();
    assert!(card.starts_with("<header class=\"card__header\">"));
    assert!(!card.contains("<html") && !card.contains("kpi__label"));
    assert!(last_day(&card).ends_with(&kiritimati.format("%-d %b").to_string()));
    assert!(card.contains(&format!(
        "title=\"{}: 1 call\"",
        kiritimati.format("%-d %b")
    )));

    // The session remembers the zone: the next visit is drawn in it at once.
    let session = answer.session();
    assert_eq!(session.get("timeZone"), Some(&json!("Pacific/Kiritimati")));
    let page = app.get("/").session(session).send().await.text();
    assert!(page.contains("name=\"timeZone\" value=\"Pacific/Kiritimati\" data-timezone"));
    assert!(last_day(&page).ends_with(&kiritimati.format("%-d %b").to_string()));

    // A zone nobody knows is not kept.
    let unknown = app
        .get("/?timeZone=Mars%2FOlympus")
        .login_as(&admin)
        .send()
        .await;
    assert!(unknown.session().get("timeZone").is_none());
    assert!(
        unknown
            .text()
            .contains("name=\"timeZone\" value=\"UTC\" data-timezone")
    );
}

#[tokio::test]
async fn says_when_the_gateway_has_no_public_address() {
    let app = TestApp::with_config(|config| config.app_url = None).await;
    let admin = create_admin(&app).await;
    create_mcp(&app, admin.id, |_| {}).await;

    let page = app.get("/").login_as(&admin).send().await.text();
    let card = between(&page, "<div class=\"hero-card\">", "</section>");
    assert!(card.contains("<span class=\"badge badge--warning\">APP_URL not set</span>"));
    assert!(card.contains(
        "<span class=\"copy-field__value\">Configure APP_URL to reveal the gateway URL.</span>"
    ));
    assert!(card.contains("disabled title=\"Set APP_URL to enable public links\""));
    assert!(!card.contains("gateway-url") && !card.contains("Online"));
    // The install dialog still explains how to connect with a token.
    assert!(card.contains("href=\"/tokens?install=1&amp;client=claude\""));
    // The shell says it too, once.
    assert_eq!(
        page.matches("Set APP_URL to enable public links</p>")
            .count(),
        1
    );
}

#[tokio::test]
async fn greets_by_the_first_name_when_there_is_one() {
    let app = TestApp::new().await;
    let mut admin = create_admin(&app).await;
    create_mcp(&app, admin.id, |_| {}).await;

    admin.full_name = Some("  Ada   Lovelace ".into());
    admin.save(&*app.core.db).await.unwrap();
    let page = app.get("/").login_as(&admin).send().await.text();
    assert!(page.contains("data-greeting=\"Welcome back\">Welcome back, Ada.<span>"));

    admin.full_name = None;
    admin.save(&*app.core.db).await.unwrap();
    let page = app.get("/").login_as(&admin).send().await.text();
    assert!(page.contains("data-greeting=\"Welcome back\">Welcome back.<span>"));

    // A name is text, whatever it holds.
    admin.full_name = Some("<b>Bobby</b> Tables".into());
    admin.save(&*app.core.db).await.unwrap();
    let page = app.get("/").login_as(&admin).send().await.text();
    assert!(page.contains("Welcome back, &lt;b&gt;Bobby&lt;/b&gt;.<span>"));
}
