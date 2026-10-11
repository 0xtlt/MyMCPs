//! Proof that this crate reads and writes what the AdonisJS app did: the
//! schema its migrations built, the values its encryption wrote, and the
//! password hashes of its scrypt driver. The fixtures were produced by the
//! Node app itself (`node ace migration:fresh`, `@boringnode/encryption`,
//! `@adonisjs/hash`).

use std::collections::BTreeMap;

use mymcps_core::config::TEST_APP_KEY;
use mymcps_core::crypto::{Encryption, hash_password, sha256_hex, verify_password};
use mymcps_core::db::{Db, migrations::MIGRATIONS};
use serde_json::Value;

const ADONIS_SCHEMA: &str = include_str!("fixtures/adonis_schema.sql");
const ADONIS_CRYPTO: &str = include_str!("fixtures/adonis_crypto.json");

/// Statements by object name. sqlite3's `.schema` adds `IF NOT EXISTS` for
/// nothing and ends each statement with a semicolon.
fn statements(schema: impl Iterator<Item = String>) -> BTreeMap<String, String> {
    schema
        .map(|statement| statement.trim().trim_end_matches(';').to_string())
        .filter(|statement| !statement.is_empty())
        .map(|statement| {
            let name = statement
                .split(['`', '"'])
                .nth(1)
                .unwrap_or_else(|| statement.split_whitespace().nth(2).unwrap_or(""))
                .trim_matches(['(', ' '])
                .to_string();
            let name = if name.is_empty() {
                statement.clone()
            } else {
                name
            };
            (name, statement)
        })
        .collect()
}

async fn fresh_database() -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("nested").join("db.sqlite3"))
        .await
        .unwrap();
    (db, dir)
}

#[tokio::test]
async fn a_fresh_database_has_the_schema_the_node_migrations_built() {
    let (db, _dir) = fresh_database().await;
    let ran = db.migrate().await.unwrap();
    assert_eq!(ran.len(), 25);
    assert_eq!(
        ran,
        MIGRATIONS
            .iter()
            .map(|migration| migration.name)
            .collect::<Vec<_>>()
    );

    let built: Vec<String> = sqlx::query_scalar(
        "select `sql` from `sqlite_master` where `sql` is not null order by `rowid`",
    )
    .fetch_all(&*db)
    .await
    .unwrap();
    let built = statements(built.into_iter());
    let expected = statements(ADONIS_SCHEMA.lines().map(str::to_string));

    assert_eq!(
        built.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "the same tables and indexes"
    );
    for (name, statement) in &expected {
        assert_eq!(&built[name], statement, "{name}");
    }

    // The ledger reads as if the Node app had migrated the database.
    let ledger: Vec<(String, i64)> =
        sqlx::query_as("select `name`, `batch` from `adonis_schema` order by `id`")
            .fetch_all(&*db)
            .await
            .unwrap();
    assert_eq!(ledger.len(), 25);
    assert!(ledger.iter().all(|(_, batch)| *batch == 1));
    assert_eq!(
        ledger[0].0,
        "database/migrations/1761885935168_create_users_table"
    );
    assert_eq!(
        ledger[24].0,
        "database/migrations/1791378864679_create_approval_requests_table"
    );
    let version: i64 = sqlx::query_scalar("select `version` from `adonis_schema_versions`")
        .fetch_one(&*db)
        .await
        .unwrap();
    assert_eq!(version, 2);

    // The settings row the Node migration seeded.
    let (level, days): (String, i64) = sqlx::query_as(
        "select `mcp_log_level`, `mcp_log_retention_days` from `instance_settings` where `id` = 1",
    )
    .fetch_one(&*db)
    .await
    .unwrap();
    assert_eq!((level.as_str(), days), ("metadata", 14));

    assert!(
        db.migrate().await.unwrap().is_empty(),
        "migrating again does nothing"
    );
}

#[tokio::test]
async fn a_database_migrated_by_the_node_app_is_left_alone() {
    let (db, _dir) = fresh_database().await;
    // What the Node app left behind: its schema, and its ledger.
    for statement in ADONIS_SCHEMA
        .lines()
        .filter(|line| !line.contains("sqlite_sequence"))
    {
        sqlx::query(sqlx::AssertSqlSafe(statement.to_string()))
            .execute(&*db)
            .await
            .unwrap();
    }
    sqlx::query("insert into `adonis_schema_versions` (`version`) values (2)")
        .execute(&*db)
        .await
        .unwrap();
    for migration in MIGRATIONS {
        sqlx::query("insert into `adonis_schema` (`name`, `batch`) values (?, 1)")
            .bind(migration.name)
            .execute(&*db)
            .await
            .unwrap();
    }
    sqlx::query(
        "insert into `users` (`email`, `password`, `created_at`, `role`) values ('admin@example.com', 'x', '2026-10-07 12:19:57', 'admin')",
    )
    .execute(&*db)
    .await
    .unwrap();

    assert!(db.migrate().await.unwrap().is_empty());
    let users: i64 = sqlx::query_scalar("select count(*) from `users`")
        .fetch_one(&*db)
        .await
        .unwrap();
    assert_eq!(users, 1);
}

#[tokio::test]
async fn an_older_node_database_is_brought_up_to_date_without_losing_rows() {
    let (db, _dir) = fresh_database().await;
    // Stop before the migration that rebuilds `mcps`, with rows that reference it.
    let rebuild = MIGRATIONS
        .iter()
        .position(|migration| migration.name.ends_with("add_auto_auth_to_mcps_table"))
        .unwrap();
    sqlx::query("create table `adonis_schema` (`id` integer not null primary key autoincrement, `name` varchar(255) not null, `batch` integer not null, `migration_time` datetime default CURRENT_TIMESTAMP)")
        .execute(&*db)
        .await
        .unwrap();
    for migration in &MIGRATIONS[..rebuild] {
        for statement in migration.statements {
            sqlx::query(sqlx::AssertSqlSafe(*statement))
                .execute(&*db)
                .await
                .unwrap();
        }
        sqlx::query("insert into `adonis_schema` (`name`, `batch`) values (?, 1)")
            .bind(migration.name)
            .execute(&*db)
            .await
            .unwrap();
    }
    for statement in [
        "insert into `users` (`id`, `email`, `password`, `created_at`, `role`) values (1, 'admin@example.com', 'x', '2026-10-07 12:19:57', 'admin')",
        "insert into `mcps` (`id`, `name`, `slug`, `transport`, `auth_type`, `created_by`, `created_at`) values (7, 'Notion', 'notion', 'http', 'oauth', 1, '2026-10-07 12:19:57')",
        "insert into `mcps` (`id`, `name`, `slug`, `transport`, `auth_type`, `created_by`, `created_at`) values (8, 'Open', 'open', 'http', 'none', 1, '2026-10-07 12:19:57')",
        "insert into `access_tokens` (`id`, `name`, `token_prefix`, `token_hash`, `scope_mode`, `created_by`, `created_at`) values (3, 'Agent', 'mcp_abc', 'hash', 'selected', 1, '2026-10-07 12:19:57')",
        "insert into `access_token_mcps` (`access_token_id`, `mcp_id`, `created_at`) values (3, 7, '2026-10-07 12:19:57')",
        "insert into `mcp_call_logs` (`access_token_id`, `access_token_name`, `access_token_prefix`, `mcp_id`, `requested_tool_name`, `outcome`, `duration_ms`, `created_at`) values (3, 'Agent', 'mcp_abc', 7, 'notion__search', 'success', 12, '2026-10-07 12:19:57')",
    ] {
        sqlx::query(statement).execute(&*db).await.unwrap();
    }

    let ran = db.migrate().await.unwrap();
    assert_eq!(ran.len(), MIGRATIONS.len() - rebuild);

    let mcps: Vec<(i64, String, bool)> =
        sqlx::query_as("select `id`, `auth_type`, `oauth_required` from `mcps` order by `id`")
            .fetch_all(&*db)
            .await
            .unwrap();
    assert_eq!(
        mcps,
        [
            (7, "auto".to_string(), true),
            (8, "auto".to_string(), false)
        ]
    );

    // Rebuilding `mcps` and `access_tokens` kept the rows that point at them.
    let links: i64 = sqlx::query_scalar("select count(*) from `access_token_mcps`")
        .fetch_one(&*db)
        .await
        .unwrap();
    assert_eq!(links, 1);
    let logged: (Option<i64>, Option<i64>) =
        sqlx::query_as("select `mcp_id`, `access_token_id` from `mcp_call_logs`")
            .fetch_one(&*db)
            .await
            .unwrap();
    assert_eq!(logged, (Some(7), Some(3)));
    let later_batch: i64 = sqlx::query_scalar("select max(`batch`) from `adonis_schema`")
        .fetch_one(&*db)
        .await
        .unwrap();
    assert_eq!(later_batch, 2);

    // And foreign keys are enforced again afterwards.
    sqlx::query("delete from `mcps` where `id` = 7")
        .execute(&*db)
        .await
        .unwrap();
    let links: i64 = sqlx::query_scalar("select count(*) from `access_token_mcps`")
        .fetch_one(&*db)
        .await
        .unwrap();
    assert_eq!(links, 0);
}

#[test]
fn decrypts_what_the_node_encryption_wrote() {
    let fixtures: Value = serde_json::from_str(ADONIS_CRYPTO).unwrap();
    assert_eq!(fixtures["key"], TEST_APP_KEY);
    let encryption = Encryption::new(TEST_APP_KEY);

    for case in fixtures["encrypted"].as_array().unwrap() {
        let ciphertext = case["ciphertext"].as_str().unwrap();
        let purpose = case["purpose"].as_str();
        let decrypted = encryption.decrypt_value(ciphertext, purpose);
        match &case["value"] {
            // The Node driver also refuses to return a falsy message.
            Value::String(text) if text.is_empty() => assert_eq!(decrypted, None),
            value => assert_eq!(decrypted.as_ref(), Some(value), "{ciphertext}"),
        }
        if let Value::String(text) = &case["value"]
            && !text.is_empty()
            && purpose.is_none()
        {
            assert_eq!(
                encryption.decrypt(ciphertext).as_deref(),
                Some(text.as_str())
            );
        }
        // A purpose is part of the seal.
        let other_purpose = if purpose.is_some() {
            None
        } else {
            Some("another")
        };
        assert_eq!(encryption.decrypt_value(ciphertext, other_purpose), None);
        assert_eq!(
            Encryption::new("some-other-key-of-16-chars").decrypt_value(ciphertext, purpose),
            None
        );
    }

    // Sealed with an expiry a century away, and with one already past.
    assert_eq!(
        encryption
            .decrypt(fixtures["expiring"]["ciphertext"].as_str().unwrap())
            .as_deref(),
        Some("soon")
    );
    assert_eq!(
        encryption.decrypt(fixtures["expired"]["ciphertext"].as_str().unwrap()),
        None
    );
}

#[test]
fn writes_values_in_the_node_format() {
    let encryption = Encryption::new(TEST_APP_KEY);
    let sealed = encryption.encrypt("sk-live-123");
    let parts: Vec<&str> = sealed.split('.').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[0], "gcm");
    assert!(parts[1].starts_with("v1:"));
    // Same length as the Node ciphertext of the same value: {"message":"sk-live-123"}.
    assert_eq!(
        parts[1].len(),
        "v1:dGPiev1qQukKOmCk3pbitxI5KY3X_SlDgg".len()
    );
    assert_eq!(parts[2].len(), 16);
    assert_eq!(parts[3].len(), 22);
    assert_ne!(
        sealed,
        encryption.encrypt("sk-live-123"),
        "a fresh IV each time"
    );
    assert_eq!(encryption.decrypt(&sealed).as_deref(), Some("sk-live-123"));

    for tampered in [
        sealed.replace("gcm.", "cbc."),
        format!("{sealed}A"),
        sealed.replacen("v1:", "v1:A", 1),
        "gcm.v1:.a.b".to_string(),
        String::new(),
    ] {
        assert_eq!(encryption.decrypt(&tampered), None, "{tampered}");
    }

    let later = chrono::Utc::now() + chrono::Duration::minutes(5);
    let sealed = encryption.encrypt_value(
        &serde_json::json!({"mcp": 7}),
        Some("builtin_file"),
        Some(later),
    );
    assert_eq!(
        encryption.decrypt_value(&sealed, Some("builtin_file")),
        Some(serde_json::json!({"mcp": 7}))
    );
    assert_eq!(
        encryption.decrypt_value(&sealed, Some("builtin_upload")),
        None
    );
    let earlier = chrono::Utc::now() - chrono::Duration::seconds(1);
    let sealed = encryption.encrypt_value(&serde_json::json!("x"), None, Some(earlier));
    assert_eq!(encryption.decrypt_value(&sealed, None), None);
}

#[test]
fn verifies_the_password_hashes_of_the_node_app() {
    let fixtures: Value = serde_json::from_str(ADONIS_CRYPTO).unwrap();
    for case in fixtures["hashes"].as_array().unwrap() {
        let (password, hash) = (
            case["password"].as_str().unwrap(),
            case["hash"].as_str().unwrap(),
        );
        assert!(verify_password(hash, password), "{password}");
        assert!(!verify_password(hash, &format!("{password}!")));
    }

    let hash = hash_password("correct horse");
    assert!(hash.starts_with("$scrypt$n=16384,r=8,p=1$"));
    assert_eq!(hash.split('$').count(), 5);
    assert_eq!(
        hash.len(),
        fixtures["hashes"][0]["hash"].as_str().unwrap().len()
    );
    assert!(verify_password(&hash, "correct horse"));
    assert!(!verify_password(&hash, "wrong horse"));

    for not_a_hash in [
        "",
        "plain",
        "$argon2id$v=19$m=65536,t=3,p=4$c2FsdA$aGFzaA",
        "$scrypt$n=1,r=8,p=1$c2FsdHNhbHQ$aGFzaA",
        "$scrypt$n=1048576,r=8,p=1$unhCAThKvwzqTKikmbtwTw$h0s/QkhJHBhgNK82G01Cq38YHVH/N3biBZCo1pwF+9f/6ql9cbNuuUXObMiovgH0kP4+3s6C65PTflx2E2fzsQ",
    ] {
        assert!(!verify_password(not_a_hash, "anything"), "{not_a_hash}");
    }
}

#[test]
fn hashes_tokens_like_the_node_app() {
    // createHash('sha256').update('mcp_example').digest('hex')
    assert_eq!(
        sha256_hex("mcp_example"),
        "a7fd7a2b18fd70d3f7f8cc393c89a1ea85de1929924cec88edeeea7c76e7874b"
    );
    assert_eq!(
        sha256_hex(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}
