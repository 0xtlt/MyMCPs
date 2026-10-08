//! The address a request came from, for rate limits and call logs.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRef, FromRequestParts};
use http::request::Parts;
use mymcps_core::Core;

/// The client address, always an IP address. A forwarded address is believed
/// only when every hop after it is a proxy `TRUST_PROXY` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIp(pub String);

pub fn client_ip(core: &Core, parts: &Parts) -> String {
    let socket = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip());
    let forwarded: Vec<&str> = parts
        .headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();
    let forwarded = (!forwarded.is_empty()).then(|| forwarded.join(", "));
    core.config
        .trust_proxy
        .client_ip(socket, forwarded.as_deref())
}

impl<S> FromRequestParts<S> for ClientIp
where
    S: Send + Sync,
    Arc<Core>: FromRef<S>,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let core = Arc::<Core>::from_ref(state);
        Ok(Self(client_ip(&core, parts)))
    }
}
