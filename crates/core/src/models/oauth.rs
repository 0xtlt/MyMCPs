use crate::time::Timestamp;

/// Registration stores at most two grant types and one response type. Older
/// rows may hold an arbitrarily long list, which is not worth parsing.
const MAX_TYPE_LIST_CHARS: usize = 256;

fn parse_string_list(value: &str, max_chars: usize) -> Vec<String> {
    if value.encode_utf16().count() > max_chars {
        return Vec::new();
    }
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| match item {
                serde_json::Value::String(text) => Some(text),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

model! {
    /// An MCP client registered with the gateway's OAuth server.
    table = "oauth_clients", created_at = true, updated_at = true;
    pub struct OauthClient {
        pub client_id: String,
        pub client_secret_hash: Option<String>,
        pub client_secret_prefix: Option<String>,
        pub client_secret_expires_at: Option<Timestamp>,
        pub client_name: String,
        pub redirect_uris: String,
        pub token_endpoint_auth_method: String,
        pub grant_types: String,
        pub response_types: String,
        pub scope: String,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
    }
}

impl OauthClient {
    pub fn redirect_uri_list(&self) -> Vec<String> {
        parse_string_list(&self.redirect_uris, usize::MAX)
    }

    pub fn grant_type_list(&self) -> Vec<String> {
        parse_string_list(&self.grant_types, MAX_TYPE_LIST_CHARS)
    }

    pub fn response_type_list(&self) -> Vec<String> {
        parse_string_list(&self.response_types, MAX_TYPE_LIST_CHARS)
    }
}

model! {
    table = "oauth_authorization_codes", created_at = true, updated_at = false;
    pub struct OauthAuthorizationCode {
        pub code_hash: String,
        pub oauth_client_id: i64,
        pub user_id: i64,
        pub redirect_uri: String,
        pub code_challenge: String,
        pub scopes: String,
        pub resource: String,
        pub expires_at: Timestamp,
        pub created_at: Timestamp,
    }
}
