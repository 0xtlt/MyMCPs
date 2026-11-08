//! The result of a test of an MCP is saved only while the MCP is still what
//! the test started from.

use mymcps_core::models::{Mcp, McpHealth, McpStatus, McpTransport, Team, User};
use mymcps_core::{Config, Core, Db};

async fn core() -> (std::sync::Arc<Core>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::boot(Config::for_tests(dir.path())).await.unwrap();
    (core, dir)
}

async fn mcp(db: &Db) -> Mcp {
    let mut user =
        User::with_password("admin@example.com", Some("Ada Lovelace"), "secret-password")
            .await
            .unwrap();
    let mut transaction = db.begin().await.unwrap();
    user.create_first(&mut transaction).await.unwrap();
    transaction.commit().await.unwrap();
    let mut mcp = Mcp {
        name: "Wiki".into(),
        slug: "wiki".into(),
        transport: McpTransport::Http,
        http_url: Some("https://wiki.example/mcp".into()),
        created_by: user.id,
        team_id: Team::ROOT_ID,
        ..Default::default()
    };
    mcp.insert(&**db).await.unwrap();
    mcp
}

#[tokio::test]
async fn a_test_of_what_the_mcp_no_longer_is_is_dropped() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let mut tested = mcp(db).await;
    let basis = tested.probe_basis();

    // While the test runs, another request points the MCP elsewhere, and
    // leaves it untested.
    let mut edited = Mcp::find(&**db, tested.id).await.unwrap().unwrap();
    edited.http_url = Some("https://other.example/mcp".into());
    edited.mark_untested();
    edited.save(&**db).await.unwrap();

    tested.set_health(McpHealth::Failing("unreachable".into()));
    assert!(!tested.save_probe_result(&**db, &basis).await.unwrap());
    let saved = Mcp::find(&**db, tested.id).await.unwrap().unwrap();
    assert_eq!(saved.health(), McpHealth::Untested);
    assert_eq!(saved.http_url.as_deref(), Some("https://other.example/mcp"));
}

#[tokio::test]
async fn a_test_of_an_mcp_that_was_only_renamed_is_saved() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let mut tested = mcp(db).await;
    let basis = tested.probe_basis();

    let mut renamed = Mcp::find(&**db, tested.id).await.unwrap().unwrap();
    renamed.name = "Renamed".into();
    renamed.enabled = false;
    renamed.save(&**db).await.unwrap();

    tested.set_health(McpHealth::Ready);
    assert!(tested.save_probe_result(&**db, &basis).await.unwrap());
    let saved = Mcp::find(&**db, tested.id).await.unwrap().unwrap();
    assert_eq!(saved.status, McpStatus::Ready);
    assert_eq!(saved.name, "Renamed");
    assert!(!saved.enabled);
}

#[tokio::test]
async fn a_test_of_an_mcp_signed_out_meanwhile_is_dropped() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let mut tested = mcp(db).await;
    tested.oauth_access_token = Some("access-ciphertext".into());
    tested.save(&**db).await.unwrap();
    let basis = tested.probe_basis();

    let mut signed_out = Mcp::find(&**db, tested.id).await.unwrap().unwrap();
    signed_out.sign_out("moved");
    signed_out.save(&**db).await.unwrap();

    tested.set_health(McpHealth::Ready);
    assert!(!tested.save_probe_result(&**db, &basis).await.unwrap());
    let saved = Mcp::find(&**db, tested.id).await.unwrap().unwrap();
    assert_eq!(saved.health(), McpHealth::SignInRejected("moved".into()));
}
