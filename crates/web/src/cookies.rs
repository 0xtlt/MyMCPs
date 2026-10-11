//! The cookies of one request: the ones the browser sent, and the ones the
//! response will set.

use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use http::HeaderValue;
use http::header::{COOKIE, SET_COOKIE};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// What `encodeURIComponent` leaves alone, as the Node app's cookies were written.
const COOKIE_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

#[derive(Debug, Default)]
struct Jar {
    received: Vec<(String, String)>,
    outgoing: Vec<String>,
}

/// Request-scoped cookie jar, placed in the request extensions by
/// [`cookies_layer`]. Cloning shares the jar.
#[derive(Debug, Clone, Default)]
pub struct Cookies {
    jar: Arc<Mutex<Jar>>,
    secure: bool,
}

/// How long the browser keeps a cookie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxAge {
    Seconds(i64),
    /// Delete the cookie now.
    Expired,
}

impl Cookies {
    pub fn parse(header: Option<&str>, secure: bool) -> Self {
        let received = header
            .unwrap_or("")
            .split(';')
            .filter_map(|pair| {
                let (name, value) = pair.trim().split_once('=')?;
                let value = value.trim().trim_matches('"');
                Some((
                    name.trim().to_string(),
                    percent_decode_str(value).decode_utf8_lossy().into_owned(),
                ))
            })
            .collect();
        Self {
            jar: Arc::new(Mutex::new(Jar {
                received,
                outgoing: Vec::new(),
            })),
            secure,
        }
    }

    fn jar(&self) -> std::sync::MutexGuard<'_, Jar> {
        self.jar
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The value the browser sent, percent-decoded.
    pub fn get(&self, name: &str) -> Option<String> {
        self.jar()
            .received
            .iter()
            .find(|(received, _)| received == name)
            .map(|(_, value)| value.clone())
    }

    /// Set a cookie for the whole site: HTTP only, `SameSite=Lax`, and
    /// `Secure` in production.
    pub fn set(&self, name: &str, value: &str, max_age: MaxAge) {
        let (value, seconds) = match max_age {
            MaxAge::Seconds(seconds) => (
                utf8_percent_encode(value, COOKIE_VALUE).to_string(),
                seconds,
            ),
            MaxAge::Expired => (String::new(), 0),
        };
        let mut cookie =
            format!("{name}={value}; Max-Age={seconds}; Path=/; HttpOnly; SameSite=Lax");
        if self.secure {
            cookie.push_str("; Secure");
        }

        let mut jar = self.jar();
        // The last value set for a name is the one the browser should keep.
        jar.outgoing
            .retain(|existing| !existing.starts_with(&format!("{name}=")));
        jar.outgoing.push(cookie);
    }

    pub fn clear(&self, name: &str) {
        self.set(name, "", MaxAge::Expired);
    }

    /// The value this request has set for the cookie so far, decoded. `None`
    /// when it set none, or deleted it.
    pub fn pending(&self, name: &str) -> Option<String> {
        let prefix = format!("{name}=");
        self.jar().outgoing.iter().find_map(|cookie| {
            let (pair, attributes) = cookie.split_once(';')?;
            let value = pair.strip_prefix(&prefix)?;
            (!attributes.contains("Max-Age=0"))
                .then(|| percent_decode_str(value).decode_utf8_lossy().into_owned())
        })
    }

    fn take_outgoing(&self) -> Vec<String> {
        std::mem::take(&mut self.jar().outgoing)
    }
}

/// Gives the request its [`Cookies`] and writes the ones set onto the response.
pub async fn cookies_layer(secure: bool, mut request: Request, next: Next) -> Response {
    let header = request
        .headers()
        .get(COOKIE)
        .and_then(|value| value.to_str().ok());
    let cookies = Cookies::parse(header, secure);
    request.extensions_mut().insert(cookies.clone());

    let mut response = next.run(request).await;
    for cookie in cookies.take_outgoing() {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(SET_COOKIE, value);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_cookies() {
        let cookies = Cookies::parse(
            Some("a=1; remember_web=e%3Agcm.v1%3Aabc; quoted=\"x y\""),
            true,
        );
        assert_eq!(cookies.get("a").as_deref(), Some("1"));
        assert_eq!(cookies.get("remember_web").as_deref(), Some("e:gcm.v1:abc"));
        assert_eq!(cookies.get("quoted").as_deref(), Some("x y"));
        assert_eq!(cookies.get("missing"), None);

        cookies.set("s", "first", MaxAge::Seconds(60));
        cookies.set("s", "e:gcm.v1:a b", MaxAge::Seconds(7200));
        cookies.clear("gone");
        assert_eq!(cookies.pending("s").as_deref(), Some("e:gcm.v1:a b"));
        assert_eq!(cookies.pending("gone"), None);
        assert_eq!(cookies.pending("never"), None);
        assert_eq!(
            cookies.take_outgoing(),
            [
                "s=e%3Agcm.v1%3Aa%20b; Max-Age=7200; Path=/; HttpOnly; SameSite=Lax; Secure",
                "gone=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax; Secure",
            ]
        );

        let plain = Cookies::parse(None, false);
        plain.set("s", "v", MaxAge::Seconds(1));
        assert_eq!(
            plain.take_outgoing(),
            ["s=v; Max-Age=1; Path=/; HttpOnly; SameSite=Lax"]
        );
    }
}
