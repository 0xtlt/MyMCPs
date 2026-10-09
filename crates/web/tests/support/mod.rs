//! What the tests of Logs, Analytics and Home share: the call log and
//! stored token factories of `tests/helpers/factories.ts`, and a few ways
//! to read a page.
#![allow(dead_code)]

use mymcps_core::Timestamp;
use mymcps_core::models::{AccessToken, CallOutcome, Mcp, McpCallLog, ScopeMode};
use mymcps_web::testing::TestApp;
use mymcps_web::testing::factories::next_value;

/// A token row as the database holds it, without a secret anyone knows.
/// Change it with `adjust`.
pub async fn create_stored_access_token(
    app: &TestApp,
    created_by: i64,
    adjust: impl FnOnce(&mut AccessToken),
) -> AccessToken {
    let mut token = AccessToken {
        name: next_value("stored-token"),
        // Unique, where the Node factory relied on one stored token a test.
        token_hash: next_value("stored-token-hash"),
        token_prefix: "mcp_stored".into(),
        scope_mode: ScopeMode::All,
        created_by,
        ..Default::default()
    };
    adjust(&mut token);
    token.insert(&*app.core.db).await.expect("a new token");
    token
}

/// A successful call of 25 ms made with this token, to `mcp` when there is
/// one. Change it with `adjust`: a `created_at` set there is kept.
pub async fn create_mcp_call_log(
    app: &TestApp,
    token: &AccessToken,
    mcp: Option<&Mcp>,
    adjust: impl FnOnce(&mut McpCallLog),
) -> McpCallLog {
    let mut log = McpCallLog {
        access_token_id: Some(token.id),
        access_token_name: token.name.clone(),
        access_token_prefix: token.token_prefix.clone(),
        mcp_id: mcp.map(|mcp| mcp.id),
        mcp_name: mcp.map(|mcp| mcp.name.clone()),
        mcp_slug: mcp.map(|mcp| mcp.slug.clone()),
        requested_tool_name: format!("{}__echo", mcp.map_or("test", |mcp| mcp.slug.as_str())),
        tool_name: Some("echo".into()),
        outcome: CallOutcome::Success,
        duration_ms: 25,
        ..Default::default()
    };
    adjust(&mut log);
    let created_at = (log.created_at != Timestamp::default()).then_some(log.created_at);
    log.insert(&*app.core.db).await.expect("a new call log");
    // Inserting a row stamps it with the current time.
    if let Some(created_at) = created_at {
        sqlx::query("update `mcp_call_logs` set `created_at` = ? where `id` = ?")
            .bind(created_at)
            .bind(log.id)
            .execute(&*app.core.db)
            .await
            .expect("a dated call log");
        log.created_at = created_at;
    }
    log
}

/// The text between the first `start` and the next `end` after it.
pub fn between<'a>(page: &'a str, start: &str, end: &str) -> &'a str {
    let from = page
        .find(start)
        .unwrap_or_else(|| panic!("{start:?} is not in the page"))
        + start.len();
    let length = page[from..]
        .find(end)
        .unwrap_or_else(|| panic!("{end:?} does not follow {start:?}"));
    &page[from..from + length]
}

/// The text of a piece of markup: its tags removed, its entities read, and
/// each run of white space (the inlined icons hold some) made one space.
pub fn text_of(markup: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for character in markup.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// The cells of a table row, as text.
fn cells(row: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut rest = row;
    loop {
        let (start, close) = match (rest.find("<td"), rest.find("<th")) {
            (Some(data), Some(header)) if header < data => (header, "</th>"),
            (Some(data), _) => (data, "</td>"),
            (None, Some(header)) => (header, "</th>"),
            (None, None) => return cells,
        };
        let cell = &rest[start..];
        let content = cell.find('>').map_or(0, |end| end + 1);
        let end = cell.find(close).unwrap_or(cell.len());
        cells.push(text_of(&cell[content..end]));
        rest = &cell[(end + close.len()).min(cell.len())..];
    }
}

/// The cells of every body row of the first table after `marker`, as text.
pub fn table_rows(page: &str, marker: &str) -> Vec<Vec<String>> {
    let table = between(page, marker, "</table>");
    let body = between(table, "<tbody>", "</tbody>");
    body.split("<tr").skip(1).map(cells).collect()
}
