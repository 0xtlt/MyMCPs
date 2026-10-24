//! Tailnets and the MCPs that use them travel in a backup, and a backup
//! made before they existed still restores.

use std::path::Path;
use std::sync::Arc;

use mymcps_core::backup::container::write_container;
use mymcps_core::backup::{BackupDir, Export, Header, Key, KeyDerivations, import};
use mymcps_core::config::generate_app_key;
use mymcps_core::crypto::hash_password;
use mymcps_core::db::migrations::MIGRATIONS;
use mymcps_core::models::{Mcp, McpTransport, Tailnet, Team, User};
use mymcps_core::{Config, Core};
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{ConnectOptions, Connection};

const BACKUP_PASSWORD: &str = "a password for the backup";

/// An instance with a key of its own, as every real one has.
async fn instance(dir: &Path) -> Arc<Core> {
    let mut config = Config::for_tests(dir);
    config.app_key = generate_app_key();
    Core::boot(config).await.unwrap()
}

async fn administrator(core: &Core) -> User {
    let mut user = User::with_password("ada@example.com", Some("Ada"), "secret-password")
        .await
        .unwrap();
    let mut transaction = core.db.begin().await.unwrap();
    user.create_first(&mut transaction).await.unwrap();
    transaction.commit().await.unwrap();
    user
}

/// What the Settings page downloads.
async fn export(core: &Arc<Core>) -> Vec<u8> {
    let mut export = Export::start(core, BACKUP_PASSWORD.to_string(), &KeyDerivations::new())
        .await
        .unwrap();
    let mut file = Vec::new();
    while let Some(block) = export.next_block().await.unwrap() {
        file.extend(block);
    }
    assert_eq!(file.len() as u64, export.content_length());
    file
}

/// What the setup screen does with an uploaded file.
async fn restore(core: &Arc<Core>, file: &[u8]) -> mymcps_core::backup::Imported {
    let dir = BackupDir::create(&core.config).unwrap();
    let path = dir.path().join("upload.mymcps");
    std::fs::write(&path, file).unwrap();
    import(
        core,
        &dir,
        &path,
        BACKUP_PASSWORD.to_string(),
        &KeyDerivations::new(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn tailnets_and_their_mcps_come_back_from_a_backup() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = instance(source_dir.path()).await;
    let ada = administrator(&source).await;

    let mut work = Tailnet {
        team_id: Team::ROOT_ID,
        name: "work".into(),
        control_url: Some("https://headscale.example.com".into()),
        hostname: "mymcps-work".into(),
        created_by: ada.id,
        ..Default::default()
    };
    work.insert(&*source.db).await.unwrap();
    let mut home = Tailnet {
        team_id: Team::ROOT_ID,
        name: "home".into(),
        hostname: "mymcps-home".into(),
        created_by: ada.id,
        ..Default::default()
    };
    home.insert(&*source.db).await.unwrap();
    let mut wiki = Mcp {
        name: "Wiki".into(),
        slug: "wiki".into(),
        transport: McpTransport::Http,
        http_url: Some("http://wiki.tailnet.ts.net/mcp".into()),
        created_by: ada.id,
        team_id: Team::ROOT_ID,
        tailnet_id: Some(work.id),
        ..Default::default()
    };
    wiki.insert(&*source.db).await.unwrap();
    // A tailnet deleted before the backup does not give its id away.
    let mut gone = Tailnet {
        team_id: Team::ROOT_ID,
        name: "gone".into(),
        hostname: "mymcps-gone".into(),
        created_by: ada.id,
        ..Default::default()
    };
    gone.insert(&*source.db).await.unwrap();
    gone.delete(&*source.db).await.unwrap();

    let file = export(&source).await;

    let target_dir = tempfile::tempdir().unwrap();
    let target = instance(target_dir.path()).await;
    let imported = restore(&target, &file).await;
    assert!(imported.migrated.is_empty());
    assert!(imported.reencrypted);

    let tailnets = Tailnet::of_team(&*target.db, Team::ROOT_ID).await.unwrap();
    assert_eq!(
        tailnets
            .iter()
            .map(|tailnet| (tailnet.id, tailnet.name.as_str(), tailnet.hostname.as_str()))
            .collect::<Vec<_>>(),
        [
            (home.id, "home", "mymcps-home"),
            (work.id, "work", "mymcps-work")
        ]
    );
    assert_eq!(
        tailnets[1].control_url.as_deref(),
        Some("https://headscale.example.com")
    );
    assert_eq!(tailnets[0].control_url, None);
    assert_eq!(tailnets[1].created_by, ada.id);
    assert_eq!(tailnets[1].created_at, work.created_at);

    let wiki = Mcp::find(&*target.db, wiki.id).await.unwrap().unwrap();
    assert_eq!(wiki.tailnet_id, Some(work.id));
    assert_eq!(
        work.mcps(&*target.db).await.unwrap().len(),
        1,
        "the MCP still uses its tailnet"
    );

    // Ids go on counting past the deleted tailnet.
    let mut next = Tailnet {
        team_id: Team::ROOT_ID,
        name: "next".into(),
        hostname: "mymcps-next".into(),
        created_by: ada.id,
        ..Default::default()
    };
    next.insert(&*target.db).await.unwrap();
    assert_eq!(next.id, gone.id + 1);
}

async fn connect(path: &Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .connect()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_backup_made_before_tailnets_restores_with_none() {
    // The database of the version before this one: every migration but the
    // last, run as the migrator runs them.
    let (last, earlier) = MIGRATIONS.split_last().unwrap();
    assert!(last.name.ends_with("_add_tailnets"));
    let old_key = generate_app_key();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("older.sqlite3");
    let mut database = connect(&path).await;
    for statement in [
        "create table `adonis_schema` (`id` integer not null primary key autoincrement, `name` varchar(255) not null, `batch` integer not null, `migration_time` datetime default CURRENT_TIMESTAMP)",
        "create table `adonis_schema_versions` (`version` integer, primary key (`version`))",
        "insert into `adonis_schema_versions` (`version`) values (2)",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut database)
            .await
            .unwrap();
    }
    for migration in earlier {
        for statement in migration.statements {
            sqlx::query(sqlx::AssertSqlSafe(*statement))
                .execute(&mut database)
                .await
                .unwrap();
        }
        sqlx::query("insert into `adonis_schema` (`name`, `batch`) values (?, 1)")
            .bind(migration.name)
            .execute(&mut database)
            .await
            .unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from pragma_table_info('mcps') where `name` = 'tailnet_id'"
        )
        .fetch_one(&mut database)
        .await
        .unwrap(),
        0
    );
    sqlx::query(
        "insert into `users` (`full_name`, `email`, `password`, `created_at`, `is_super_admin`) values ('Old Admin', 'old@example.com', ?, '2026-01-01 00:00:00', 1)",
    )
    .bind(hash_password("secret-password"))
    .execute(&mut database)
    .await
    .unwrap();
    sqlx::query(
        "insert into `mcps` (`name`, `slug`, `transport`, `http_url`, `auth_type`, `status`, `enabled`, `created_by`, `created_at`, `team_id`) values ('Notes', 'notes', 'http', 'https://notes.example/mcp', 'auto', 'ready', 1, 1, '2026-01-01 00:00:00', 1)",
    )
    .execute(&mut database)
    .await
    .unwrap();
    database.close().await.unwrap();
    let database = std::fs::read(&path).unwrap();

    let header = Header::from_parts(14, [7; 16], [9; 7]).unwrap();
    let key = Key::derive(BACKUP_PASSWORD, &header);
    let metadata = json!({ "createdAt": "2026-01-02T03:04:05.000Z", "appKey": old_key });
    let mut file = Vec::new();
    write_container(
        &key,
        &header,
        metadata.to_string().as_bytes(),
        database.as_slice(),
        &mut file,
    )
    .unwrap();

    let target_dir = tempfile::tempdir().unwrap();
    let target = instance(target_dir.path()).await;
    let imported = restore(&target, &file).await;
    assert_eq!(imported.migrated, [last.name]);

    // The migration ran on the backup, as a batch after its own.
    let batches: Vec<(String, i64)> = sqlx::query_as(
        "select `name`, `batch` from `adonis_schema` where `name` like '%add_tailnets' or `batch` > 1",
    )
    .fetch_all(&*target.db)
    .await
    .unwrap();
    assert_eq!(batches, [(last.name.to_string(), 2)]);

    assert!(
        Tailnet::of_team(&*target.db, Team::ROOT_ID)
            .await
            .unwrap()
            .is_empty()
    );
    let notes = Mcp::find(&*target.db, 1).await.unwrap().unwrap();
    assert_eq!(notes.slug, "notes");
    assert_eq!(notes.tailnet_id, None);
    assert!(
        User::find_by_email(&*target.db, "old@example.com")
            .await
            .unwrap()
            .is_some()
    );

    // The tables are those of this version: a tailnet can be added.
    let mut tailnet = Tailnet {
        team_id: Team::ROOT_ID,
        name: "work".into(),
        hostname: "mymcps".into(),
        created_by: 1,
        ..Default::default()
    };
    tailnet.insert(&*target.db).await.unwrap();
    assert_eq!(tailnet.id, 1);
}
