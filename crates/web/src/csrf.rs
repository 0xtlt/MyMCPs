//! Cross-site request forgery protection for the pages: a secret in the
//! session, and a token derived from it that every state-changing request
//! must send back, in a `_csrf` field or an `X-CSRF-Token` header.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use mymcps_core::crypto::{constant_time_eq, random_base64url};
use sha2::{Digest, Sha256};

use crate::session::Session;

const SECRET_KEY: &str = "csrf-secret";

pub const CSRF_FIELD: &str = "_csrf";
pub const CSRF_HEADER: &str = "x-csrf-token";

/// What a request without a valid token is told.
pub const CSRF_MESSAGE: &str = "Invalid or expired CSRF token";

fn digest(salt: &str, secret: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(format!("{salt}.{secret}").as_bytes()))
}

/// A token for the forms of this response. Each call salts it anew, so the
/// secret never appears twice in the same form in a compressed page.
pub fn csrf_token(session: &Session) -> String {
    let secret = match session.get_as::<String>(SECRET_KEY) {
        Some(secret) if !secret.is_empty() => secret,
        _ => {
            let secret = random_base64url(18);
            session.put(SECRET_KEY, &secret);
            secret
        }
    };
    let salt = random_base64url(6);
    format!("{salt}.{}", digest(&salt, &secret))
}

pub fn verify_csrf_token(session: &Session, token: Option<&str>) -> bool {
    let (Some(secret), Some(token)) = (session.get_as::<String>(SECRET_KEY), token) else {
        return false;
    };
    // The salt and the digest are base64url, which has no dot.
    let Some((salt, hash)) = token.split_once('.') else {
        return false;
    };
    constant_time_eq(digest(salt, &secret).as_bytes(), hash.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_tokens_of_the_session() {
        let session = Session::default();
        assert!(
            !verify_csrf_token(&session, Some("anything")),
            "no secret yet"
        );

        let token = csrf_token(&session);
        let another = csrf_token(&session);
        assert_ne!(token, another);
        assert!(verify_csrf_token(&session, Some(&token)));
        assert!(verify_csrf_token(&session, Some(&another)));

        for forged in [
            None,
            Some(""),
            Some("no dot"),
            Some("salt."),
            Some(&token[..token.len() - 1]),
        ] {
            assert!(!verify_csrf_token(&session, forged), "{forged:?}");
        }
        let stranger = Session::default();
        csrf_token(&stranger);
        assert!(!verify_csrf_token(&stranger, Some(&token)));
    }
}
