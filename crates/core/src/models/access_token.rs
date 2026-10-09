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
    }
}

impl AccessToken {
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at.is_past())
    }

    pub fn is_usable(&self) -> bool {
        !self.is_revoked() && !self.is_expired()
    }

    pub fn is_oauth_session_active(&self) -> bool {
        if self.source != TokenSource::Oauth || self.is_revoked() {
            return false;
        }
        match self.oauth_refresh_expires_at.or(self.expires_at) {
            Some(connection_expires_at) => connection_expires_at.is_future(),
            None => true,
        }
    }

    pub fn is_active(&self) -> bool {
        if self.source == TokenSource::Oauth {
            self.is_oauth_session_active()
        } else {
            self.is_usable()
        }
    }
}
