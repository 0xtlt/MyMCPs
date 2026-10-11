use sqlx::sqlite::SqliteExecutor;
use sqlx::{Sqlite, Transaction};

use crate::crypto::{hash_password, verify_password};
use crate::db::Db;
use crate::models::{Team, TeamRole};
use crate::time::Timestamp;

model! {
    table = "users", created_at = true, updated_at = true;
    pub struct User {
        pub full_name: Option<String>,
        pub email: String,
        /// The scrypt hash, never the password.
        pub password: String,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
        /// A session only authenticates while it carries this version.
        pub session_version: i64,
        /// The team this person last worked in, shown when they sign in.
        pub current_team_id: Option<i64>,
    }
}

/// Run a password hash or check off the async threads: scrypt holds a core
/// for tens of milliseconds.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, sqlx::Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| sqlx::Error::Protocol(format!("password hashing task failed: {error}")))
}

impl User {
    /// A user ready to insert, with the password hashed. An account is
    /// nothing without a team: insert it with [`User::create_first`] or
    /// [`User::create`].
    pub async fn with_password(
        email: &str,
        full_name: Option<&str>,
        password: &str,
    ) -> Result<Self, sqlx::Error> {
        let password = password.to_string();
        Ok(Self {
            email: email.to_string(),
            full_name: full_name.map(str::to_string),
            password: blocking(move || hash_password(&password)).await?,
            // The column default is not read back on insert.
            session_version: 1,
            ..Default::default()
        })
    }

    /// The name this person goes by: their name, or their email when they
    /// have none.
    pub fn display_name(&self) -> &str {
        self.full_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.email)
    }

    /// Store the first account of an instance, which owns the Root Team.
    pub async fn create_first(
        &mut self,
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<(), sqlx::Error> {
        self.insert(&mut **transaction).await?;
        Team::attach(&mut **transaction, Team::ROOT_ID, self.id, TeamRole::Owner).await
    }

    /// Store any later account, with the personal team it owns.
    pub async fn create(
        &mut self,
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<Team, sqlx::Error> {
        self.insert(&mut **transaction).await?;
        Team::create_personal(transaction, self).await
    }

    /// Whether this person runs the instance: an owner or an administrator
    /// of the Root Team, whichever team they are working in.
    pub async fn is_instance_admin<'e, E>(&self, db: E) -> Result<bool, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        Ok(Team::role_of(db, Team::ROOT_ID, self.id)
            .await?
            .is_some_and(TeamRole::is_admin))
    }

    /// Remember the team this person works in.
    pub async fn remember_team<'e, E>(&mut self, db: E, team_id: i64) -> Result<(), sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        self.current_team_id = Some(team_id);
        self.save(db).await
    }

    pub fn initials(&self) -> String {
        let mut parts = match &self.full_name {
            Some(name) if !name.is_empty() => name.split(' '),
            _ => self.email.split('@'),
        };
        let first = parts.next().unwrap_or("");
        let last = parts.next().unwrap_or("");
        let initials: String = if !first.is_empty() && !last.is_empty() {
            first.chars().take(1).chain(last.chars().take(1)).collect()
        } else {
            first.chars().take(2).collect()
        };
        initials.to_uppercase()
    }

    pub async fn find_by_email<'e, E>(db: E, email: &str) -> Result<Option<Self>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as("select * from `users` where `email` = ?")
            .bind(email)
            .fetch_optional(db)
            .await
    }

    /// Whether the first account, which owns the Root Team, exists.
    pub async fn setup_complete<'e, E>(db: E) -> Result<bool, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        let total: i64 = sqlx::query_scalar("select count(*) from `users`")
            .fetch_one(db)
            .await?;
        Ok(total > 0)
    }

    /// The user with this email and password. When no account has the
    /// email, a hash is computed all the same, so that the answer takes as
    /// long as for a wrong password.
    pub async fn verify_credentials(
        db: &Db,
        email: &str,
        password: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        let user = Self::find_by_email(&**db, email).await?;
        let password = password.to_string();
        match user {
            Some(user) => {
                let stored = user.password.clone();
                let matches = blocking(move || verify_password(&stored, &password)).await?;
                Ok(matches.then_some(user))
            }
            None => {
                blocking(move || hash_password(&password)).await?;
                Ok(None)
            }
        }
    }

    pub async fn verify_password(&self, password: &str) -> Result<bool, sqlx::Error> {
        let stored = self.password.clone();
        let password = password.to_string();
        blocking(move || verify_password(&stored, &password)).await
    }

    /// Retire every session issued to this user so far. Incremented in SQL
    /// so that concurrent calls never settle on the same version.
    pub async fn invalidate_sessions<'e, E>(&mut self, db: E) -> Result<(), sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        let version: i64 = sqlx::query_scalar(
            "update `users` set `session_version` = `session_version` + 1 where `id` = ? returning `session_version`",
        )
        .bind(self.id)
        .fetch_one(db)
        .await?;
        self.session_version = version;
        if let Some(stored) = self.stored.as_mut() {
            stored.session_version = version;
        }
        Ok(())
    }

    /// Set a new password and sign the account out everywhere. One
    /// transaction, so the password never changes without the revocation,
    /// nor the reverse.
    pub async fn change_password(
        &mut self,
        db: &Db,
        new_password: &str,
    ) -> Result<(), sqlx::Error> {
        let new_password = new_password.to_string();
        let hashed = blocking(move || hash_password(&new_password)).await?;

        let mut transaction = db.begin().await?;
        self.password = hashed;
        self.save(&mut *transaction).await?;
        self.invalidate_sessions(&mut *transaction).await?;
        sqlx::query("delete from `remember_me_tokens` where `tokenable_id` = ?")
            .bind(self.id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await
    }
}
