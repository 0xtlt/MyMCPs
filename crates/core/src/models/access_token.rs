use crate::time::Timestamp;

string_enum! {
    pub enum ScopeMode {
        /// Every enabled MCP, including ones added after the token was made.
        #[default]
        All => "all",
        Selected => "selected",
    }
}

string_enum! {
    pub enum TokenSource {
        #[default]
        Manual => "manual",
        Oauth => "oauth",
    }
}

model! {
    table = "access_tokens", created_at = true, updated_at = true;
    pub struct AccessToken {
        pub name: String,
        pub token_prefix: String,
        pub token_hash: String,
        pub scope_mode: ScopeMode,
        pub expires_at: Option<Timestamp>,
        pub revoked_at: Option<Timestamp>,
        pub last_used_at: Option<Timestamp>,
        pub created_by: i64,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
        pub source: TokenSource,
        pub oauth_client_id: Option<i64>,
        pub oauth_scopes: Option<String>,
        pub oauth_resource: Option<String>,
        pub oauth_refresh_token_hash: Option<String>,
        pub oauth_refresh_token_prefix: Option<String>,
        pub oauth_refresh_expires_at: Option<Timestamp>,
        /// The team whose MCPs the token reaches.
        pub team_id: i64,
    }
}

/// Where a token stands. A manual token works until it expires; an OAuth
/// connection lives on while its access token works or it can still be
/// refreshed. Every page and query that tells an active token from another
/// one goes by this, and [`AccessToken::push_active_condition`] says the
/// same in SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenStatus {
    Active,
    Expired,
    Revoked,
}

impl TokenStatus {
    pub fn is_active(self) -> bool {
        self == Self::Active
    }
}

impl AccessToken {
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Whether the access token itself has expired: an OAuth connection may
    /// still be refreshed after that, see [`AccessToken::status`].
    pub fn is_expired(&self) -> bool {
        self.is_expired_at(Timestamp::now())
    }

    fn is_expired_at(&self, now: Timestamp) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    /// Whether this very secret authenticates a request now.
    pub fn is_usable(&self) -> bool {
        !self.is_revoked() && !self.is_expired()
    }

    /// When the connection of the token ends, `None` for never: the expiry of
    /// a manual token; for an OAuth connection the later of its access token
    /// and its refresh token, since it works until both have expired. A
    /// client that cannot refresh has no refresh token, and its connection
    /// ends with its access token.
    pub fn connection_expires_at(&self) -> Option<Timestamp> {
        if self.source != TokenSource::Oauth {
            return self.expires_at;
        }
        match (self.expires_at, self.oauth_refresh_expires_at) {
            (Some(access), Some(refresh)) => Some(access.max(refresh)),
            (access, refresh) => access.or(refresh),
        }
    }

    /// Where the token stands at `now`. A connection ends at the instant it
    /// expires: it is active while that instant is in the future.
    pub fn status_at(&self, now: Timestamp) -> TokenStatus {
        if self.is_revoked() {
            TokenStatus::Revoked
        } else if self
            .connection_expires_at()
            .is_some_and(|expires_at| expires_at <= now)
        {
            TokenStatus::Expired
        } else {
            TokenStatus::Active
        }
    }

    pub fn status(&self) -> TokenStatus {
        self.status_at(Timestamp::now())
    }

    /// Whether the token, or the OAuth connection it belongs to, still works.
    pub fn is_active(&self) -> bool {
        self.status().is_active()
    }

    /// [`AccessToken::status_at`] is [`TokenStatus::Active`], as a condition on
    /// the row of `access_tokens` named `table` (the table itself or an
    /// alias). Every term is true or false, never null, so that `not (...)`
    /// selects exactly the other rows.
    pub fn push_active_condition(
        query: &mut sqlx::QueryBuilder<sqlx::Sqlite>,
        table: &str,
        now: Timestamp,
    ) {
        let column = |name: &str| format!("`{table}`.`{name}`");
        let (revoked, expires, refresh, source) = (
            column("revoked_at"),
            column("expires_at"),
            column("oauth_refresh_expires_at"),
            column("source"),
        );
        query
            .push(format!(
                "({revoked} is null and (({expires} is not null and {expires} > "
            ))
            .push_bind(now)
            .push(format!(
                ") or ({source} = 'oauth' and {refresh} is not null and {refresh} > "
            ))
            .push_bind(now)
            .push(format!(
                ") or ({expires} is null and ({source} <> 'oauth' or {refresh} is null))))"
            ));
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn manual(expires_at: Option<Timestamp>, revoked_at: Option<Timestamp>) -> AccessToken {
        AccessToken {
            expires_at,
            revoked_at,
            ..Default::default()
        }
    }

    fn oauth(expires_at: Option<Timestamp>, refresh: Option<Timestamp>) -> AccessToken {
        AccessToken {
            source: TokenSource::Oauth,
            expires_at,
            oauth_refresh_expires_at: refresh,
            ..Default::default()
        }
    }

    #[test]
    fn a_manual_token_is_active_until_it_expires_or_is_revoked() {
        let now = Timestamp::now();
        let hour = Duration::hours(1);
        assert_eq!(manual(None, None).status_at(now), TokenStatus::Active);
        assert_eq!(
            manual(Some(now + hour), None).status_at(now),
            TokenStatus::Active
        );
        assert_eq!(
            manual(Some(now - hour), None).status_at(now),
            TokenStatus::Expired
        );
        // The instant it expires, it no longer works.
        assert_eq!(manual(Some(now), None).status_at(now), TokenStatus::Expired);
        assert_eq!(
            manual(Some(now + hour), Some(now)).status_at(now),
            TokenStatus::Revoked
        );
        assert_eq!(
            manual(Some(now + hour), None).connection_expires_at(),
            Some(now + hour)
        );
    }

    #[test]
    fn an_oauth_connection_lives_while_it_can_be_refreshed() {
        let now = Timestamp::now();
        let hour = Duration::hours(1);
        let refreshable = oauth(Some(now - hour), Some(now + hour));
        assert!(
            !refreshable.is_usable(),
            "the access token itself has expired"
        );
        assert_eq!(refreshable.status_at(now), TokenStatus::Active);
        assert_eq!(refreshable.connection_expires_at(), Some(now + hour));

        let ended = oauth(Some(now - hour), Some(now - hour));
        assert_eq!(ended.status_at(now), TokenStatus::Expired);

        // The last access token can outlive the refresh token it came with.
        let last = oauth(Some(now + hour), Some(now - hour));
        assert_eq!(last.status_at(now), TokenStatus::Active);
        assert_eq!(last.connection_expires_at(), Some(now + hour));

        let revoked = AccessToken {
            revoked_at: Some(now),
            ..refreshable
        };
        assert_eq!(revoked.status_at(now), TokenStatus::Revoked);
    }

    #[test]
    fn an_oauth_connection_without_refresh_ends_with_its_access_token() {
        let now = Timestamp::now();
        let hour = Duration::hours(1);
        let working = oauth(Some(now + hour), None);
        assert_eq!(working.connection_expires_at(), Some(now + hour));
        assert_eq!(working.status_at(now), TokenStatus::Active);

        let ended = oauth(Some(now - hour), None);
        assert_eq!(ended.connection_expires_at(), Some(now - hour));
        assert_eq!(ended.status_at(now), TokenStatus::Expired);
    }
}
