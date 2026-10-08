//! The SQLite database: the same file, schema and migration ledger as the
//! AdonisJS app, so an existing instance keeps its data.

use std::ops::Deref;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};
use sqlx::{AssertSqlSafe, Connection, Sqlite, Transaction};

use crate::error::{Error, Result};

pub mod migrations;
#[macro_use]
pub(crate) mod model;

pub use sqlx::sqlite::SqliteExecutor;

/// Handle to the database. Cheap to clone. Dereferences to the pool, so
/// `&*db` is an executor for any query.
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Deref for Db {
    type Target = SqlitePool;

    fn deref(&self) -> &SqlitePool {
        &self.pool
    }
}

impl Db {
    /// Open the database file, creating it and its directory when missing.
    /// Call [`Db::migrate`] before using it.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(directory) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(directory)?;
        }

        let options = SqliteConnectOptions::from_str("sqlite://")?
            .filename(path)
            .create_if_missing(true)
            // better-sqlite3 enforced foreign keys: deletes cascade.
            .foreign_keys(true)
            // Readers do not wait for the one writer.
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Start a write transaction. It takes the write lock at once, so that
    /// two transactions that read and then write wait for each other instead
    /// of one failing halfway. Pass `&mut *tx` as the executor, and commit.
    pub async fn begin(&self) -> Result<Transaction<'static, Sqlite>, sqlx::Error> {
        self.pool.begin_with("BEGIN IMMEDIATE").await
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Bring the schema up to date and return the names of the migrations
    /// that ran. The ledger is the `adonis_schema` table of the Node app,
    /// with the same names, so each side sees what the other applied.
    pub async fn migrate(&self) -> Result<Vec<&'static str>> {
        let mut connection = self.pool.acquire().await?;
        let connection: &mut SqliteConnection = &mut connection;

        sqlx::query(
            "create table if not exists `adonis_schema` (`id` integer not null primary key autoincrement, `name` varchar(255) not null, `batch` integer not null, `migration_time` datetime default CURRENT_TIMESTAMP)",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "create table if not exists `adonis_schema_versions` (`version` integer, primary key (`version`))",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "insert into `adonis_schema_versions` (`version`) select 2 where not exists (select 1 from `adonis_schema_versions`)",
        )
        .execute(&mut *connection)
        .await?;

        let applied: Vec<String> = sqlx::query_scalar("select `name` from `adonis_schema`")
            .fetch_all(&mut *connection)
            .await?;
        for name in &applied {
            if !migrations::MIGRATIONS
                .iter()
                .any(|migration| migration.name == name)
            {
                tracing::warn!(
                    migration = %name,
                    "The database has a migration this version does not know; it was created by a newer version"
                );
            }
        }
        let batch: i64 =
            sqlx::query_scalar("select coalesce(max(`batch`), 0) + 1 from `adonis_schema`")
                .fetch_one(&mut *connection)
                .await?;

        let mut ran = Vec::new();
        for migration in migrations::MIGRATIONS {
            if applied.iter().any(|name| name == migration.name) {
                continue;
            }
            run_migration(connection, migration, batch)
                .await
                .map_err(|source| Error::Migration {
                    name: migration.name.to_string(),
                    source,
                })?;
            ran.push(migration.name);
        }
        Ok(ran)
    }
}

/// Two migrations rebuild a table (create, copy, drop, rename). With foreign
/// keys enforced, dropping the old table would delete or detach the rows
/// that reference it, so enforcement is off while a migration runs. SQLite
/// ignores that pragma inside a transaction: it is set around it.
async fn run_migration(
    connection: &mut SqliteConnection,
    migration: &migrations::Migration,
    batch: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await?;

    let outcome = async {
        let mut transaction = connection.begin().await?;
        for statement in migration.statements {
            sqlx::query(AssertSqlSafe(*statement))
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query("insert into `adonis_schema` (`name`, `batch`) values (?, ?)")
            .bind(migration.name)
            .bind(batch)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await
    }
    .await;

    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await?;
    outcome
}
