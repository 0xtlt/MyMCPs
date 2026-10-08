//! The HTTP seam: this crate builds requests and reads responses, and the
//! caller sends them. It plays the part of the `fetchFn` option of the SDK.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode, header};
use url::Url;

use crate::js;

/// The error of whoever implements [`HttpFetch`].
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A request this crate asks the caller to send. Headers the caller's HTTP
/// client adds on its own (`Host`, `Content-Length`, `User-Agent`...) are not
/// in here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    /// Empty for a `GET`.
    pub body: Bytes,
}

/// A response, with its whole body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    /// The URL that answered (`response.url`), for a fetch that followed
    /// redirects on its own. `None` stands for the URL of the request.
    pub url: Option<Url>,
}

impl HttpResponse {
    pub fn new(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: body.into(),
            url: None,
        }
    }

    /// `response.ok`
    pub(crate) fn ok(&self) -> bool {
        self.status.is_success()
    }

    /// `response.headers.get(name)`: several fields of one name are joined,
    /// and a value is read one character per byte.
    pub(crate) fn header(&self, name: header::HeaderName) -> Option<String> {
        header_value(&self.headers, name)
    }
}

pub(crate) fn header_value(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    let mut values = headers.get_all(name).iter().map(|value| {
        let value = js::latin1_string(value.as_bytes());
        value.trim_matches(['\t', '\n', '\r', ' ']).to_owned()
    });
    let first = values.next()?;
    Some(values.fold(first, |joined, value| joined + ", " + &value))
}

/// Why the caller's fetch returned no response.
///
/// The SDK tells the two apart by the class of the JavaScript error. `fetch`
/// rejects with a `TypeError` when it could not get a response at all, and
/// discovery treats that as "nothing at this URL": the request is tried once
/// more without the headers the SDK adds, then the URL is given up for the
/// next one. Any other error stops the operation and is returned as it is.
///
/// Registration and token requests make no such difference: either error
/// ends them.
#[derive(Debug)]
pub enum HttpFetchError {
    /// The request could not be made or no response arrived: the name did
    /// not resolve, the connection was refused or reset, TLS failed, the
    /// server never answered. What `fetch` reports as `TypeError: fetch
    /// failed`.
    ///
    /// Not for a failure while the body of a response is read: with `fetch`
    /// that one surfaces when the SDK reads the body, where nothing is
    /// retried. Return [`HttpFetchError::Other`] for it.
    Network(BoxError),
    /// A refusal or a limit of the caller's own: an address it will not
    /// connect to, a redirect it will not follow, a response too large, a
    /// request it aborted. Never retried, never taken for an absent document.
    Other(BoxError),
}

impl HttpFetchError {
    pub fn network(error: impl Into<BoxError>) -> Self {
        Self::Network(error.into())
    }

    pub fn other(error: impl Into<BoxError>) -> Self {
        Self::Other(error.into())
    }

    pub fn is_network(&self) -> bool {
        matches!(self, Self::Network(_))
    }

    /// The error the fetch returned.
    pub fn get_ref(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        match self {
            Self::Network(error) | Self::Other(error) => error.as_ref(),
        }
    }

    /// The error the fetch returned, if it is a `T`.
    pub fn downcast_ref<T: std::error::Error + 'static>(&self) -> Option<&T> {
        self.get_ref().downcast_ref()
    }

    /// The error the fetch returned.
    pub fn into_inner(self) -> BoxError {
        match self {
            Self::Network(error) | Self::Other(error) => error,
        }
    }
}

impl fmt::Display for HttpFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.get_ref(), formatter)
    }
}

impl std::error::Error for HttpFetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.get_ref().source()
    }
}

/// Sends the requests of an OAuth flow.
///
/// An implementation may follow redirects itself or return them. A redirect
/// it returns is followed here as the SDK follows one: only within the origin
/// of the request (or from `http` to `https` on the same host), never more
/// than five times, and only with 307 or 308 for a request with a body. Any
/// other redirect is read as the response it is.
pub trait HttpFetch: Send + Sync {
    fn fetch(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpFetchError>> + Send;
}

impl<T: HttpFetch + ?Sized> HttpFetch for &T {
    fn fetch(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpFetchError>> + Send {
        (**self).fetch(request)
    }
}

impl<T: HttpFetch + ?Sized> HttpFetch for Arc<T> {
    fn fetch(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpFetchError>> + Send {
        (**self).fetch(request)
    }
}

const MAX_REDIRECTS: usize = 5;

fn is_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// `new URL(reference, base)`
pub(crate) fn resolve(base: &Url, reference: &str) -> Option<Url> {
    // The URL parser of JavaScript skips every slash and backslash a
    // reference to another host starts with. `Url::join` refuses three of
    // them where it accepts two, but reads the absolute form the same way.
    let special = matches!(base.scheme(), "http" | "https" | "ws" | "wss" | "ftp");
    if special {
        let cleaned: String = reference
            .trim_matches(|character| character <= ' ')
            .chars()
            .filter(|character| !matches!(character, '\t' | '\n' | '\r'))
            .collect();
        let slashes = cleaned
            .chars()
            .take_while(|character| matches!(character, '/' | '\\'))
            .count();
        if slashes > 2 {
            return Url::parse(&format!("{}:{cleaned}", base.scheme())).ok();
        }
    }
    base.join(reference).ok()
}

/// Where a redirect response points, resolved against the URL it answered.
fn redirect_target(response: &HttpResponse, request_url: &Url) -> Option<Url> {
    if !is_redirect(response.status) {
        return None;
    }
    let location = response.header(header::LOCATION)?;
    if location.is_empty() {
        return None;
    }
    resolve(request_url, &location)
}

/// Whether `to` has the scheme, host and port of `from`, or is its https form
/// with both on default ports.
fn is_within_origin(from: &Url, to: &Url) -> bool {
    if from.scheme() == to.scheme() && from.host_str() == to.host_str() && from.port() == to.port()
    {
        return true;
    }
    from.host_str() == to.host_str()
        && from.scheme() == "http"
        && from.port().is_none()
        && to.scheme() == "https"
        && to.port().is_none()
}

/// `fetchWithinOrigin` of the SDK: sends the request and follows a redirect
/// only within the origin of the request. Any other redirect response is
/// returned as it is.
///
/// The response comes back with its `url` set.
pub(crate) async fn fetch_within_origin<F: HttpFetch + ?Sized>(
    fetch: &F,
    mut request: HttpRequest,
) -> Result<HttpResponse, HttpFetchError> {
    let mut followed = 0;
    loop {
        let mut response = fetch.fetch(request.clone()).await?;
        let target = redirect_target(&response, &request.url);
        if response.url.is_none() {
            response.url = Some(request.url.clone());
        }

        // 301, 302 and 303 turn a request with a body into a GET, so only 307
        // and 308 are followed for those.
        let keeps_method =
            matches!(response.status.as_u16(), 307 | 308) || request.method == Method::GET;
        let Some(target) = target else {
            return Ok(response);
        };
        if followed == MAX_REDIRECTS || !keeps_method {
            return Ok(response);
        }

        let from = &request.url;
        let password = |url: &Url| url.password().unwrap_or_default().to_owned();
        let adds_userinfo = (!target.username().is_empty() || !password(&target).is_empty())
            && (target.username() != from.username() || password(&target) != password(from));
        if adds_userinfo || !is_within_origin(from, &target) {
            return Ok(response);
        }

        request.url = target;
        followed += 1;
    }
}

/// Describes a redirect that [`fetch_within_origin`] returned unfollowed,
/// naming its target without userinfo, query or fragment.
pub(crate) fn unfollowed_redirect(response: &HttpResponse) -> Option<String> {
    let from = response.url.as_ref()?;
    let mut target = redirect_target(response, from)?;
    // The setters refuse what the URL cannot hold, which then is not there to remove.
    let _ = target.set_username("");
    let _ = target.set_password(None);
    target.set_query(None);
    target.set_fragment(None);
    if target.scheme() == "http" && from.scheme() == "https" {
        let _ = target.set_scheme("https");
        return Some(format!(
            "Redirect to plain http not followed; try {target} instead"
        ));
    }
    Some(format!("Redirect to {target} not followed"))
}
