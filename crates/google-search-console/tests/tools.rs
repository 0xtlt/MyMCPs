//! What the tools send to Google, what they make of its answers, and what
//! they refuse before contacting it.

mod support;

use chrono::NaiveDate;
use http::Method;
use mymcps_google_search_console::testing::{FakeSearchConsole, PREFIX_SITE, SITE, google_json};
use mymcps_google_search_console::tools::period_dates;
use serde_json::json;
use support::Google;

const ALLOWED: (&str, &str) = ("siteUrls", "https://blog.example.com/");

#[tokio::test]
async fn queries_a_report_and_names_each_dimension() {
    let google = Google::new(FakeSearchConsole::new()).await;
    let report = google
        .call(
            "query_search_analytics",
            json!({
                "site_url": "SC-DOMAIN:example.com",
                "start_date": "2026-10-01",
                "end_date": "2026-10-31",
                "dimensions": ["query", "page", "query"],
                "filters": [
                    { "dimension": "country", "expression": "fra" },
                    { "dimension": "page", "operator": "contains", "expression": "/shoes" },
                ],
                "data_state": "all",
                "row_limit": "2",
                "start_row": 4,
            }),
            &[],
        )
        .await
        .unwrap();

    let request = &google.fake.requests()[0];
    assert_eq!(request.method, Method::POST);
    assert_eq!(
        request.url.path(),
        "/webmasters/v3/sites/sc-domain%3Aexample.com/searchAnalytics/query"
    );
    assert_eq!(
        request.json.as_ref().unwrap(),
        &json!({
            "startDate": "2026-10-01",
            "endDate": "2026-10-31",
            "dimensions": ["query", "page"],
            "type": "web",
            "dimensionFilterGroups": [{ "groupType": "and", "filters": [
                { "dimension": "country", "operator": "equals", "expression": "fra" },
                { "dimension": "page", "operator": "contains", "expression": "/shoes" },
            ] }],
            "dataState": "all",
            "rowLimit": 2,
            "startRow": 4,
        })
    );
    assert_eq!(
        report,
        json!({
            "site_url": SITE,
            "start_date": "2026-10-01",
            "end_date": "2026-10-31",
            "search_type": "web",
            "aggregation": "byPage",
            "truncated": true,
            "next_start_row": 6,
            "rows": [
                { "query": "running shoes", "page": "https://www.example.com/shoes", "clicks": 120, "impressions": 4000, "ctr": 3, "position": 4.3 },
                { "query": "trail shoes", "page": "https://www.example.com/trail", "clicks": 7, "impressions": 900, "ctr": 0.78, "position": 11 },
            ],
        })
    );
}

#[tokio::test]
async fn reports_the_last_28_days_by_default() {
    let google = Google::new(FakeSearchConsole::new()).await;
    let report = google
        .call(
            "query_search_analytics",
            json!({ "site_url": PREFIX_SITE }),
            &[],
        )
        .await
        .unwrap();

    let request = &google.fake.requests()[0];
    assert_eq!(
        request.url.path(),
        "/webmasters/v3/sites/https%3A%2F%2Fblog.example.com%2F/searchAnalytics/query"
    );
    let body = request.json.as_ref().unwrap();
    assert_eq!(body["rowLimit"], 100);
    assert_eq!(body.get("dimensions"), None);
    let (start, end) = period_dates("LAST_28_DAYS", chrono::Utc::now().date_naive());
    assert_eq!(body["startDate"], start.to_string());
    assert_eq!(body["endDate"], end.to_string());
    assert_eq!(report.get("truncated"), None);
}

#[test]
fn named_periods_end_yesterday() {
    let day = |text: &str| NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap();
    let today = day("2026-03-01");
    for (period, start) in [
        ("LAST_7_DAYS", "2026-02-22"),
        ("LAST_28_DAYS", "2026-02-01"),
        ("LAST_3_MONTHS", "2025-11-29"),
        ("LAST_16_MONTHS", "2024-10-29"),
    ] {
        assert_eq!(
            period_dates(period, today),
            (day(start), day("2026-02-28")),
            "{period}"
        );
    }
}

#[tokio::test]
async fn refuses_wrong_arguments_before_contacting_google() {
    let google = Google::new(FakeSearchConsole::new()).await;
    for (tool, arguments, message) in [
        ("query_search_analytics", json!({}), "site_url is required"),
        (
            "query_search_analytics",
            json!({ "site_url": "example.com" }),
            "site_url must be a Search Console property such as sc-domain:example.com or https://www.example.com/, as list_sites returns them",
        ),
        (
            "query_search_analytics",
            json!({ "site_url": SITE, "start_date": "2026-02-30", "end_date": "2026-03-01" }),
            "start_date must be a date such as 2026-01-31",
        ),
        (
            "query_search_analytics",
            json!({ "site_url": SITE, "start_date": "2026-03-02", "end_date": "2026-03-01" }),
            "start_date must not be after end_date",
        ),
        (
            "query_search_analytics",
            json!({ "site_url": SITE, "start_date": "2026-03-01" }),
            "Set both start_date and end_date, or neither and use period",
        ),
        (
            "query_search_analytics",
            json!({ "site_url": SITE, "dimensions": ["keyword"] }),
            "dimensions must be one of: query, page, country, device, searchAppearance, date",
        ),
        (
            "query_search_analytics",
            json!({ "site_url": SITE, "filters": [{ "dimension": "date", "expression": "x" }] }),
            "filters must be one of: query, page, country, device, searchAppearance",
        ),
        (
            "query_search_analytics",
            json!({ "site_url": SITE, "row_limit": 25001 }),
            "row_limit must be an integer between 1 and 25000",
        ),
        (
            "get_sitemap",
            json!({ "site_url": SITE, "feedpath": "sitemap.xml" }),
            "feedpath must be a web address such as https://www.example.com/page",
        ),
        (
            "inspect_url",
            json!({ "site_url": SITE, "inspection_url": "https://www.example.com/", "language_code": "english" }),
            "language_code must be a language code such as en-US or fr",
        ),
    ] {
        assert_eq!(
            google.refusal(tool, arguments, &[]).await,
            message,
            "{tool}"
        );
    }
    assert!(google.fake.requests().is_empty());
}

#[tokio::test]
async fn keeps_to_the_properties_an_administrator_allowed() {
    let google = Google::new(FakeSearchConsole::new()).await;

    let sites = google
        .call("list_sites", json!({}), &[ALLOWED])
        .await
        .unwrap();
    assert_eq!(
        sites,
        json!({ "sites": [{ "site_url": PREFIX_SITE, "permission_level": "siteFullUser" }] })
    );
    let all = google.call("list_sites", json!({}), &[]).await.unwrap();
    assert_eq!(all["sites"].as_array().unwrap().len(), 2);

    let message = "This MCP may not use the Search Console property sc-domain:example.com. It is limited to: https://blog.example.com/. An administrator can change that from the MCPs page in MyMCPs.";
    for (tool, arguments) in [
        ("query_search_analytics", json!({ "site_url": SITE })),
        ("list_sitemaps", json!({ "site_url": SITE })),
        (
            "submit_sitemap",
            json!({ "site_url": SITE, "feedpath": "https://example.com/sitemap.xml" }),
        ),
        (
            "inspect_url",
            json!({ "site_url": SITE, "inspection_url": "https://example.com/" }),
        ),
    ] {
        assert_eq!(
            google.refusal(tool, arguments, &[ALLOWED]).await,
            message,
            "{tool}"
        );
    }
    assert_eq!(google.fake.requests().len(), 2, "only the two list_sites");

    // The allowed one, written another way, is the same property.
    google
        .call(
            "list_sitemaps",
            json!({ "site_url": "https://BLOG.example.com" }),
            &[ALLOWED],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn reads_submits_and_deletes_sitemaps() {
    let google = Google::new(FakeSearchConsole::new()).await;
    let sitemap = "https://www.example.com/sitemap.xml";
    let encoded = "/webmasters/v3/sites/sc-domain%3Aexample.com/sitemaps/https%3A%2F%2Fwww.example.com%2Fsitemap.xml";
    let arguments = json!({ "site_url": SITE, "feedpath": sitemap });

    let listed = google
        .call(
            "list_sitemaps",
            json!({ "site_url": SITE, "sitemap_index": "https://www.example.com/index.xml" }),
            &[],
        )
        .await
        .unwrap();
    let read = google
        .call("get_sitemap", arguments.clone(), &[])
        .await
        .unwrap();
    let submitted = google
        .call("submit_sitemap", arguments.clone(), &[])
        .await
        .unwrap();
    let deleted = google.call("delete_sitemap", arguments, &[]).await.unwrap();

    let expected = json!({
        "path": sitemap,
        "kind": "sitemap",
        "last_submitted": "2026-10-01T08:00:00.000Z",
        "last_downloaded": "2026-11-08T03:12:45.000Z",
        "is_pending": false,
        "is_sitemaps_index": false,
        "errors": 0,
        "warnings": 1,
        "contents": [{ "kind": "web", "submitted": 1234 }],
    });
    assert_eq!(
        listed,
        json!({ "site_url": SITE, "sitemaps": [expected.clone()] })
    );
    assert_eq!(read, expected);
    assert_eq!(submitted, json!({ "site_url": SITE, "submitted": sitemap }));
    assert_eq!(deleted, json!({ "site_url": SITE, "deleted": sitemap }));

    let requests = google.fake.requests();
    let sent: Vec<(Method, String)> = requests
        .iter()
        .map(|request| {
            (
                request.method.clone(),
                request.url[url::Position::BeforePath..].to_owned(),
            )
        })
        .collect();
    assert_eq!(
        sent,
        [
            (
                Method::GET,
                "/webmasters/v3/sites/sc-domain%3Aexample.com/sitemaps?sitemapIndex=https%3A%2F%2Fwww.example.com%2Findex.xml".to_owned()
            ),
            (Method::GET, encoded.to_owned()),
            (Method::PUT, encoded.to_owned()),
            (Method::DELETE, encoded.to_owned()),
        ]
    );
    assert!(requests[2].json.is_none());
}

#[tokio::test]
async fn summarizes_how_google_indexed_a_page() {
    let google = Google::new(FakeSearchConsole::new()).await;
    let inspected = google
        .call(
            "inspect_url",
            json!({ "site_url": SITE, "inspection_url": "https://www.example.com/shoes", "language_code": "fr" }),
            &[],
        )
        .await
        .unwrap();

    let request = &google.fake.requests()[0];
    assert_eq!(
        request.url.as_str(),
        "https://searchconsole.googleapis.com/v1/urlInspection/index:inspect"
    );
    assert_eq!(
        request.json.as_ref().unwrap(),
        &json!({ "inspectionUrl": "https://www.example.com/shoes", "siteUrl": SITE, "languageCode": "fr" })
    );
    assert_eq!(
        inspected,
        json!({
            "inspection_url": "https://www.example.com/shoes",
            "site_url": SITE,
            "index_status": {
                "verdict": "PASS",
                "coverage_state": "Submitted and indexed",
                "robots_txt_state": "ALLOWED",
                "indexing_state": "INDEXING_ALLOWED",
                "page_fetch_state": "SUCCESSFUL",
                "last_crawl_time": "2026-11-07T10:00:00Z",
                "crawled_as": "MOBILE",
                "google_canonical": "https://www.example.com/shoes",
                "user_canonical": "https://www.example.com/shoes",
                "sitemaps": ["https://www.example.com/sitemap.xml"],
                "referring_urls": ["https://www.example.com/"],
            },
            "mobile_usability": "VERDICT_UNSPECIFIED",
            "rich_results": { "verdict": "PASS", "types": ["Product snippets", "Breadcrumbs"] },
            "link": "https://search.google.com/search-console/inspect?resource_id=sc-domain:example.com&id=abc",
        })
    );
}

#[tokio::test]
async fn an_answer_google_does_not_document_is_a_tool_error() {
    let google = Google::new(FakeSearchConsole::responding(|_| {
        Some(google_json(&json!({ "rows": "nope" }), 200))
    }))
    .await;
    assert_eq!(
        google
            .refusal("query_search_analytics", json!({ "site_url": SITE }), &[])
            .await,
        "Google Search Console did not return the rows of a report"
    );
}
