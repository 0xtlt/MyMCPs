//! Backups: the export of the Settings page, the import of the setup
//! screen, and what an instance refuses to be set up from.

mod support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use http::StatusCode;
use mymcps_core::backup::container::{HEADER_BYTES, read_body, write_container};
use mymcps_core::backup::{
    BackupDir, BackupError, Header, Key, KeyDerivations, TEMPORARY_DIRECTORY,
};
use mymcps_core::config::{TEST_APP_KEY, generate_app_key};
use mymcps_core::crypto::{Encryption, hash_password};
use mymcps_core::db::migrations::MIGRATIONS;
use mymcps_core::models::{ApprovalRequest, InstanceSetting, Mcp, User};
use mymcps_core::secrets::{EnvironmentInput, decrypt_environment, merge_environment};
use mymcps_core::{Timestamp, VERSION};
use mymcps_web::AppState;
use mymcps_web::csrf::csrf_token;
use mymcps_web::routes::backup::BackupWork;
use mymcps_web::session::Session;
use mymcps_web::testing::factories::{
    PASSWORD, create_admin, create_admin_with, create_mcp, create_member,
};
use mymcps_web::testing::{TestApp, TestRequest, TestResponse, multipart_form};
use serde_json::{Map, Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{ConnectOptions, Connection};
use support::create_stored_access_token;

const BACKUP_PASSWORD: &str = "a password for the backup";
const IMPORT: &str = "/onboarding/import";

const NOT_A_BACKUP: &str = "This file is not a MyMCPs backup";
const NEWER_VERSION: &str = "This backup was made by a newer version of MyMCPs. Update this instance, then import it again.";
const WRONG_PASSWORD: &str = "The password is incorrect, or the backup file is damaged";
const DAMAGED: &str = "The backup file is damaged or incomplete";
const NO_ADMINISTRATOR: &str = "This backup holds no administrator account";
const UNDER_WAY: &str = "Another import is in progress. Try again in a moment.";
const TOO_LARGE: &str = "The backup file is larger than 4 GB";
const NO_FILE: &str = "Choose a backup file";
const NO_PASSWORD: &str = "Enter the password of the backup";
const IMPORTED: &str = "Backup imported. Sign in with an account of the imported instance.";

// ----------------------------------------------------------------- helpers

/// An instance with a key of its own, as every real one has.
async fn fresh_instance() -> TestApp {
    TestApp::with_config(|config| config.app_key = generate_app_key()).await
}

async fn export(app: &TestApp, user: &User, fields: &[(&str, &str)]) -> TestResponse {
    app.post("/settings/backup")
        .login_as(user)
        .csrf()
        .form(fields)
        .send()
        .await
}

async fn export_with(app: &TestApp, user: &User, password: &str) -> TestResponse {
    export(
        app,
        user,
        &[
            ("password", password),
            ("passwordConfirmation", password),
            ("currentPassword", PASSWORD),
        ],
    )
    .await
}

/// A request to import, from an address no other request of the test run
/// comes from: an address may only try ten times in a quarter of an hour.
fn import_request(app: &TestApp) -> TestRequest<'_> {
    static VISITORS: AtomicU32 = AtomicU32::new(0);
    let visitor = VISITORS.fetch_add(1, Ordering::Relaxed);
    let address = format!(
        "10.{}.{}.{}",
        (visitor >> 16) & 255,
        (visitor >> 8) & 255,
        visitor & 255
    );
    app.post(IMPORT).header("x-forwarded-for", &address)
}

/// Send the import form as the page does: the token, the file, the password.
async fn import(app: &TestApp, file: &[u8], password: &str) -> TestResponse {
    import_request(app)
        .csrf()
        .multipart(&[
            ("backup", Some("backup.mymcps"), file),
            ("password", None, password.as_bytes()),
        ])
        .send()
        .await
}

/// What the form says first once the visitor is sent back to it.
fn refusal(response: &TestResponse) -> Option<String> {
    match response.flashed("errors")? {
        Value::Object(errors) => errors.values().next()?.as_str().map(str::to_string),
        _ => None,
    }
}

fn refused_field(response: &TestResponse, field: &str) -> Option<String> {
    response
        .flashed("errors")?
        .get(field)?
        .as_str()
        .map(str::to_string)
}

fn assert_redirect(response: &TestResponse, path: &str) {
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.redirect_path().as_deref(), Some(path));
}

/// The files exports and imports have in the data directory right now.
fn temporary_files(app: &TestApp) -> Vec<PathBuf> {
    let root = app.core.config.data_dir.join(TEMPORARY_DIRECTORY);
    match std::fs::read_dir(root) {
        Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

async fn count(app: &TestApp, table: &'static str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select count(*) from `{table}`"
    )))
    .fetch_one(&*app.core.db)
    .await
    .unwrap()
}

/// A refused import changes nothing: the instance still has no row of
/// its own, and nothing of the file is left on its disk.
async fn assert_refused(app: &TestApp, response: &TestResponse, message: &str) {
    assert_redirect(response, IMPORT);
    assert_eq!(refusal(response).as_deref(), Some(message));
    assert_untouched(app).await;
}

async fn assert_untouched(app: &TestApp) {
    // But for the row of its settings, which a migration writes: the one
    // row a new instance has.
    assert_eq!(count(app, "instance_settings").await, 1);
    let settings = InstanceSetting::current(&*app.core.db).await.unwrap();
    assert_eq!(settings.mcp_log_retention_days, 14);
    assert_eq!(settings.updated_by, None);
    for table in [
        "users",
        "mcps",
        "access_tokens",
        "invites",
        "mcp_call_logs",
        "approval_requests",
        "oauth_clients",
        "remember_me_tokens",
    ] {
        assert_eq!(count(app, table).await, 0, "{table}");
    }
    assert_eq!(count(app, "adonis_schema").await, MIGRATIONS.len() as i64);
    assert_eq!(temporary_files(app), Vec::<PathBuf>::new());
}

/// The metadata of a backup made by an instance with this key.
fn metadata(app_key: &str) -> String {
    json!({ "createdAt": "2026-01-02T03:04:05.000Z", "appKey": app_key }).to_string()
}

/// Wrap a database as an export does, with the cheapest key a reader takes
/// and a salt that does not matter here.
fn seal_with(metadata: &str, database: &[u8], password: &str) -> Vec<u8> {
    let header = Header::from_parts(14, [7; 16], [9; 7]).unwrap();
    let key = Key::derive(password, &header);
    let mut file = Vec::new();
    write_container(&key, &header, metadata.as_bytes(), database, &mut file).unwrap();
    file
}

fn seal(database: &[u8]) -> Vec<u8> {
    seal_with(&metadata(TEST_APP_KEY), database, BACKUP_PASSWORD)
}

/// Open a backup with its password: the metadata and the database.
fn open(password: &str, file: &[u8]) -> Result<(Value, Vec<u8>), BackupError> {
    let header = Header::read(file)?;
    let key = Key::derive(password, &header);
    let mut database = Vec::new();
    let body = &file[HEADER_BYTES..];
    let metadata = read_body(&key, &header, body, body.len() as u64, &mut database)?;
    Ok((serde_json::from_slice(&metadata).unwrap(), database))
}

async fn connect(path: &std::path::Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        // To write what an instance never would: a row that points at nothing.
        .foreign_keys(false)
        .connect()
        .await
        .unwrap()
}

/// A copy of the database of `app`, as an export takes it, then changed by
/// `tamper`.
async fn snapshot(app: &TestApp, tamper: &[&'static str]) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("copy.sqlite3");
    sqlx::query("vacuum into ?")
        .bind(path.to_str().unwrap())
        .execute(&*app.core.db)
        .await
        .unwrap();
    let mut connection = connect(&path).await;
    for statement in tamper {
        sqlx::query(*statement)
            .execute(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    connection.close().await.unwrap();
    std::fs::read(&path).unwrap()
}

/// An instance with one administrator, to take copies of.
async fn instance_with_admin() -> (TestApp, User) {
    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "ada@example.com").await;
    (app, admin)
}

// ------------------------------------------------------------------ export

#[tokio::test]
async fn only_an_administrator_is_offered_and_given_a_backup() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;
    let fields = [
        ("password", BACKUP_PASSWORD),
        ("passwordConfirmation", BACKUP_PASSWORD),
        ("currentPassword", PASSWORD),
    ];

    let visitor = app
        .post("/settings/backup")
        .csrf()
        .form(&fields)
        .send()
        .await;
    assert_redirect(&visitor, "/login");

    let refused = export(&app, &member, &fields).await;
    assert_redirect(&refused, "/");
    assert_eq!(
        refused.flashed("error"),
        Some(json!("Admin access required"))
    );
    assert!(refused.header("content-disposition").is_none());

    // Without a CSRF token, not even an administrator.
    let forged = app
        .post("/settings/backup")
        .login_as(&admin)
        .form(&fields)
        .send()
        .await;
    assert_eq!(forged.status, StatusCode::FOUND);
    assert_eq!(
        forged.flashed("error"),
        Some(json!("Invalid or expired CSRF token"))
    );
    assert_eq!(temporary_files(&app), Vec::<PathBuf>::new());

    let page = app.get("/settings").login_as(&admin).send().await.text();
    assert!(page.contains(
        "<h2 class=\"section__title\" id=\"backup-title\">Backup <span class=\"badge badge--info badge--no-dot\">Admin only</span></h2>"
    ));
    assert!(page.contains("Export everything this instance stores to one encrypted file. Import it on the setup screen of a new instance."));
    assert!(page.contains("<span class=\"detail-row__label\">Export backup</span>"));
    assert!(
        page.contains("Users, MCPs with their credentials, access tokens, call logs and settings.")
    );
    assert!(page.contains(
        "<button type=\"button\" class=\"button button--secondary\" data-dialog-open=\"#export-backup\">Export backup</button>"
    ));
    // A plain post, which the browser answers by saving the file.
    assert!(page.contains(
        "<dialog class=\"dialog dialog--sm\" id=\"export-backup\" aria-labelledby=\"export-backup-title\" data-dialog-reset><div data-fragment><form method=\"post\" action=\"/settings/backup\" data-download>"
    ));
    assert!(
        page.contains("<h2 class=\"dialog__title\" id=\"export-backup-title\">Export backup</h2>")
    );
    assert!(page.contains(
        "The file is encrypted with the password you choose. It cannot be opened without it."
    ));
    for (label, name) in [
        ("Backup password", "password"),
        ("Confirm backup password", "passwordConfirmation"),
        ("Current password", "currentPassword"),
    ] {
        assert!(page.contains(&format!(">{label}</label>")), "{label}");
        assert!(
            page.contains(&format!("name=\"{name}\" type=\"password\"")),
            "{name}"
        );
    }
    assert!(page.contains(
        "<p class=\"field__help\" id=\"backup-current-password-help\">Your account password, to confirm it is you.</p>"
    ));
    assert!(page.contains("aria-describedby=\"backup-current-password-help\""));

    let page = app.get("/settings").login_as(&member).send().await.text();
    for admin_only in ["Backup", "backup", "/settings/backup"] {
        assert!(!page.contains(admin_only), "{admin_only}");
    }
}

#[tokio::test]
async fn refuses_an_export_without_the_account_password_and_counts_the_guesses() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let guess = async |current: &str| {
        export(
            &app,
            &admin,
            &[
                ("password", BACKUP_PASSWORD),
                ("passwordConfirmation", BACKUP_PASSWORD),
                ("currentPassword", current),
            ],
        )
        .await
    };

    let wrong = guess("not the password").await;
    assert_redirect(&wrong, "/settings");
    assert_eq!(
        refused_field(&wrong, "currentPassword").as_deref(),
        Some("The current password is incorrect")
    );
    assert!(wrong.header("content-disposition").is_none());

    // The page opens the dialog again on what was refused. No password
    // comes back, and the message is not said twice.
    let page = app
        .get("/settings")
        .session(wrong.session())
        .send()
        .await
        .text();
    assert!(page.contains(
        "id=\"export-backup\" aria-labelledby=\"export-backup-title\" data-dialog-reset data-open>"
    ));
    assert!(page.contains(
        "<p class=\"field__error\" id=\"backup-current-password-error\">The current password is incorrect</p>"
    ));
    assert!(page.contains(
        "aria-invalid=\"true\" aria-describedby=\"backup-current-password-error backup-current-password-help\""
    ));
    assert!(!page.contains("<p class=\"toast__message\">The current password is incorrect</p>"));
    assert!(!page.contains(BACKUP_PASSWORD) && !page.contains("not the password"));

    // The budget is the one of the email and password forms: five guesses
    // in all, whichever form they are typed in.
    for _ in 0..3 {
        assert_redirect(&guess("not the password").await, "/settings");
    }
    let email = app
        .patch("/settings/email")
        .login_as(&admin)
        .csrf()
        .form(&[
            ("email", "other@example.com"),
            ("currentPassword", "not the password"),
        ])
        .send()
        .await;
    assert_redirect(&email, "/settings");

    let limited = guess(PASSWORD).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header("retry-after").is_some());
    assert!(limited.text().contains("Too many requests"));
    assert_eq!(temporary_files(&app), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn refuses_a_backup_password_that_is_short_or_typed_two_ways() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let short = export_with(&app, &admin, "7 chars").await;
    assert_redirect(&short, "/settings");
    assert_eq!(
        refused_field(&short, "password").as_deref(),
        Some("The password field must have at least 8 characters")
    );

    let long = "p".repeat(129);
    let too_long = export_with(&app, &admin, &long).await;
    assert_redirect(&too_long, "/settings");
    assert_eq!(
        refused_field(&too_long, "password").as_deref(),
        Some("The password field must not be greater than 128 characters")
    );

    let mismatched = export(
        &app,
        &admin,
        &[
            ("password", BACKUP_PASSWORD),
            ("passwordConfirmation", "another password"),
            ("currentPassword", PASSWORD),
        ],
    )
    .await;
    assert_redirect(&mismatched, "/settings");
    assert_eq!(
        refused_field(&mismatched, "passwordConfirmation").as_deref(),
        Some("The password field and passwordConfirmation field must be the same")
    );
    let page = app
        .get("/settings")
        .session(mismatched.session())
        .send()
        .await
        .text();
    assert!(page.contains(
        "id=\"export-backup\" aria-labelledby=\"export-backup-title\" data-dialog-reset data-open>"
    ));
    assert!(page.contains("id=\"backup-password-confirmation-error\">The password field and passwordConfirmation field must be the same</p>"));

    let unconfirmed = export(
        &app,
        &admin,
        &[
            ("password", BACKUP_PASSWORD),
            ("passwordConfirmation", BACKUP_PASSWORD),
        ],
    )
    .await;
    assert_redirect(&unconfirmed, "/settings");
    assert_eq!(
        refused_field(&unconfirmed, "currentPassword").as_deref(),
        Some("The currentPassword field must be defined")
    );

    // None of these was a guess at the account password.
    for response in [short, too_long, mismatched, unconfirmed] {
        assert!(response.header("content-disposition").is_none());
    }
    assert_eq!(
        export_with(&app, &admin, BACKUP_PASSWORD).await.status,
        StatusCode::OK
    );
    assert_eq!(temporary_files(&app), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn exports_the_instance_as_one_encrypted_file() {
    let app = fresh_instance().await;
    let admin = create_admin_with(&app, "ada@example.com").await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Notes".into();
        mcp.slug = "notes".into();
    })
    .await;
    let before = chrono::Utc::now();

    let response = export_with(&app, &admin, BACKUP_PASSWORD).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.header("content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert_eq!(
        response.header("content-length"),
        Some(response.body.len().to_string().as_str())
    );
    // mymcps-backup-YYYYMMDD-HHMMSS.mymcps, in UTC.
    let disposition = response.header("content-disposition").unwrap();
    let name = disposition
        .strip_prefix("attachment; filename=\"mymcps-backup-")
        .and_then(|rest| rest.strip_suffix(".mymcps\""))
        .unwrap_or_else(|| panic!("{disposition}"));
    let made_at = chrono::NaiveDateTime::parse_from_str(name, "%Y%m%d-%H%M%S")
        .unwrap()
        .and_utc();
    assert!((made_at - before).num_seconds().abs() <= 5, "{name}");

    // The header of a writer, then nothing that can be read.
    let file = &response.body[..];
    assert_eq!(file[..13], *b"MYMCPSBK\x01\x01\x11\x08\x01");
    assert!(!file.windows(15).any(|window| window == b"SQLite format 3"));
    assert!(!file.windows(15).any(|window| window == b"ada@example.com"));

    let (metadata, database) = open(BACKUP_PASSWORD, file).unwrap();
    assert_eq!(metadata["appKey"], app.core.config.app_key.as_str());
    assert_eq!(
        metadata["app"],
        json!({ "runtime": "rust", "version": VERSION })
    );
    let created_at = metadata["createdAt"].as_str().unwrap();
    assert_eq!(created_at.len(), "2026-10-08T15:30:00.000Z".len());
    assert!(created_at.ends_with('Z'));
    let created_at = chrono::DateTime::parse_from_rfc3339(created_at).unwrap();
    assert_eq!(created_at.timestamp(), made_at.timestamp());
    assert_eq!(
        metadata.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["createdAt", "appKey", "app"]
    );

    // The database is one SQLite file with the rows of the instance.
    assert!(database.starts_with(b"SQLite format 3\0"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exported.sqlite3");
    std::fs::write(&path, &database).unwrap();
    let mut exported = connect(&path).await;
    let emails: Vec<String> = sqlx::query_scalar("select `email` from `users`")
        .fetch_all(&mut exported)
        .await
        .unwrap();
    assert_eq!(emails, ["ada@example.com"]);
    let slugs: Vec<String> = sqlx::query_scalar("select `slug` from `mcps`")
        .fetch_all(&mut exported)
        .await
        .unwrap();
    assert_eq!(slugs, [mcp.slug.as_str()]);
    let ledger: i64 = sqlx::query_scalar("select count(*) from `adonis_schema`")
        .fetch_one(&mut exported)
        .await
        .unwrap();
    assert_eq!(ledger, MIGRATIONS.len() as i64);
    exported.close().await.unwrap();

    assert!(matches!(
        open("another password", file),
        Err(BackupError::WrongPassword)
    ));
    // The copy of the database is gone once the file was handed out.
    assert_eq!(temporary_files(&app), Vec::<PathBuf>::new());

    // Two exports never share a salt or a nonce prefix.
    let again = export_with(&app, &admin, BACKUP_PASSWORD).await;
    assert_ne!(again.body[13..36], file[13..36]);
}

// ------------------------------------------------------------------ import

#[tokio::test]
async fn the_setup_screen_offers_to_import_a_backup() {
    let app = TestApp::new().await;

    let setup = app.get("/onboarding").send().await.text();
    assert!(setup.contains(
        "<button type=\"submit\" class=\"button button--primary button--block\">Create admin</button><a class=\"button button--secondary button--block\" href=\"/onboarding/import\">Import a backup</a>"
    ));

    let response = app.get(IMPORT).send().await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.header("cache-control"), Some("no-store"));
    let page = response.text();
    assert!(page.contains("<title>Import a backup · MyMCPs</title>"));
    assert!(
        page.contains("<h1 class=\"auth-card__title\" id=\"import-title\">Import a backup</h1>")
    );
    assert!(page.contains(
        "Restore the users, MCPs, access tokens, call logs and settings of another MyMCPs instance."
    ));
    // A plain form that carries a file, with the token before the file.
    assert!(page.contains(
        "<form class=\"form\" method=\"post\" action=\"/onboarding/import\" enctype=\"multipart/form-data\"><input type=\"hidden\" name=\"_csrf\""
    ));
    assert!(page.contains("<label class=\"field__label\" for=\"backup\">Backup file</label><input class=\"input\" id=\"backup\" name=\"backup\" type=\"file\" accept=\".mymcps\" required>"));
    assert!(
        page.contains("<label class=\"field__label\" for=\"password\">Backup password</label>")
    );
    assert!(page.contains("id=\"password\" name=\"password\" type=\"password\""));
    assert!(page.contains(
        "<button type=\"submit\" class=\"button button--primary button--block\" data-busy-label=\"Importing…\">Import backup</button>"
    ));
    assert!(page.contains(
        "<a class=\"button button--block\" href=\"/onboarding\">Create a new instance instead</a>"
    ));
    assert!(!page.contains("aria-invalid"));
    assert_eq!(temporary_files(&app), Vec::<PathBuf>::new());
}

/// Everything an instance encrypts with its key, each with a value of its own.
struct Secrets {
    mcp_id: i64,
    approval_id: i64,
}

const SINGLE_SECRETS: [(&str, &str); 6] = [
    ("auth_bearer", "the bearer of the test"),
    ("auth_header_value", "the header of the test"),
    ("oauth_client_secret", "the client side of the test"),
    ("oauth_access_token", "the access of the test"),
    ("oauth_refresh_token", "the refresh of the test"),
    ("builtin_password", "the app word of the test"),
];
const NPM_ENV: [(&str, &str); 2] = [
    ("ZONE", "last letter first"),
    ("API_BASE", "then the first"),
];
const BUILTIN_SETTINGS: [(&str, &str); 2] = [
    ("login_customer_id", "123 456 7890"),
    ("account", "the account of the test"),
];
const ARGUMENTS: &str = r#"{"campaign":"Autumn","budget":25}"#;
const SUMMARY: &str = "Raise the budget of Autumn to 25 a day";

fn environment(pairs: &[(&str, &str)]) -> Vec<EnvironmentInput> {
    pairs
        .iter()
        .map(|(name, value)| EnvironmentInput {
            name: name.to_string(),
            value: Some(value.to_string()),
        })
        .collect()
}

async fn populate_secrets(app: &TestApp, admin: &User) -> Secrets {
    let core = &app.core;
    let mcp = create_mcp(app, admin.id, |mcp| {
        let encrypt = |value: &str| core.encrypt_secret(Some(value));
        mcp.auth_bearer = encrypt(SINGLE_SECRETS[0].1);
        mcp.auth_header_value = encrypt(SINGLE_SECRETS[1].1);
        mcp.oauth_client_secret = encrypt(SINGLE_SECRETS[2].1);
        mcp.oauth_access_token = encrypt(SINGLE_SECRETS[3].1);
        mcp.oauth_refresh_token = encrypt(SINGLE_SECRETS[4].1);
        mcp.builtin_password = encrypt(SINGLE_SECRETS[5].1);
        mcp.npm_env = merge_environment(&core.encryption, None, &environment(&NPM_ENV));
        mcp.builtin_settings =
            merge_environment(&core.encryption, None, &environment(&BUILTIN_SETTINGS));
    })
    .await;
    let token = create_stored_access_token(app, admin.id, |_| {}).await;
    let mut approval = ApprovalRequest {
        public_id: "apr_backup_test".into(),
        mcp_id: mcp.id,
        access_token_id: token.id,
        tool_name: "update_campaign_budget".into(),
        arguments: core.encrypt_secret(Some(ARGUMENTS)).unwrap(),
        arguments_hash: "0".repeat(64),
        summary: core.encrypt_secret(Some(SUMMARY)).unwrap(),
        expires_at: Timestamp::now() + chrono::Duration::hours(24),
        ..Default::default()
    };
    approval.insert(&*core.db).await.unwrap();
    Secrets {
        mcp_id: mcp.id,
        approval_id: approval.id,
    }
}

/// Every secret reads back with the key of `app`, and with no other.
async fn assert_secrets(app: &TestApp, secrets: &Secrets, stranger: &Encryption) {
    let core = &app.core;
    let mcp = Mcp::find(&*core.db, secrets.mcp_id).await.unwrap().unwrap();
    let columns = [
        &mcp.auth_bearer,
        &mcp.auth_header_value,
        &mcp.oauth_client_secret,
        &mcp.oauth_access_token,
        &mcp.oauth_refresh_token,
        &mcp.builtin_password,
    ];
    for ((name, secret), stored) in SINGLE_SECRETS.iter().zip(columns) {
        assert_eq!(
            core.decrypt_secret(stored.as_deref()).as_deref(),
            Some(*secret),
            "{name}"
        );
        assert_eq!(stranger.decrypt(stored.as_deref().unwrap()), None, "{name}");
    }
    // Names and order are kept.
    let pairs = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    };
    assert_eq!(
        decrypt_environment(&core.encryption, mcp.npm_env.as_deref()).unwrap(),
        pairs(&NPM_ENV)
    );
    assert_eq!(
        decrypt_environment(&core.encryption, mcp.builtin_settings.as_deref()).unwrap(),
        pairs(&BUILTIN_SETTINGS)
    );
    assert!(decrypt_environment(stranger, mcp.npm_env.as_deref()).is_err());
    assert!(decrypt_environment(stranger, mcp.builtin_settings.as_deref()).is_err());

    let approval = ApprovalRequest::find(&*core.db, secrets.approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        core.decrypt_secret(Some(&approval.arguments)).as_deref(),
        Some(ARGUMENTS)
    );
    assert_eq!(
        core.decrypt_secret(Some(&approval.summary)).as_deref(),
        Some(SUMMARY)
    );
    assert_eq!(stranger.decrypt(&approval.arguments), None);
    assert_eq!(stranger.decrypt(&approval.summary), None);
}

#[tokio::test]
async fn an_instance_is_set_up_from_the_backup_of_another_one() {
    // The instance that is backed up.
    let source = fresh_instance().await;
    let admin = create_admin_with(&source, "ada@example.com").await;
    let member = create_member(&source).await;
    let secrets = populate_secrets(&source, &admin).await;
    assert_secrets(&source, &secrets, &Encryption::new(&generate_app_key())).await;
    // A value that no key opens, and an MCP that was deleted: its id is
    // never given again.
    let unreadable = create_mcp(&source, admin.id, |mcp| {
        mcp.auth_bearer = Some("not a ciphertext".into());
        mcp.npm_env = Some("{\"A\": \"not one either\"}".into());
    })
    .await;
    let deleted = create_mcp(&source, admin.id, |_| {}).await;
    sqlx::query("delete from `mcps` where `id` = ?")
        .bind(deleted.id)
        .execute(&*source.core.db)
        .await
        .unwrap();
    let mut settings = InstanceSetting::current(&*source.core.db).await.unwrap();
    settings.mcp_log_retention_days = 90;
    settings.updated_by = Some(admin.id);
    settings.save(&*source.core.db).await.unwrap();
    // Counters of the instance, which are not part of what it is.
    sqlx::query("insert into `rate_limits` (`key`, `points`, `expire`) values ('of the source', 3, 99999999999999)")
        .execute(&*source.core.db)
        .await
        .unwrap();

    // Typed with spaces around it, as any form value may be.
    let exported = export_with(&source, &admin, "  a password for the backup ").await;
    assert_eq!(exported.status, StatusCode::OK);

    // A new instance, with another key and counters of its own.
    let target = fresh_instance().await;
    assert_ne!(target.core.config.app_key, source.core.config.app_key);
    sqlx::query("insert into `rate_limits` (`key`, `points`, `expire`) values ('of the target', 1, 99999999999999)")
        .execute(&*target.core.db)
        .await
        .unwrap();

    let imported = import(&target, &exported.body, " a password for the backup\n").await;

    assert_redirect(&imported, "/login");
    assert_eq!(imported.flashed("success"), Some(json!(IMPORTED)));
    assert_eq!(imported.flashed("errors"), None);
    // Ids keep counting from where they were on the source, in every table.
    const SEQUENCES: &str = "select `name`, `seq` from `sqlite_sequence` order by `name`";
    let sequences: Vec<(String, i64)> = sqlx::query_as(SEQUENCES)
        .fetch_all(&*target.core.db)
        .await
        .unwrap();
    let on_source: Vec<(String, i64)> = sqlx::query_as(SEQUENCES)
        .fetch_all(&*source.core.db)
        .await
        .unwrap();
    assert_eq!(sequences, on_source);
    assert!(sequences.contains(&("mcps".to_string(), deleted.id)));
    assert!(sequences.contains(&("users".to_string(), member.id)));
    // Nobody is signed in by an import.
    assert!(!imported.session().contains_key("auth_web"));
    assert_eq!(temporary_files(&target), Vec::<PathBuf>::new());
    let login = target
        .get("/login")
        .session(imported.session())
        .send()
        .await;
    assert_eq!(login.status, StatusCode::OK);
    assert!(login.text().contains(IMPORTED));

    // The accounts of the source sign in with the passwords they had.
    for email in ["ada@example.com", member.email.as_str()] {
        let signed_in = target
            .post("/login")
            .csrf()
            .form(&[("email", email), ("password", PASSWORD)])
            .send()
            .await;
        assert_redirect(&signed_in, "/");
        let home = target.get("/").session(signed_in.session()).send().await;
        assert_eq!(home.status, StatusCode::OK, "{email}");
    }
    let users: Vec<(i64, String, String)> =
        sqlx::query_as("select `id`, `email`, `role` from `users` order by `id`")
            .fetch_all(&*target.core.db)
            .await
            .unwrap();
    assert_eq!(
        users,
        [
            (admin.id, "ada@example.com".to_string(), "admin".to_string()),
            (member.id, member.email.clone(), "member".to_string()),
        ]
    );

    // Every secret was encrypted anew with the key of this instance.
    assert_secrets(&target, &secrets, &source.core.encryption).await;
    let untouched = Mcp::find(&*target.core.db, unreadable.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(untouched.auth_bearer.as_deref(), Some("not a ciphertext"));
    assert_eq!(
        untouched.npm_env.as_deref(),
        Some("{\"A\": \"not one either\"}")
    );
    let settings = InstanceSetting::current(&*target.core.db).await.unwrap();
    assert_eq!(settings.mcp_log_retention_days, 90);
    assert_eq!(settings.updated_by, Some(admin.id));

    // The ledger is the whole of this version's migrations.
    let ledger: Vec<String> =
        sqlx::query_scalar("select `name` from `adonis_schema` order by `id`")
            .fetch_all(&*target.core.db)
            .await
            .unwrap();
    assert_eq!(
        ledger,
        MIGRATIONS
            .iter()
            .map(|migration| migration.name)
            .collect::<Vec<_>>()
    );

    // An MCP that was deleted on the source does not give its id away here.
    let next = create_mcp(&target, admin.id, |_| {}).await;
    assert_eq!(next.id, deleted.id + 1);

    // Rate limit counters are never imported, and those of the instance stay.
    let counters: Vec<String> = sqlx::query_scalar("select `key` from `rate_limits`")
        .fetch_all(&*target.core.db)
        .await
        .unwrap();
    assert_eq!(counters, ["of the target"]);

    // The instance is set up: its setup screens are gone.
    assert_redirect(&target.get(IMPORT).send().await, "/login");
    assert_redirect(&target.get("/onboarding").send().await, "/login");
}

/// A database as an instance of an older version left it: the first
/// `migrations` migrations of this one, run as the migrator runs them.
/// `rewrite` changes each statement on its way, for a database no version
/// ever made.
async fn older_database(
    dir: &tempfile::TempDir,
    migrations: usize,
    rewrite: fn(&str) -> String,
) -> SqliteConnection {
    let mut connection = connect(&dir.path().join("older.sqlite3")).await;
    for statement in [
        "create table `adonis_schema` (`id` integer not null primary key autoincrement, `name` varchar(255) not null, `batch` integer not null, `migration_time` datetime default CURRENT_TIMESTAMP)",
        "create table `adonis_schema_versions` (`version` integer, primary key (`version`))",
        "insert into `adonis_schema_versions` (`version`) values (2)",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(rewrite(statement)))
            .execute(&mut connection)
            .await
            .unwrap();
    }
    for migration in &MIGRATIONS[..migrations] {
        for statement in migration.statements {
            sqlx::query(sqlx::AssertSqlSafe(rewrite(statement)))
                .execute(&mut connection)
                .await
                .unwrap();
        }
        sqlx::query("insert into `adonis_schema` (`name`, `batch`) values (?, 1)")
            .bind(migration.name)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    connection
}

#[tokio::test]
async fn a_backup_of_an_older_version_is_migrated_before_its_rows_are_copied() {
    // Three migrations short: no session version on users, no tool
    // approvals or settings on MCPs, no approval requests.
    let older = MIGRATIONS.len() - 3;
    assert_eq!(
        MIGRATIONS[older].name,
        "database/migrations/1791132214318_add_session_version_to_users_table"
    );
    let old_key = generate_app_key();
    let old_encryption = Encryption::new(&old_key);
    let dir = tempfile::tempdir().unwrap();
    let mut database = older_database(&dir, older, str::to_string).await;
    sqlx::query(
        "insert into `users` (`full_name`, `email`, `password`, `created_at`, `role`) values ('Old Admin', 'old@example.com', ?, '2026-01-01 00:00:00', 'admin')",
    )
    .bind(hash_password(PASSWORD))
    .execute(&mut database)
    .await
    .unwrap();
    sqlx::query(
        "insert into `mcps` (`name`, `slug`, `transport`, `http_url`, `auth_type`, `auth_bearer`, `status`, `enabled`, `created_by`, `created_at`) values ('Notes', 'notes', 'http', 'https://notes.example/mcp', 'bearer', ?, 'ready', 1, 1, '2026-01-01 00:00:00')",
    )
    .bind(old_encryption.encrypt("the bearer of the old instance"))
    .execute(&mut database)
    .await
    .unwrap();
    database.close().await.unwrap();
    let database = std::fs::read(dir.path().join("older.sqlite3")).unwrap();

    let target = fresh_instance().await;
    let imported = import(
        &target,
        &seal_with(&metadata(&old_key), &database, BACKUP_PASSWORD),
        BACKUP_PASSWORD,
    )
    .await;

    assert_redirect(&imported, "/login");
    assert_eq!(imported.flashed("success"), Some(json!(IMPORTED)));
    assert_eq!(temporary_files(&target), Vec::<PathBuf>::new());

    // The ledger says what ran where: the migrations of the old instance,
    // then the three it lacked, as a second batch.
    let ledger: Vec<(String, i64)> =
        sqlx::query_as("select `name`, `batch` from `adonis_schema` order by `id`")
            .fetch_all(&*target.core.db)
            .await
            .unwrap();
    let expected: Vec<(String, i64)> = MIGRATIONS
        .iter()
        .enumerate()
        .map(|(index, migration)| {
            (
                migration.name.to_string(),
                if index < older { 1 } else { 2 },
            )
        })
        .collect();
    assert_eq!(ledger, expected);

    // The rows fit the schema of this version, and the account signs in.
    let user = User::find_by_email(&*target.core.db, "old@example.com")
        .await
        .unwrap()
        .unwrap();
    assert!(user.is_admin());
    assert_eq!(user.session_version, 1);
    let signed_in = target
        .post("/login")
        .csrf()
        .form(&[("email", "old@example.com"), ("password", PASSWORD)])
        .send()
        .await;
    assert_redirect(&signed_in, "/");
    assert_eq!(
        target
            .get("/mcps")
            .session(signed_in.session())
            .send()
            .await
            .status,
        StatusCode::OK
    );
    let mcp = Mcp::find(&*target.core.db, 1).await.unwrap().unwrap();
    assert_eq!(mcp.slug, "notes");
    assert_eq!(mcp.tool_approvals, None);
    assert_eq!(mcp.builtin_settings, None);
    assert_eq!(
        target
            .core
            .decrypt_secret(mcp.auth_bearer.as_deref())
            .as_deref(),
        Some("the bearer of the old instance")
    );
    assert_eq!(count(&target, "approval_requests").await, 0);
}

#[tokio::test]
async fn refuses_a_database_that_does_not_count_its_ids() {
    // The tables and the columns of an instance, and none of them counts
    // the ids it gave: no version made this database.
    let dir = tempfile::tempdir().unwrap();
    let mut database = older_database(&dir, MIGRATIONS.len(), |statement| {
        statement
            .replace(" autoincrement", "")
            .replace(" AUTOINCREMENT", "")
    })
    .await;
    sqlx::query(
        "insert into `users` (`full_name`, `email`, `password`, `created_at`, `role`) values ('Admin', 'admin@example.com', ?, '2026-01-01 00:00:00', 'admin')",
    )
    .bind(hash_password(PASSWORD))
    .execute(&mut database)
    .await
    .unwrap();
    let counters: i64 =
        sqlx::query_scalar("select count(*) from `sqlite_master` where `name` = 'sqlite_sequence'")
            .fetch_one(&mut database)
            .await
            .unwrap();
    assert_eq!(counters, 0);
    database.close().await.unwrap();
    let database = std::fs::read(dir.path().join("older.sqlite3")).unwrap();

    let app = TestApp::new().await;
    let response = import(&app, &seal(&database), BACKUP_PASSWORD).await;
    assert_refused(&app, &response, DAMAGED).await;
}

#[tokio::test]
async fn the_import_is_gone_once_the_instance_is_set_up() {
    let (source, _) = instance_with_admin().await;
    let file = seal(&snapshot(&source, &[]).await);

    let app = TestApp::new().await;
    let admin = create_admin_with(&app, "existing@example.com").await;

    assert_redirect(&app.get(IMPORT).send().await, "/login");
    assert_redirect(&app.get(IMPORT).login_as(&admin).send().await, "/");

    // A valid backup with its password: not read, whoever sends it.
    let posted = import(&app, &file, BACKUP_PASSWORD).await;
    assert_redirect(&posted, "/login");
    assert_eq!(posted.flashed("success"), None);
    let signed_in = import_request(&app)
        .login_as(&admin)
        .csrf()
        .multipart(&[
            ("backup", Some("backup.mymcps"), &file),
            ("password", None, BACKUP_PASSWORD.as_bytes()),
        ])
        .send()
        .await;
    assert_redirect(&signed_in, "/");

    let emails: Vec<String> = sqlx::query_scalar("select `email` from `users`")
        .fetch_all(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(emails, ["existing@example.com"]);
    assert_eq!(temporary_files(&app), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn an_import_stops_when_the_instance_was_set_up_while_it_ran() {
    let (source, _) = instance_with_admin().await;
    let file = seal(&snapshot(&source, &[]).await);

    // The first account was created between the check of the route and the
    // moment the rows are replaced.
    let app = TestApp::new().await;
    create_admin_with(&app, "first@example.com").await;
    let dir = BackupDir::create(&app.core.config).unwrap();
    let upload = dir.path().join("upload.mymcps");
    std::fs::write(&upload, &file).unwrap();

    let outcome = mymcps_core::backup::import(
        &app.core,
        &dir,
        &upload,
        BACKUP_PASSWORD.to_string(),
        &KeyDerivations::new(),
    )
    .await;

    assert!(
        matches!(outcome, Err(BackupError::AlreadySetUp)),
        "{outcome:?}"
    );
    let emails: Vec<String> = sqlx::query_scalar("select `email` from `users`")
        .fetch_all(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(emails, ["first@example.com"]);
    drop(dir);
    assert_eq!(temporary_files(&app), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn asks_for_a_file_and_for_its_password() {
    let (source, _) = instance_with_admin().await;
    let file = seal(&snapshot(&source, &[]).await);
    let app = TestApp::new().await;

    // Nothing at all, and what a browser sends when no file was chosen.
    let empty = import_request(&app).csrf().multipart(&[]).send().await;
    assert_redirect(&empty, IMPORT);
    assert_eq!(refused_field(&empty, "backup").as_deref(), Some(NO_FILE));
    assert_eq!(
        refused_field(&empty, "password").as_deref(),
        Some(NO_PASSWORD)
    );
    let page = app.get(IMPORT).session(empty.session()).send().await.text();
    assert!(page.contains("<div class=\"banner banner--critical\" role=\"alert\">"));
    assert!(page.contains(&format!("<p class=\"banner__title\">{NO_FILE}</p>")));
    assert!(page.contains(&format!(
        "type=\"file\" accept=\".mymcps\" required aria-invalid=\"true\" aria-describedby=\"backup-error\"><p class=\"field__error\" id=\"backup-error\">{NO_FILE}</p>"
    )));
    assert!(page.contains(&format!(
        "<p class=\"field__error\" id=\"password-error\">{NO_PASSWORD}</p>"
    )));

    let no_file = import_request(&app)
        .csrf()
        .multipart(&[
            ("backup", Some(""), b""),
            ("password", None, BACKUP_PASSWORD.as_bytes()),
        ])
        .send()
        .await;
    assert_refused(&app, &no_file, NO_FILE).await;
    assert_eq!(refused_field(&no_file, "password"), None);

    for blank in [&b""[..], b"   ", b" \r\n\t"] {
        let no_password = import_request(&app)
            .csrf()
            .multipart(&[
                ("backup", Some("backup.mymcps"), &file),
                ("password", None, blank),
            ])
            .send()
            .await;
        assert_refused(&app, &no_password, NO_PASSWORD).await;
        assert_eq!(refused_field(&no_password, "backup"), None);
    }
    let file_only = import_request(&app)
        .csrf()
        .multipart(&[("backup", Some("backup.mymcps"), &file)])
        .send()
        .await;
    assert_refused(&app, &file_only, NO_PASSWORD).await;
}

#[tokio::test]
async fn refuses_a_file_that_is_not_a_backup_or_comes_from_a_newer_version() {
    let (source, _) = instance_with_admin().await;
    let database = snapshot(&source, &[]).await;
    let file = seal(&database);
    let app = TestApp::new().await;

    let mut wrong_magic = file.clone();
    wrong_magic[0] = b'm';
    for not_a_backup in [
        &database[..],
        b"just some text, in a file with the right name",
        &file[..35],
        b"M",
        &wrong_magic,
    ] {
        let response = import(&app, not_a_backup, BACKUP_PASSWORD).await;
        assert_refused(&app, &response, NOT_A_BACKUP).await;
        assert_eq!(
            refused_field(&response, "backup").as_deref(),
            Some(NOT_A_BACKUP)
        );
    }

    // Version 2, another key derivation, and costs no reader takes: 8 KiB
    // and 512 MiB of memory.
    for (offset, value) in [(8, 2), (9, 2), (10, 13), (10, 19), (11, 4), (12, 2)] {
        let mut newer = file.clone();
        newer[offset] = value;
        let response = import(&app, &newer, BACKUP_PASSWORD).await;
        assert_refused(&app, &response, NEWER_VERSION).await;
    }
}

#[tokio::test]
async fn refuses_a_wrong_password_and_a_file_that_was_damaged() {
    let (source, admin) = instance_with_admin().await;
    // Enough rows for a file of several chunks.
    create_mcp(&source, admin.id, |mcp| {
        mcp.description = Some("a long description ".repeat(16_000));
    })
    .await;
    let database = snapshot(&source, &[]).await;
    let file = seal(&database);
    assert!(file.len() > 4 * 65_552, "more than four chunks");
    let app = TestApp::new().await;

    let wrong = import(&app, &file, "another password").await;
    assert_refused(&app, &wrong, WRONG_PASSWORD).await;
    assert_eq!(
        refused_field(&wrong, "password").as_deref(),
        Some(WRONG_PASSWORD)
    );
    let page = app.get(IMPORT).session(wrong.session()).send().await.text();
    assert!(page.contains(&format!(
        "<p class=\"field__error\" id=\"password-error\">{WRONG_PASSWORD}</p>"
    )));
    assert!(!page.contains("another password"));

    // A byte changed in the first chunk reads as a wrong password: nothing
    // tells the two apart.
    let mut first_chunk = file.clone();
    first_chunk[HEADER_BYTES + 1_000] ^= 1;
    let response = import(&app, &first_chunk, BACKUP_PASSWORD).await;
    assert_refused(&app, &response, WRONG_PASSWORD).await;

    let mut later_chunk = file.clone();
    later_chunk[HEADER_BYTES + 2 * 65_552 + 1_000] ^= 1;
    let mut longer = file.clone();
    longer.extend_from_slice(b"more");
    for damaged in [
        &later_chunk[..],
        // Cut at the end of the second chunk, inside the third, and short
        // of its last byte.
        &file[..HEADER_BYTES + 2 * 65_552],
        &file[..HEADER_BYTES + 2 * 65_552 + 100],
        &file[..file.len() - 1],
        // A header and nothing that could be a chunk.
        &file[..HEADER_BYTES],
        &file[..HEADER_BYTES + 15],
        &longer,
    ] {
        let response = import(&app, damaged, BACKUP_PASSWORD).await;
        assert_refused(&app, &response, DAMAGED).await;
        assert_eq!(refused_field(&response, "backup").as_deref(), Some(DAMAGED));
    }

    // What decrypts, and is not the metadata and the database of an instance.
    let long_key = "k".repeat(513);
    for metadata in [
        "not json".to_string(),
        "[]".to_string(),
        r#"{"createdAt":"2026-01-02T03:04:05.000Z"}"#.to_string(),
        r#"{"appKey":"sixteen chars ok"}"#.to_string(),
        r#"{"createdAt":"2026-01-02T03:04:05.000Z","appKey":"fifteen chars o"}"#.to_string(),
        json!({ "createdAt": "2026-01-02T03:04:05.000Z", "appKey": long_key }).to_string(),
    ] {
        let response = import(
            &app,
            &seal_with(&metadata, &database, BACKUP_PASSWORD),
            BACKUP_PASSWORD,
        )
        .await;
        assert_refused(&app, &response, DAMAGED).await;
    }
    let mut scrambled = database.clone();
    scrambled[4_096..12_288].fill(0xa5);
    for not_a_database in [
        &b""[..],
        b"SQLite format 3",
        b"SQLite format 3\0 and nothing that follows a header",
        &database[..database.len() / 2],
        &database[1..],
        &scrambled,
    ] {
        let response = import(&app, &seal(not_a_database), BACKUP_PASSWORD).await;
        assert_refused(&app, &response, DAMAGED).await;
    }
}

#[tokio::test]
async fn refuses_a_database_that_is_not_the_one_of_an_instance() {
    let (source, admin) = instance_with_admin().await;
    create_mcp(&source, admin.id, |_| {}).await;
    let app = TestApp::new().await;

    // The copy as it is would be imported: each change below is the one
    // reason of its refusal.
    let cases: [(&str, &[&'static str], &str); 18] = [
        (
            "a trigger",
            &[
                "create trigger `promote` after insert on `users` begin update `users` set `role` = 'admin'; end",
            ],
            DAMAGED,
        ),
        (
            "a view",
            &["create view `everyone` as select * from `users`"],
            DAMAGED,
        ),
        (
            "a migration from the future",
            &[
                "insert into `adonis_schema` (`name`, `batch`) values ('database/migrations/1999999999999_add_something_new', 9)",
            ],
            NEWER_VERSION,
        ),
        (
            "no administrator",
            &["update `users` set `role` = 'member'"],
            NO_ADMINISTRATOR,
        ),
        (
            "no user",
            &["delete from `mcps`", "delete from `users`"],
            NO_ADMINISTRATOR,
        ),
        ("a missing table", &["drop table `invites`"], DAMAGED),
        (
            "a table too many",
            &["create table `extras` (`id` integer primary key)"],
            DAMAGED,
        ),
        (
            "a missing column",
            &["alter table `mcps` drop column `description`"],
            DAMAGED,
        ),
        (
            "a column too many",
            &["alter table `users` add column `nickname` text"],
            DAMAGED,
        ),
        (
            "a column that is computed",
            &[
                "alter table `users` add column `shadow` text generated always as (hex(zeroblob(1000))) virtual",
            ],
            DAMAGED,
        ),
        (
            "an index on what is computed",
            &["create index `computed` on `users` (hex(`email`))"],
            DAMAGED,
        ),
        (
            "an index on some of the rows",
            &["create index `some` on `users` (`email`) where length(`email`) > 0"],
            DAMAGED,
        ),
        (
            "a constraint of its own",
            &[
                "alter table `users` add column `checked` text check (length(hex(zeroblob(1000))) > 0)",
            ],
            DAMAGED,
        ),
        (
            "settings the server cannot read",
            &[
                "PRAGMA ignore_check_constraints = ON",
                "insert or replace into `instance_settings` (`id`, `gateway_tool_mode`, `mcp_log_level`, `mcp_log_retention_days`, `mcp_auto_update_enabled`, `mcp_auto_update_cron`, `created_at`, `updated_at`) values (1, 'eager', 'everything', 14, 0, '0 3 * * *', '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
            ],
            DAMAGED,
        ),
        (
            "a row that refers to a user who is not there",
            &[
                "insert into `invites` (`token`, `email`, `role`, `created_by`, `expires_at`, `created_at`) values ('t', 'x@example.com', 'member', 999, '2030-01-01 00:00:00', '2026-01-01 00:00:00')",
            ],
            DAMAGED,
        ),
        (
            "an MCP of nobody",
            &["update `mcps` set `created_by` = 404"],
            DAMAGED,
        ),
        (
            "a row the schema of the instance refuses",
            &[
                "create table `copy` as select * from `users`",
                "drop table `users`",
                "create table `users` (`id` integer not null primary key autoincrement, `full_name` varchar(255) null, `email` varchar(254) null, `password` varchar(255) not null, `created_at` datetime not null, `updated_at` datetime null, `role` varchar(32) not null default 'member', `session_version` integer not null default '1')",
                "insert into `users` select * from `copy`",
                "drop table `copy`",
                "update `users` set `email` = null",
            ],
            DAMAGED,
        ),
        ("no ledger", &["drop table `adonis_schema`"], DAMAGED),
    ];
    // The copy itself is one an instance takes.
    let intact = seal(&snapshot(&source, &[]).await);
    assert!(open(BACKUP_PASSWORD, &intact).is_ok());

    for (name, tamper, message) in cases {
        let file = seal(&snapshot(&source, tamper).await);
        let response = import(&app, &file, BACKUP_PASSWORD).await;
        assert_redirect(&response, IMPORT);
        assert_eq!(refusal(&response).as_deref(), Some(message), "{name}");
        assert_eq!(
            refused_field(&response, "backup").as_deref(),
            Some(message),
            "{name}"
        );
        assert_untouched(&app).await;
    }

    // After all of these, the instance still takes the copy as it was.
    let imported = import(&app, &intact, BACKUP_PASSWORD).await;
    assert_redirect(&imported, "/login");
    assert_eq!(count(&app, "users").await, 1);
    assert_eq!(count(&app, "mcps").await, 1);
}

#[tokio::test]
async fn takes_one_import_at_a_time() {
    let (source, _) = instance_with_admin().await;
    let file = seal(&snapshot(&source, &[]).await);
    let app = TestApp::new().await;

    let under_way = app.state.backups.begin_import().expect("no import yet");
    assert!(app.state.backups.begin_import().is_none());

    let second = import(&app, &file, BACKUP_PASSWORD).await;
    assert_refused(&app, &second, UNDER_WAY).await;
    let page = app
        .get(IMPORT)
        .session(second.session())
        .send()
        .await
        .text();
    assert!(page.contains(&format!("<p class=\"banner__title\">{UNDER_WAY}</p>")));
    assert!(!page.contains("aria-invalid"));
    // A form without a file takes nobody's place.
    let no_file = import_request(&app).csrf().multipart(&[]).send().await;
    assert_eq!(refused_field(&no_file, "backup").as_deref(), Some(NO_FILE));

    drop(under_way);
    assert_redirect(&import(&app, &file, BACKUP_PASSWORD).await, "/login");
    // The place is free again once an import is over, however it ended.
    assert!(app.state.backups.begin_import().is_some());
}

#[tokio::test]
async fn refuses_a_file_larger_than_the_limit() {
    let (source, _) = instance_with_admin().await;
    let file = seal(&snapshot(&source, &[]).await);
    // The limit of a server is 4 GiB. Here it is the size of this file.
    let limit = file.len() as u64;
    let app = TestApp::with_state(
        |_| {},
        move |core| {
            let mut state = AppState::new(core);
            state.backups = BackupWork::with_limits(limit, Duration::from_secs(60));
            state
        },
    )
    .await;

    let mut larger = file.clone();
    larger.push(0);
    let response = import(&app, &larger, BACKUP_PASSWORD).await;
    assert_refused(&app, &response, TOO_LARGE).await;
    assert_eq!(
        refused_field(&response, "backup").as_deref(),
        Some(TOO_LARGE)
    );
    // The place of the import was given back.
    assert!(app.state.backups.begin_import().is_some());

    // A file of exactly the limit is taken.
    assert_redirect(&import(&app, &file, BACKUP_PASSWORD).await, "/login");
}

#[tokio::test]
async fn limits_imports_by_client_address() {
    let app = TestApp::new().await;
    let attempt = |from: &'static str| {
        app.post(IMPORT)
            .header("x-forwarded-for", from)
            .csrf()
            .multipart(&[("password", None, b"x")])
            .send()
    };

    for _ in 0..10 {
        assert_redirect(&attempt("198.51.100.20").await, IMPORT);
    }
    let limited = attempt("198.51.100.20").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header("retry-after").is_some());
    assert!(limited.text().contains("Too many requests"));
    // Counted whatever the request holds, before any of it is read.
    let forged = app
        .post(IMPORT)
        .header("x-forwarded-for", "198.51.100.20")
        .multipart(&[("backup", Some("backup.mymcps"), b"MYMCPSBK")])
        .send()
        .await;
    assert_eq!(forged.status, StatusCode::TOO_MANY_REQUESTS);

    assert_redirect(&attempt("198.51.100.21").await, IMPORT);
    assert_untouched(&app).await;
}

#[tokio::test]
async fn refuses_an_import_without_a_valid_token_and_writes_nothing() {
    let (source, _) = instance_with_admin().await;
    let file = seal(&snapshot(&source, &[]).await);
    let app = TestApp::new().await;
    let root = app.core.config.data_dir.join(TEMPORARY_DIRECTORY);
    let parts: [(&str, Option<&str>, &[u8]); 2] = [
        ("backup", Some("backup.mymcps"), &file),
        ("password", None, BACKUP_PASSWORD.as_bytes()),
    ];

    // No token at all: a browser is sent back with a message, anything
    // else is told so.
    let browser = import_request(&app).multipart(&parts).send().await;
    assert_eq!(browser.status, StatusCode::FOUND);
    assert_eq!(browser.redirect_path().as_deref(), Some("/"));
    assert_eq!(
        browser.flashed("error"),
        Some(json!("Invalid or expired CSRF token"))
    );
    let script = import_request(&app).multipart(&parts).api().send().await;
    assert_eq!(script.status, StatusCode::FORBIDDEN);
    assert_eq!(script.text(), "Invalid or expired CSRF token");

    // A session of this visitor, and the token of its forms.
    let session = Session::from_values(Map::new());
    let token = csrf_token(&session);
    let cookie = session.snapshot();
    let send = |parts: Vec<(&'static str, Option<&'static str>, Vec<u8>)>| {
        let parts: Vec<(&str, Option<&str>, &[u8])> = parts
            .iter()
            .map(|(name, file_name, content)| (*name, *file_name, &content[..]))
            .collect();
        let (body, content_type) = multipart_form(&parts);
        import_request(&app)
            .session(cookie.clone())
            .raw_body(body, &content_type)
            .api()
            .send()
    };
    let backup = || ("backup", Some("backup.mymcps"), file.clone());
    let password = || ("password", None, BACKUP_PASSWORD.as_bytes().to_vec());

    // The token of another session, a token after the file, and a form
    // that is not one.
    let stranger = csrf_token(&Session::from_values(Map::new()));
    for refused in [
        send(vec![
            ("_csrf", None, stranger.into_bytes()),
            backup(),
            password(),
        ])
        .await,
        send(vec![
            ("_csrf", None, b"made.up".to_vec()),
            backup(),
            password(),
        ])
        .await,
        send(vec![
            backup(),
            ("_csrf", None, token.clone().into_bytes()),
            password(),
        ])
        .await,
        send(vec![backup(), password()]).await,
        import_request(&app)
            .session(cookie.clone())
            .form(&[("password", BACKUP_PASSWORD), ("_csrf", &token)])
            .api()
            .send()
            .await,
    ] {
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert_eq!(refused.text(), "Invalid or expired CSRF token");
        // Not a file, not even the directory it would have gone to.
        assert!(!root.exists());
    }
    assert_untouched(&app).await;

    // With the token in a header, as the page's script sends it, a body
    // that is not a form is no import either.
    let not_a_form = import_request(&app)
        .csrf()
        .raw_body(file.clone(), "application/octet-stream")
        .send()
        .await;
    assert_eq!(not_a_form.status, StatusCode::BAD_REQUEST);
    let unknown_field = import_request(&app)
        .csrf()
        .multipart(&[("other", None, b"x")])
        .send()
        .await;
    assert_eq!(unknown_field.status, StatusCode::BAD_REQUEST);
    let cut_short = {
        let (body, content_type) = multipart_form(&parts);
        import_request(&app)
            .csrf()
            .raw_body(body.slice(..body.len() - 60), &content_type)
            .send()
            .await
    };
    assert_eq!(cut_short.status, StatusCode::BAD_REQUEST);
    let two_files = import_request(&app)
        .csrf()
        .multipart(&[parts[0], parts[0], parts[1]])
        .send()
        .await;
    assert_eq!(two_files.status, StatusCode::BAD_REQUEST);
    let long_password = import_request(&app)
        .csrf()
        .multipart(&[("password", None, &[b'p'; 20_000])])
        .send()
        .await;
    assert_eq!(long_password.status, StatusCode::PAYLOAD_TOO_LARGE);
    // A body in which no part ever starts, or in which the headers of one
    // never end, would be kept in memory while its end is looked for: it is
    // refused once it is larger than a form without its file can be.
    let (_, content_type) = multipart_form(&parts);
    let boundary = content_type.split("boundary=").nth(1).unwrap();
    let endless = vec![b'0'; 3 * 1024 * 1024];
    let no_part = import_request(&app)
        .raw_body(endless.clone(), &content_type)
        .send()
        .await;
    assert_eq!(no_part.status, StatusCode::PAYLOAD_TOO_LARGE);
    let mut no_end_of_headers = format!("--{boundary}\r\ncontent-disposition: ").into_bytes();
    no_end_of_headers.extend_from_slice(&endless);
    let headers_only = import_request(&app)
        .csrf()
        .raw_body(no_end_of_headers, &content_type)
        .send()
        .await;
    assert_eq!(headers_only.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_untouched(&app).await;

    // The token in the form, before the file, or in the header: an import.
    let imported = send(vec![
        ("_csrf", None, token.into_bytes()),
        ("password", None, BACKUP_PASSWORD.as_bytes().to_vec()),
        backup(),
    ])
    .await;
    assert_redirect(&imported, "/login");
    assert_eq!(count(&app, "users").await, 1);

    let other = TestApp::new().await;
    let (body, content_type) = multipart_form(&parts);
    let by_header = import_request(&other)
        .csrf()
        .raw_body(body, &content_type)
        .send()
        .await;
    assert_redirect(&by_header, "/login");
    assert_eq!(count(&other, "users").await, 1);
}

/// A backup the Node app exported from its Settings page: one administrator
/// and one MCP behind a bearer token, under a key of its own. The tests of
/// the Node app import the one this server exported from the same instance.
const FROM_NODE: &[u8] = include_bytes!("fixtures/backup-from-node.mymcps");
const FROM_NODE_PASSWORD: &str = "fixture backup password";

#[tokio::test]
async fn imports_a_backup_the_node_app_exported() {
    let (metadata, _) = open(FROM_NODE_PASSWORD, FROM_NODE).unwrap();
    assert_eq!(metadata["app"]["runtime"], "node");

    let target = fresh_instance().await;
    assert_ne!(json!(target.core.config.app_key), metadata["appKey"]);

    let imported = import(&target, FROM_NODE, FROM_NODE_PASSWORD).await;
    assert_redirect(&imported, "/login");
    assert_eq!(imported.flashed("success"), Some(json!(IMPORTED)));

    // Its administrator signs in with the password chosen on the Node app.
    let signed_in = target
        .post("/login")
        .csrf()
        .form(&[
            ("email", "admin@fixture.example"),
            ("password", "fixture admin password"),
        ])
        .send()
        .await;
    assert_redirect(&signed_in, "/");

    // Its MCP is there, and the secret the Node app encrypted with its key
    // is read with the key of this instance.
    let mcp = Mcp::find(&*target.core.db, 1).await.unwrap().unwrap();
    assert_eq!(mcp.name, "Fixture MCP");
    assert_eq!(
        target.core.decrypt_secret(mcp.auth_bearer.as_deref()),
        Some("fixture-bearer-4390".to_string())
    );
    assert_eq!(
        count(&target, "adonis_schema").await,
        MIGRATIONS.len() as i64
    );
}
