//! Reading a backup back into an instance that has no user yet.
//!
//! The file comes from whoever holds the setup screen, and so does the
//! database inside it once the password opened it: an authenticated
//! container says that the file was not altered, not that its author meant
//! well. Nothing of the database is trusted before it was checked, it is
//! only ever opened with `trusted_schema` off, and its rows reach the
//! instance through the schema of the instance, never through its own.
//!
//! Every failure leaves the database of the instance as it was.

use std::io::{self, BufReader, BufWriter, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{AssertSqlSafe, ConnectOptions, Connection, Row};
use tokio::io::AsyncReadExt;

use super::container::{self, HEADER_BYTES, Header, TAG_BYTES};
use super::{
    BackupDir, BackupError, KeyDerivations, Metadata, blocking, private_file, sqlite_path,
};
use crate::context::Core;
use crate::crypto::Encryption;
use crate::db::migrations::MIGRATIONS;

/// The name of the decrypted database in its [`BackupDir`].
const DATABASE_FILE: &str = "db.sqlite3";
/// What every SQLite database file starts with.
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
/// How long a statement waits for another writer of the same database.
const BUSY_TIMEOUT: Duration = Duration::from_secs(30);
/// How many ids are read at a time while secrets are encrypted anew. The
/// rows themselves are read one by one.
const REENCRYPTION_PAGE: usize = 500;
/// The longest value that is read as a ciphertext. The largest one an
/// instance writes, the arguments of a tool call that waits for approval,
/// is a tenth of this: a longer value is not a secret of this app, and is
/// not worth the memory of whoever wrote it there.
const MAX_CIPHERTEXT_CHARS: usize = 4 * 1024 * 1024;

/// The columns that hold ciphertext made with `APP_KEY`.
struct SecretColumns {
    table: &'static str,
    /// One ciphertext each.
    single: &'static [&'static str],
    /// A JSON object whose values are ciphertexts.
    maps: &'static [&'static str],
}

const SECRET_COLUMNS: &[SecretColumns] = &[
    SecretColumns {
        table: "mcps",
        single: &[
            "auth_bearer",
            "auth_header_value",
            "oauth_client_secret",
            "oauth_access_token",
            "oauth_refresh_token",
            "builtin_password",
        ],
        maps: &["npm_env", "builtin_settings"],
    },
    SecretColumns {
        table: "approval_requests",
        single: &["arguments", "summary"],
        maps: &[],
    },
];

/// The tables whose rows a backup replaces: those of the instance, less
/// SQLite's own and the rate limit counters, which are never imported.
const IMPORTED_TABLES: &str =
    "`type` = 'table' and `name` not like 'sqlite\\_%' escape '\\' and `name` <> 'rate_limits'";

/// What an import did.
#[derive(Debug, Clone)]
pub struct Imported {
    /// When the backup was made, as its metadata says.
    pub created_at: String,
    /// The migrations of this version that the backup lacked: they ran on
    /// its database before its rows were copied.
    pub migrated: Vec<&'static str>,
    /// Whether the backup came from an instance with another `APP_KEY`: its
    /// secrets were encrypted anew with the key of this one.
    pub reencrypted: bool,
    /// How many accounts the instance has now.
    pub users: i64,
}

/// Whether the instance, rather than the backup, is why a statement failed:
/// a full disk, a busy database. Everything else a statement that reads a
/// backup can fail with is the fault of the backup.
fn is_instance_trouble(error: &sqlx::Error) -> bool {
    // The primary result codes of SQLite: PERM, ABORT, BUSY, LOCKED, NOMEM,
    // READONLY, INTERRUPT, IOERR, FULL, CANTOPEN, PROTOCOL.
    const TROUBLE: &[i32] = &[3, 4, 5, 6, 7, 8, 9, 10, 13, 14, 15];
    match error {
        sqlx::Error::Database(error) => error
            .code()
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| TROUBLE.contains(&(code & 0xff))),
        sqlx::Error::ColumnDecode { .. }
        | sqlx::Error::Decode(_)
        | sqlx::Error::ColumnNotFound(_)
        | sqlx::Error::ColumnIndexOutOfBounds { .. }
        | sqlx::Error::TypeNotFound { .. }
        | sqlx::Error::RowNotFound => false,
        _ => true,
    }
}

/// The failure of a statement that reads the database of a backup.
fn blame(error: sqlx::Error) -> BackupError {
    if is_instance_trouble(&error) {
        return BackupError::Database(error);
    }
    // The message of SQLite names tables and columns, never what they hold.
    tracing::warn!(%error, "The database of a backup was refused");
    BackupError::Damaged
}

fn quote(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

/// Read a backup and make the instance the one it holds.
///
/// `file` is the encrypted file, and `dir` the private directory the
/// decrypted database is written to: the caller deletes it by dropping it,
/// whatever the outcome. In order:
///
/// 1. the header is read, and the key derived from `password`;
/// 2. the body is decrypted to a file;
/// 3. the metadata is parsed;
/// 4. the database is checked: a SQLite file that passes `quick_check`,
///    holds tables and indexes only, only migrations this version knows,
///    and an administrator;
/// 5. the migrations it lacks are run on it;
/// 6. its secrets are encrypted with the key of this instance when the
///    backup was made with another;
/// 7. its rows replace those of the instance, in one transaction that
///    stops if the instance got a user meanwhile.
///
/// Nobody else may write to the instance meanwhile: the caller runs one
/// import at a time, and tells the parts of the server that keep data in
/// memory when it is done.
pub async fn import(
    core: &Arc<Core>,
    dir: &BackupDir,
    file: &Path,
    password: String,
    derivations: &KeyDerivations,
) -> Result<Imported, BackupError> {
    let source = file.to_path_buf();
    let (header, body_len) = blocking(move || {
        let mut input = std::fs::File::open(&source)?;
        let file_len = input.metadata()?.len();
        let header = Header::read(&mut input)?;
        Ok((header, file_len.saturating_sub(HEADER_BYTES as u64)))
    })
    .await?;
    // Not worth a key: the file ends before its first chunk does.
    if body_len < TAG_BYTES as u64 {
        return Err(BackupError::Damaged);
    }

    let key = derivations.derive(password, header.clone()).await?;
    let database = dir.path().join(DATABASE_FILE);
    let (source, target) = (file.to_path_buf(), database.clone());
    let metadata = blocking(move || {
        let mut input = std::fs::File::open(&source)?;
        input.seek(SeekFrom::Start(HEADER_BYTES as u64))?;
        let output = private_file().open(&target)?;
        container::read_body(
            &key,
            &header,
            BufReader::new(input),
            body_len,
            BufWriter::new(output),
        )
    })
    .await?;
    let metadata = Metadata::parse(&metadata)?;

    let (migrated, reencrypted) = prepare(core, &database, &metadata).await?;
    let users = replace_rows(core, &database).await?;
    Ok(Imported {
        created_at: metadata.created_at,
        migrated,
        reencrypted,
        users,
    })
}

async fn starts_like_sqlite(database: &Path) -> io::Result<bool> {
    let mut file = tokio::fs::File::open(database).await?;
    let mut magic = Vec::with_capacity(SQLITE_MAGIC.len());
    (&mut file)
        .take(SQLITE_MAGIC.len() as u64)
        .read_to_end(&mut magic)
        .await?;
    Ok(magic == SQLITE_MAGIC)
}

/// Steps 4 to 6, on a connection that only ever sees the decrypted file.
async fn prepare(
    core: &Core,
    database: &Path,
    metadata: &Metadata,
) -> Result<(Vec<&'static str>, bool), BackupError> {
    if !starts_like_sqlite(database).await? {
        return Err(BackupError::Damaged);
    }
    let mut connection = SqliteConnectOptions::new()
        .filename(sqlite_path(database)?)
        .create_if_missing(false)
        // The schema of the file was written by a stranger: no function, no
        // virtual table it names is run on its word, and cells that lie
        // about their size are caught where they are read.
        .pragma("trusted_schema", "OFF")
        .pragma("cell_size_check", "ON")
        .busy_timeout(BUSY_TIMEOUT)
        .connect()
        .await
        .map_err(blame)?;

    let outcome = prepare_on(&mut connection, core, metadata).await;
    // The file is attached to the database of the instance next: this
    // connection lets go of it first.
    let closed = connection.close().await;
    let prepared = outcome?;
    closed.map_err(blame)?;
    Ok(prepared)
}

async fn prepare_on(
    connection: &mut SqliteConnection,
    core: &Core,
    metadata: &Metadata,
) -> Result<(Vec<&'static str>, bool), BackupError> {
    check(connection).await?;

    let migrated = crate::db::migrate(connection)
        .await
        .map_err(|error| match error {
            crate::Error::Database(error) | crate::Error::Migration { source: error, .. } => {
                blame(error)
            }
            crate::Error::Io(error) => BackupError::Io(error),
            crate::Error::Config(message) => BackupError::Io(io::Error::other(message)),
        })?;

    let reencrypted = metadata.app_key != core.config.app_key;
    if reencrypted {
        let from = Encryption::new(&metadata.app_key);
        reencrypt(connection, &from, &core.encryption)
            .await
            .map_err(blame)?;
    }
    Ok((migrated, reencrypted))
}

/// Step 4: what the database must be before anything is run on it.
async fn check(connection: &mut SqliteConnection) -> Result<(), BackupError> {
    let report: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_all(&mut *connection)
        .await
        .map_err(blame)?;
    if report != ["ok"] {
        return Err(BackupError::Damaged);
    }

    // Tables and their indexes, each with pages of its own: no trigger and
    // no view, which would run on the rows that are copied, and no virtual
    // table, which has no page and is read by code of its own.
    let others: i64 = sqlx::query_scalar(
        "select count(*) from `sqlite_master` where `type` not in ('table', 'index') or `rootpage` is null or `rootpage` < 1",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(blame)?;
    if others > 0 {
        return Err(BackupError::Damaged);
    }

    // Counted where they are: the ledger of a stranger is not read into
    // memory before it is known to be short.
    let known = vec!["?"; MIGRATIONS.len()].join(", ");
    let mut ledger = sqlx::query_as::<_, (i64, i64)>(AssertSqlSafe(format!(
        "select count(*), coalesce(sum(`name` is null or `name` not in ({known})), 0) from `adonis_schema`"
    )));
    for migration in MIGRATIONS {
        ledger = ledger.bind(migration.name);
    }
    let (applied, unknown) = ledger.fetch_one(&mut *connection).await.map_err(blame)?;
    if unknown > 0 {
        return Err(BackupError::NewerVersion);
    }
    // A migration runs once.
    if applied > MIGRATIONS.len() as i64 {
        return Err(BackupError::Damaged);
    }

    let administrators: i64 =
        sqlx::query_scalar("select count(*) from `users` where `role` = 'admin'")
            .fetch_one(&mut *connection)
            .await
            .map_err(blame)?;
    if administrators == 0 {
        return Err(BackupError::NoAdministrator);
    }
    Ok(())
}

/// One ciphertext, encrypted anew. `None` for a value that does not
/// decrypt with the key of the backup: it is left as it is.
fn reencrypt_value(stored: &str, from: &Encryption, to: &Encryption) -> Option<String> {
    from.decrypt(stored).map(|secret| to.encrypt(&secret))
}

/// A JSON object of ciphertexts, each encrypted anew, names and order
/// kept. `None` when nothing in it decrypts.
fn reencrypt_map(stored: &str, from: &Encryption, to: &Encryption) -> Option<String> {
    let Ok(Value::Object(mut entries)) = serde_json::from_str::<Value>(stored) else {
        return None;
    };
    let mut changed = false;
    for value in entries.values_mut() {
        if let Value::String(ciphertext) = value
            && let Some(next) = reencrypt_value(ciphertext, from, to)
        {
            *ciphertext = next;
            changed = true;
        }
    }
    changed.then(|| Value::Object(entries).to_string())
}

/// Step 6: decrypt every secret with the key of the backup and encrypt it
/// with the key of this instance.
async fn reencrypt(
    connection: &mut SqliteConnection,
    from: &Encryption,
    to: &Encryption,
) -> Result<(), sqlx::Error> {
    let mut transaction = connection.begin().await?;
    for columns in SECRET_COLUMNS {
        let table = columns.table;
        let names: Vec<&str> = columns.single.iter().chain(columns.maps).copied().collect();
        // A value that is not text, or longer than any ciphertext, is no
        // secret of this app: it is read as missing, and left as it is.
        let selected: Vec<String> = names
            .iter()
            .map(|name| {
                format!(
                    "case when typeof(`{name}`) = 'text' and length(`{name}`) <= {MAX_CIPHERTEXT_CHARS} then `{name}` end"
                )
            })
            .collect();
        let ids = format!(
            "select `id` from `{table}` where `id` > ? order by `id` limit {REENCRYPTION_PAGE}"
        );
        let secrets = format!(
            "select {} from `{table}` where `id` = ?",
            selected.join(", ")
        );

        let mut after = i64::MIN;
        loop {
            let page: Vec<i64> = sqlx::query_scalar(AssertSqlSafe(ids.clone()))
                .bind(after)
                .fetch_all(&mut *transaction)
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            after = *last;

            // One row in memory at a time.
            for id in page {
                let row = sqlx::query(AssertSqlSafe(secrets.clone()))
                    .bind(id)
                    .fetch_optional(&mut *transaction)
                    .await?;
                let Some(row) = row else {
                    continue;
                };
                for (index, name) in names.iter().enumerate() {
                    let Some(stored) = row.try_get::<Option<String>, _>(index)? else {
                        continue;
                    };
                    let next = if index < columns.single.len() {
                        reencrypt_value(&stored, from, to)
                    } else {
                        reencrypt_map(&stored, from, to)
                    };
                    let Some(next) = next else {
                        continue;
                    };
                    sqlx::query(AssertSqlSafe(format!(
                        "update `{table}` set `{name}` = ? where `id` = ?"
                    )))
                    .bind(next)
                    .bind(id)
                    .execute(&mut *transaction)
                    .await?;
                }
            }
        }
    }
    transaction.commit().await
}

/// Step 7, on a connection of its own to the database of the instance:
/// none of the pool, which a request could be using.
async fn replace_rows(core: &Core, backup: &Path) -> Result<i64, BackupError> {
    let mut live = SqliteConnectOptions::new()
        .filename(sqlite_path(&core.config.database_path())?)
        .create_if_missing(false)
        // Rows are deleted and inserted table by table, in no particular
        // order: the keys are checked once, before the commit. Set here
        // because SQLite ignores it inside a transaction.
        .foreign_keys(false)
        // This connection reads the pages of the backup too.
        .pragma("trusted_schema", "OFF")
        .pragma("cell_size_check", "ON")
        .busy_timeout(BUSY_TIMEOUT)
        .connect()
        .await?;

    let attached = sqlx::query("attach database ? as `backup`")
        .bind(sqlite_path(backup)?)
        .execute(&mut live)
        .await;
    let outcome = match attached {
        Ok(_) => copy_rows(&mut live).await,
        Err(error) => Err(blame(error)),
    };
    // A transaction that did not commit is rolled back before this.
    if let Err(error) = live.close().await {
        tracing::warn!(%error, "The connection of an import did not close cleanly");
    }
    outcome
}

async fn columns_of(
    connection: &mut SqliteConnection,
    schema: &'static str,
    table: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("select `name` from pragma_table_info(?, ?)")
        .bind(table)
        .bind(schema)
        .fetch_all(connection)
        .await
}

async fn copy_rows(live: &mut SqliteConnection) -> Result<i64, BackupError> {
    // The write lock, from the first statement: nobody creates the first
    // account between the check below and the commit.
    let mut transaction = live.begin_with("BEGIN IMMEDIATE").await?;
    let users: i64 = sqlx::query_scalar("select count(*) from `main`.`users`")
        .fetch_one(&mut *transaction)
        .await?;
    if users > 0 {
        return Err(BackupError::AlreadySetUp);
    }

    // Table and column names come from the schema of the instance. The
    // backup must have the same ones, and is never asked for its own.
    let differing: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "select count(*) from (\
           select * from (select `name` from `main`.`sqlite_master` where {IMPORTED_TABLES} \
             except select `name` from `backup`.`sqlite_master` where {IMPORTED_TABLES}) \
           union all \
           select * from (select `name` from `backup`.`sqlite_master` where {IMPORTED_TABLES} \
             except select `name` from `main`.`sqlite_master` where {IMPORTED_TABLES}))"
    )))
    .fetch_one(&mut *transaction)
    .await
    .map_err(blame)?;
    if differing > 0 {
        tracing::warn!("A backup was refused: its tables are not those of this version");
        return Err(BackupError::Damaged);
    }
    let tables: Vec<String> = sqlx::query_scalar(AssertSqlSafe(format!(
        "select `name` from `main`.`sqlite_master` where {IMPORTED_TABLES} order by `name`"
    )))
    .fetch_all(&mut *transaction)
    .await?;
    let mut columns = Vec::with_capacity(tables.len());
    for table in &tables {
        let ours = columns_of(&mut transaction, "main", table).await?;
        let mut theirs = columns_of(&mut transaction, "backup", table)
            .await
            .map_err(blame)?;
        let mut sorted = ours.clone();
        sorted.sort();
        theirs.sort();
        if sorted != theirs {
            tracing::warn!(
                table,
                "A backup was refused: the columns of a table are not those of this version"
            );
            return Err(BackupError::Damaged);
        }
        columns.push(ours);
    }

    for table in &tables {
        sqlx::query(AssertSqlSafe(format!(
            "delete from `main`.{}",
            quote(table)
        )))
        .execute(&mut *transaction)
        .await?;
    }
    for (table, columns) in tables.iter().zip(&columns) {
        let table = quote(table);
        let columns: Vec<String> = columns.iter().map(|column| quote(column)).collect();
        let columns = columns.join(", ");
        sqlx::query(AssertSqlSafe(format!(
            "insert into `main`.{table} ({columns}) select {columns} from `backup`.{table}"
        )))
        .execute(&mut *transaction)
        .await
        .map_err(blame)?;
    }

    // Ids keep counting from where they were, also past rows that were
    // deleted before the backup was made.
    let sequences: i64 = sqlx::query_scalar(
        "select count(*) from `backup`.`sqlite_master` where `type` = 'table' and `name` = 'sqlite_sequence'",
    )
    .fetch_one(&mut *transaction)
    .await
    .map_err(blame)?;
    if sequences == 0 {
        return Err(BackupError::Damaged);
    }
    sqlx::query("delete from `main`.`sqlite_sequence`")
        .execute(&mut *transaction)
        .await?;
    sqlx::query(AssertSqlSafe(format!(
        "insert into `main`.`sqlite_sequence` (`name`, `seq`) select `name`, `seq` from `backup`.`sqlite_sequence` \
         where `name` in (select `name` from `main`.`sqlite_master` where {IMPORTED_TABLES})"
    )))
    .execute(&mut *transaction)
    .await
    .map_err(blame)?;

    // Enforcement was off for the copy: a row that points at nothing stops it here.
    let violation = sqlx::query("PRAGMA main.foreign_key_check")
        .fetch_optional(&mut *transaction)
        .await?;
    if violation.is_some() {
        tracing::warn!("A backup was refused: a row refers to a row that is not there");
        return Err(BackupError::Damaged);
    }

    let users: i64 = sqlx::query_scalar("select count(*) from `main`.`users`")
        .fetch_one(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(users)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_identifiers_whatever_they_hold() {
        assert_eq!(quote("users"), "`users`");
        assert_eq!(quote("a`b"), "`a``b`");
        assert_eq!(
            quote("x`; drop table users; --"),
            "`x``; drop table users; --`"
        );
    }

    #[test]
    fn encrypts_a_map_anew_value_by_value() {
        // The key of the instance the backup was made on, and the key of the
        // one it is imported into.
        let from = Encryption::new(&crate::config::generate_app_key());
        let to = Encryption::new(&crate::config::generate_app_key());

        let stored = format!(
            "{{\"B\":\"{}\",\"A\":\"{}\",\"left\":\"not a ciphertext\",\"n\":1}}",
            from.encrypt("second"),
            from.encrypt("first")
        );
        let next = reencrypt_map(&stored, &from, &to).unwrap();
        let Value::Object(entries) = serde_json::from_str::<Value>(&next).unwrap() else {
            panic!("an object");
        };
        assert_eq!(
            entries.keys().collect::<Vec<_>>(),
            ["B", "A", "left", "n"],
            "names and order are kept"
        );
        assert_eq!(
            to.decrypt(entries["B"].as_str().unwrap()).unwrap(),
            "second"
        );
        assert_eq!(to.decrypt(entries["A"].as_str().unwrap()).unwrap(), "first");
        assert_eq!(entries["left"], "not a ciphertext");
        assert_eq!(entries["n"], 1);

        // Nothing to do: the column is left as it is, byte for byte.
        for untouched in ["", "[]", "{not json", "{ \"A\" : \"plain\" }", "\"text\""] {
            assert_eq!(reencrypt_map(untouched, &from, &to), None, "{untouched}");
        }
        assert_eq!(reencrypt_value("not a ciphertext", &from, &to), None);
        assert_eq!(reencrypt_value(&to.encrypt("already"), &from, &to), None);
    }
}
