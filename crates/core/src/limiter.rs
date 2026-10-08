//! Fixed-window rate limits, counted in the `rate_limits` table the Node app
//! used, or in memory for checks that run on every gateway request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::db::Db;

/// How often a store drops the counters whose window has ended.
const SWEEP_INTERVAL_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimiterResponse {
    pub limit: u32,
    pub consumed: u32,
    pub remaining: u32,
    /// Seconds until the window ends, rounded up.
    pub available_in: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum LimiterError {
    #[error("Too many requests")]
    TooManyRequests(LimiterResponse),
    #[error(transparent)]
    Store(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
enum Store {
    Database(Db),
    Memory(Arc<Mutex<MemoryStore>>),
}

#[derive(Debug, Default)]
struct MemoryStore {
    counters: HashMap<String, (i64, i64)>,
    swept_at: i64,
}

/// An allowance of `requests` per `duration` for each key.
///
/// Two limiters on the same store with the same allowance share their
/// counters, as they did in the Node app: callers keep their keys apart with
/// a prefix (`login:`, `oauth-token:`...).
#[derive(Debug, Clone)]
pub struct Limiter {
    store: Store,
    requests: u32,
    duration: Duration,
    swept_at: Arc<Mutex<i64>>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

impl Limiter {
    /// Counted in SQLite: durable, and atomic across requests.
    pub fn database(db: &Db, requests: u32, duration: Duration) -> Self {
        Self {
            store: Store::Database(db.clone()),
            requests,
            duration,
            swept_at: Arc::default(),
        }
    }

    /// Counted in this process only.
    pub fn memory(requests: u32, duration: Duration) -> Self {
        Self {
            store: Store::Memory(Arc::default()),
            requests,
            duration,
            swept_at: Arc::default(),
        }
    }

    pub fn requests(&self) -> u32 {
        self.requests
    }

    fn storage_key(&self, key: &str) -> String {
        format!("mymcps:{}:{}:{key}", self.requests, self.duration.as_secs())
    }

    fn response(&self, consumed: i64, expire: i64, now: i64) -> LimiterResponse {
        let consumed = consumed.clamp(0, i64::from(u32::MAX)) as u32;
        LimiterResponse {
            limit: self.requests,
            consumed,
            remaining: self.requests.saturating_sub(consumed),
            available_in: ((expire - now).max(0) as u64).div_ceil(1000),
        }
    }

    /// Where the key stands, or `None` when it has no running window.
    pub async fn get(&self, key: &str) -> Result<Option<LimiterResponse>, sqlx::Error> {
        let storage_key = self.storage_key(key);
        let now = now_ms();
        let row: Option<(i64, i64)> =
            match &self.store {
                Store::Database(db) => sqlx::query_as(
                    "select `points`, `expire` from `rate_limits` where `key` = ? and `expire` > ?",
                )
                .bind(&storage_key)
                .bind(now)
                .fetch_optional(&**db)
                .await?,
                Store::Memory(store) => {
                    let store = store
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    store
                        .counters
                        .get(&storage_key)
                        .copied()
                        .filter(|(_, expire)| *expire > now)
                }
            };
        Ok(row.map(|(points, expire)| self.response(points, expire, now)))
    }

    async fn add(&self, key: &str, points: i64) -> Result<LimiterResponse, sqlx::Error> {
        let storage_key = self.storage_key(key);
        let now = now_ms();
        let window_end = now + self.duration.as_millis() as i64;

        let (consumed, expire) = match &self.store {
            Store::Database(db) => {
                self.sweep_database(db, now).await;
                sqlx::query_as::<_, (i64, i64)>(
                    "insert into `rate_limits` (`key`, `points`, `expire`) values (?1, max(?2, 0), ?3) \
                     on conflict (`key`) do update set \
                       `points` = case when `rate_limits`.`expire` <= ?4 then max(?2, 0) else max(`rate_limits`.`points` + ?2, 0) end, \
                       `expire` = case when `rate_limits`.`expire` <= ?4 then ?3 else `rate_limits`.`expire` end \
                     returning `points`, `expire`",
                )
                .bind(&storage_key)
                .bind(points)
                .bind(window_end)
                .bind(now)
                .fetch_one(&**db)
                .await?
            }
            Store::Memory(store) => {
                let mut store = store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if now - store.swept_at > SWEEP_INTERVAL_MS {
                    store.counters.retain(|_, (_, expire)| *expire > now);
                    store.swept_at = now;
                }
                let entry = store.counters.entry(storage_key).or_insert((0, window_end));
                if entry.1 <= now {
                    *entry = (0, window_end);
                }
                entry.0 = (entry.0 + points).max(0);
                *entry
            }
        };
        Ok(self.response(consumed, expire, now))
    }

    /// The table keeps expired counters unless asked to clear them, and some
    /// keys are chosen by callers (a client address, an account).
    async fn sweep_database(&self, db: &Db, now: i64) {
        {
            let mut swept_at = self
                .swept_at
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if now - *swept_at <= SWEEP_INTERVAL_MS {
                return;
            }
            *swept_at = now;
        }
        if let Err(error) = sqlx::query("delete from `rate_limits` where `expire` <= ?")
            .bind(now)
            .execute(&**db)
            .await
        {
            tracing::warn!(%error, "Expired rate limit counters could not be cleared");
        }
    }

    /// Count one request. Fails with [`LimiterError::TooManyRequests`] once
    /// the allowance of the window is used up; the request is counted anyway.
    pub async fn consume(&self, key: &str) -> Result<LimiterResponse, LimiterError> {
        let response = self.add(key, 1).await?;
        if response.consumed > self.requests {
            return Err(LimiterError::TooManyRequests(response));
        }
        Ok(response)
    }

    /// Count one request and say whether it may go ahead.
    pub async fn attempt(&self, key: &str) -> Result<bool, sqlx::Error> {
        if self
            .get(key)
            .await?
            .is_some_and(|response| response.consumed > response.limit)
        {
            return Ok(false);
        }
        match self.consume(key).await {
            Ok(_) => Ok(true),
            Err(LimiterError::TooManyRequests(_)) => Ok(false),
            Err(LimiterError::Store(error)) => Err(error),
        }
    }

    /// Count one request without ever refusing it.
    pub async fn increment(&self, key: &str) -> Result<LimiterResponse, sqlx::Error> {
        self.add(key, 1).await
    }

    /// Give one request back. Never goes below zero.
    pub async fn decrement(&self, key: &str) -> Result<(), sqlx::Error> {
        if self.get(key).await?.is_some() {
            self.add(key, -1).await?;
        }
        Ok(())
    }

    /// Forget the key: its next request starts a new window.
    pub async fn delete(&self, key: &str) -> Result<(), sqlx::Error> {
        let storage_key = self.storage_key(key);
        match &self.store {
            Store::Database(db) => {
                sqlx::query("delete from `rate_limits` where `key` = ?")
                    .bind(&storage_key)
                    .execute(&**db)
                    .await?;
            }
            Store::Memory(store) => {
                store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .counters
                    .remove(&storage_key);
            }
        }
        Ok(())
    }

    pub async fn remaining(&self, key: &str) -> Result<u32, sqlx::Error> {
        Ok(self
            .get(key)
            .await?
            .map_or(self.requests, |response| response.remaining))
    }

    /// Seconds until the key may send requests again. Zero while it still may.
    pub async fn available_in(&self, key: &str) -> Result<u64, sqlx::Error> {
        Ok(self
            .get(key)
            .await?
            .filter(|response| response.remaining == 0)
            .map_or(0, |response| response.available_in))
    }

    /// Forget every key of every limiter on this store. For tests.
    pub async fn clear(&self) -> Result<(), sqlx::Error> {
        match &self.store {
            Store::Database(db) => {
                sqlx::query("delete from `rate_limits`")
                    .execute(&**db)
                    .await?;
            }
            Store::Memory(store) => {
                store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .counters
                    .clear();
            }
        }
        Ok(())
    }
}
