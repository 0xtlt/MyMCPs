//! The browser session as the place where the OAuth flow of an MCP keeps an
//! authorization between its start and its callback.

use mymcps_upstream::OauthSessionStore;
use serde_json::Value;

use crate::session::Session;

impl OauthSessionStore for Session {
    fn get(&self, key: &str) -> Option<Value> {
        Session::get(self, key)
    }

    fn put(&self, key: &str, value: Value) {
        Session::put(self, key, value);
    }

    fn forget(&self, key: &str) {
        Session::forget(self, key);
    }

    fn keys(&self) -> Vec<String> {
        Session::keys(self)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn lists_keys_oldest_first_and_keeps_the_place_of_a_replaced_one() {
        let session = Session::default();
        let store: &dyn OauthSessionStore = &session;
        store.put("mcp_oauth:a", json!({"mcpId": 1}));
        store.put("auth_web", json!(7));
        store.put("mcp_oauth:b", json!({"mcpId": 2}));
        store.put("mcp_oauth:a", json!({"mcpId": 3}));
        assert_eq!(store.keys(), ["mcp_oauth:a", "auth_web", "mcp_oauth:b"]);
        assert_eq!(store.get("mcp_oauth:a"), Some(json!({"mcpId": 3})));

        store.forget("mcp_oauth:a");
        assert_eq!(store.keys(), ["auth_web", "mcp_oauth:b"]);
        assert_eq!(store.get("mcp_oauth:a"), None);
    }
}
