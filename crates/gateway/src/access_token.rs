//! Access tokens: the secrets AI clients authenticate to the gateway with,
//! made by hand on the Tokens page or granted through OAuth.
//!
//! Only the SHA-256 of a token is stored. An OAuth grant is one row that a
//! refresh rotates in place: its access token and its refresh token change,
//! and the refresh token it gave up is remembered, so that seeing it again
//! revokes the grant.

use chrono::Duration;
use mymcps_core::crypto::{random_base64url, sha256_hex};
use mymcps_core::models::{AccessToken, Mcp, ScopeMode, TokenSource};
use mymcps_core::{Db, Timestamp};
use sqlx::sqlite::SqliteExecutor;
use sqlx::{QueryBuilder, Sqlite, Transaction};

use crate::db::begin;
use crate::js::slice_utf16;

/// How long the access token of an OAuth grant works before it must be refreshed.
pub const OAUTH_ACCESS_TOKEN_TTL_SECONDS: i64 = 60 * 60;
const OAUTH_REFRESH_TOKEN_TTL_DAYS: i64 = 30;

/// How often the time a token was last used is written.
const LAST_USED_WRITE_INTERVAL_MINUTES: i64 = 5;

/// A token that was just made. `plaintext` is shown once and never stored.
#[derive(Debug, Clone)]
pub struct CreatedAccessToken {
    pub token: AccessToken,
    pub plaintext: String,
}

/// The tokens of an OAuth grant. A client registered without the
/// `refresh_token` grant gets no refresh token.
#[derive(Debug, Clone)]
pub struct CreatedOauthTokens {
    pub token: AccessToken,
    pub plaintext: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone)]
pub enum OauthGrantRotation {
    Rotated(Box<CreatedOauthTokens>),
    /// No grant of this client has this refresh token, or it has expired.
    Invalid,
    /// The refresh token was already exchanged. Its grant is now revoked.
    Reused,
}

#[derive(Debug, Clone, Copy)]
pub struct NewAccessToken<'a> {
    pub name: &'a str,
    pub scope_mode: ScopeMode,
    pub mcp_ids: &'a [i64],
    pub expires_at: Option<Timestamp>,
    pub created_by: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct NewOauthGrant<'a> {
    pub name: &'a str,
    /// The `id` of the row in `oauth_clients`.
    pub client_id: i64,
    pub client_supports_refresh: bool,
    pub scopes: &'a str,
    pub resource: &'a str,
    pub created_by: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct AccessTokenUpdate<'a> {
    pub name: &'a str,
    pub scope_mode: ScopeMode,
    pub mcp_ids: &'a [i64],
    pub expires_at: Option<Timestamp>,
}

pub fn generate_plaintext() -> String {
    format!("mcp_{}", random_base64url(32))
}

pub fn hash(plaintext: &str) -> String {
    sha256_hex(plaintext)
}

/// The start of a token, kept in clear so that its owner can recognise it.
pub fn prefix(plaintext: &str) -> &str {
    slice_utf16(plaintext, 12)
}

pub fn generate_refresh_token() -> String {
    format!("mcp_refresh_{}", random_base64url(32))
}

pub async fn create(
    db: &Db,
    params: NewAccessToken<'_>,
) -> Result<CreatedAccessToken, sqlx::Error> {
    let plaintext = generate_plaintext();
    let mut token = AccessToken {
        name: params.name.to_string(),
        token_prefix: prefix(&plaintext).to_string(),
        token_hash: hash(&plaintext),
        scope_mode: params.scope_mode,
        expires_at: params.expires_at,
        created_by: params.created_by,
        source: TokenSource::Manual,
        ..Default::default()
    };
    token.insert(&**db).await?;

    if params.scope_mode == ScopeMode::Selected && !params.mcp_ids.is_empty() {
        attach_mcps(&**db, token.id, params.mcp_ids).await?;
    }

    Ok(CreatedAccessToken { token, plaintext })
}

/// Store a new OAuth grant. Pass the transaction that consumes the
/// authorization code as the executor, so that both happen or neither does.
pub async fn create_oauth_grant<'e, E>(
    executor: E,
    params: NewOauthGrant<'_>,
) -> Result<CreatedOauthTokens, sqlx::Error>
where
    E: SqliteExecutor<'e>,
{
    let plaintext = generate_plaintext();
    let refresh_token = params.client_supports_refresh.then(generate_refresh_token);
    let mut token = AccessToken {
        name: params.name.to_string(),
        token_prefix: prefix(&plaintext).to_string(),
        token_hash: hash(&plaintext),
        scope_mode: ScopeMode::All,
        source: TokenSource::Oauth,
        expires_at: Some(Timestamp::now() + Duration::seconds(OAUTH_ACCESS_TOKEN_TTL_SECONDS)),
        created_by: params.created_by,
        oauth_client_id: Some(params.client_id),
        oauth_scopes: Some(params.scopes.to_string()),
        oauth_resource: Some(params.resource.to_string()),
        oauth_refresh_token_hash: refresh_token.as_deref().map(hash),
        oauth_refresh_token_prefix: refresh_token
            .as_deref()
            .map(|refresh_token| prefix(refresh_token).to_string()),
        oauth_refresh_expires_at: refresh_token
            .as_ref()
            .map(|_| Timestamp::now() + Duration::days(OAUTH_REFRESH_TOKEN_TTL_DAYS)),
        ..Default::default()
    };
    token.insert(executor).await?;

    Ok(CreatedOauthTokens {
        token,
        plaintext,
        refresh_token,
    })
}

/// Exchange a refresh token for new tokens. `client_id` is the `id` of the
/// row in `oauth_clients`.
///
/// Of two requests that present the same refresh token, one rotates the
/// grant. The other is a replay, whether it comes later or at the same
/// time, and revokes the grant.
pub async fn rotate_oauth_grant(
    db: &Db,
    refresh_token: &str,
    client_id: i64,
    resource: &str,
) -> Result<OauthGrantRotation, sqlx::Error> {
    let refresh_hash = hash(refresh_token);
    if revoke_oauth_grant_for_refresh_reuse(db, &refresh_hash, client_id, resource).await? {
        return Ok(OauthGrantRotation::Reused);
    }

    let current: Option<AccessToken> = sqlx::query_as(
        "select * from `access_tokens` where `oauth_refresh_token_hash` = ? and `oauth_client_id` = ? \
         and `oauth_resource` = ? and `source` = 'oauth' and `revoked_at` is null limit 1",
    )
    .bind(&refresh_hash)
    .bind(client_id)
    .bind(resource)
    .fetch_optional(&**db)
    .await?;

    let Some(current) = current.filter(|current| {
        current
            .oauth_refresh_expires_at
            .is_some_and(|expires_at| expires_at > Timestamp::now())
    }) else {
        return Ok(OauthGrantRotation::Invalid);
    };

    let plaintext = generate_plaintext();
    let next_refresh_token = generate_refresh_token();
    let next_expires_at = Timestamp::now() + Duration::seconds(OAUTH_ACCESS_TOKEN_TTL_SECONDS);

    let mut transaction = begin(db).await?;
    let updated = sqlx::query(
        "update `access_tokens` set `token_hash` = ?, `token_prefix` = ?, `expires_at` = ?, \
         `oauth_refresh_token_hash` = ?, `oauth_refresh_token_prefix` = ?, `last_used_at` = null \
         where `id` = ? and `oauth_refresh_token_hash` = ? and `revoked_at` is null",
    )
    .bind(hash(&plaintext))
    .bind(prefix(&plaintext))
    .bind(next_expires_at)
    .bind(hash(&next_refresh_token))
    .bind(prefix(&next_refresh_token))
    .bind(current.id)
    .bind(&refresh_hash)
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    let rotated = if updated == 1 {
        sqlx::query(
            "insert into `oauth_refresh_token_history` (`access_token_id`, `token_hash`, `invalidated_at`) values (?, ?, ?)",
        )
        .bind(current.id)
        .bind(&refresh_hash)
        .bind(Timestamp::now())
        .execute(&mut *transaction)
        .await?;

        let token: AccessToken = sqlx::query_as("select * from `access_tokens` where `id` = ?")
            .bind(current.id)
            .fetch_one(&mut *transaction)
            .await?;
        Some(CreatedOauthTokens {
            token,
            plaintext,
            refresh_token: Some(next_refresh_token),
        })
    } else {
        None
    };
    transaction.commit().await?;

    if let Some(tokens) = rotated {
        return Ok(OauthGrantRotation::Rotated(Box::new(tokens)));
    }
    if revoke_oauth_grant_for_refresh_reuse(db, &refresh_hash, client_id, resource).await? {
        return Ok(OauthGrantRotation::Reused);
    }
    Ok(OauthGrantRotation::Invalid)
}

async fn find_oauth_grant_by_refresh_history(
    db: &Db,
    refresh_hash: &str,
    client_id: i64,
    resource: Option<&str>,
) -> Result<Option<AccessToken>, sqlx::Error> {
    let access_token_id: Option<i64> = sqlx::query_scalar(
        "select `access_token_id` from `oauth_refresh_token_history` where `token_hash` = ? limit 1",
    )
    .bind(refresh_hash)
    .fetch_optional(&**db)
    .await?;
    let Some(access_token_id) = access_token_id else {
        return Ok(None);
    };

    match resource {
        Some(resource) => {
            sqlx::query_as(
                "select * from `access_tokens` where `id` = ? and `oauth_client_id` = ? \
                 and `source` = 'oauth' and `oauth_resource` = ? limit 1",
            )
            .bind(access_token_id)
            .bind(client_id)
            .bind(resource)
            .fetch_optional(&**db)
            .await
        }
        None => {
            sqlx::query_as(
                "select * from `access_tokens` where `id` = ? and `oauth_client_id` = ? \
                 and `source` = 'oauth' limit 1",
            )
            .bind(access_token_id)
            .bind(client_id)
            .fetch_optional(&**db)
            .await
        }
    }
}

async fn revoke_oauth_grant_for_refresh_reuse(
    db: &Db,
    refresh_hash: &str,
    client_id: i64,
    resource: &str,
) -> Result<bool, sqlx::Error> {
    let Some(mut token) =
        find_oauth_grant_by_refresh_history(db, refresh_hash, client_id, Some(resource)).await?
    else {
        return Ok(false);
    };
    if !token.is_revoked() {
        revoke(db, &mut token).await?;
    }
    Ok(true)
}

/// Revoke the grant a token of this client belongs to: its access token, its
/// refresh token, or a refresh token it gave up earlier. `client_id` is the
/// `id` of the row in `oauth_clients`. A token nobody holds changes nothing.
pub async fn revoke_oauth_token(
    db: &Db,
    client_id: i64,
    plaintext: &str,
) -> Result<(), sqlx::Error> {
    let token_hash = hash(plaintext);
    let mut token: Option<AccessToken> = sqlx::query_as(
        "select * from `access_tokens` where `oauth_client_id` = ? and `source` = 'oauth' \
         and (`token_hash` = ? or `oauth_refresh_token_hash` = ?) limit 1",
    )
    .bind(client_id)
    .bind(&token_hash)
    .bind(&token_hash)
    .fetch_optional(&**db)
    .await?;

    if token.is_none() {
        token = find_oauth_grant_by_refresh_history(db, &token_hash, client_id, None).await?;
    }

    if let Some(mut token) = token.filter(|token| !token.is_revoked()) {
        revoke(db, &mut token).await?;
    }
    Ok(())
}

pub async fn find_usable_by_plaintext(
    db: &Db,
    plaintext: &str,
) -> Result<Option<AccessToken>, sqlx::Error> {
    let token: Option<AccessToken> =
        sqlx::query_as("select * from `access_tokens` where `token_hash` = ? limit 1")
            .bind(hash(plaintext))
            .fetch_optional(&**db)
            .await?;
    Ok(token.filter(AccessToken::is_usable))
}

/// Note that the token was used, at most once every five minutes: the
/// gateway calls this on every request.
pub async fn touch_last_used(db: &Db, token: &mut AccessToken) -> Result<(), sqlx::Error> {
    let recently = Timestamp::now() - Duration::minutes(LAST_USED_WRITE_INTERVAL_MINUTES);
    if token
        .last_used_at
        .is_some_and(|last_used_at| last_used_at > recently)
    {
        return Ok(());
    }
    token.last_used_at = Some(Timestamp::now());
    token.save(&**db).await
}

/// Resolve MCPs allowed for this token.
/// scope_mode=all → every enabled MCP (including ones added after token creation).
pub async fn resolve_allowed_mcps(db: &Db, token: &AccessToken) -> Result<Vec<Mcp>, sqlx::Error> {
    if token.scope_mode == ScopeMode::All {
        return sqlx::query_as("select * from `mcps` where `enabled` = ? order by `name` asc")
            .bind(true)
            .fetch_all(&**db)
            .await;
    }

    sqlx::query_as(
        "select `mcps`.* from `mcps` \
         inner join `access_token_mcps` on `mcps`.`id` = `access_token_mcps`.`mcp_id` \
         where `access_token_mcps`.`access_token_id` = ? and `mcps`.`enabled` = ? \
         order by `mcps`.`name` asc",
    )
    .bind(token.id)
    .bind(true)
    .fetch_all(&**db)
    .await
}

/// Change the settings of a token. Its secret stays the same.
pub async fn update(
    db: &Db,
    token: &mut AccessToken,
    params: AccessTokenUpdate<'_>,
) -> Result<(), sqlx::Error> {
    let mut transaction = begin(db).await?;
    token.name = params.name.to_string();
    token.scope_mode = params.scope_mode;
    token.expires_at = params.expires_at;
    token.save(&mut *transaction).await?;

    let mcp_ids = match params.scope_mode {
        ScopeMode::Selected => params.mcp_ids,
        ScopeMode::All => &[],
    };
    sync_mcps(&mut transaction, token.id, mcp_ids).await?;
    transaction.commit().await
}

pub async fn revoke(db: &Db, token: &mut AccessToken) -> Result<(), sqlx::Error> {
    token.revoked_at = Some(Timestamp::now());
    token.save(&**db).await
}

async fn attach_mcps<'e, E>(executor: E, token_id: i64, mcp_ids: &[i64]) -> Result<(), sqlx::Error>
where
    E: SqliteExecutor<'e>,
{
    let now = Timestamp::now();
    let mut query = QueryBuilder::<Sqlite>::new(
        "insert into `access_token_mcps` (`access_token_id`, `mcp_id`, `created_at`, `updated_at`) ",
    );
    query.push_values(mcp_ids, |mut row, mcp_id| {
        row.push_bind(token_id)
            .push_bind(*mcp_id)
            .push_bind(now)
            .push_bind(now);
    });
    query.build().execute(executor).await?;
    Ok(())
}

/// Make the MCPs of a token the ones given: the rows of those it keeps are
/// left as they are.
async fn sync_mcps(
    transaction: &mut Transaction<'static, Sqlite>,
    token_id: i64,
    mcp_ids: &[i64],
) -> Result<(), sqlx::Error> {
    let mut wanted = mcp_ids.to_vec();
    wanted.sort_unstable();
    wanted.dedup();

    let mut removal =
        QueryBuilder::<Sqlite>::new("delete from `access_token_mcps` where `access_token_id` = ");
    removal.push_bind(token_id);

    if !wanted.is_empty() {
        let mut selection = QueryBuilder::<Sqlite>::new(
            "select `mcp_id` from `access_token_mcps` where `access_token_id` = ",
        );
        selection.push_bind(token_id).push(" and `mcp_id` in (");
        let mut list = selection.separated(", ");
        for mcp_id in &wanted {
            list.push_bind(*mcp_id);
        }
        selection.push(")");
        let existing: Vec<i64> = selection
            .build_query_scalar()
            .fetch_all(&mut **transaction)
            .await?;

        let added: Vec<i64> = wanted
            .iter()
            .copied()
            .filter(|mcp_id| !existing.contains(mcp_id))
            .collect();
        if !added.is_empty() {
            attach_mcps(&mut **transaction, token_id, &added).await?;
        }

        removal.push(" and `mcp_id` not in (");
        let mut list = removal.separated(", ");
        for mcp_id in &wanted {
            list.push_bind(*mcp_id);
        }
        removal.push(")");
    }

    removal.build().execute(&mut **transaction).await?;
    Ok(())
}
