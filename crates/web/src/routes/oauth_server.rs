//! MCP OAuth 2.1 discovery and authorization server endpoints: the two
//! metadata documents, client registration, the consent screen, and the
//! token and revocation endpoints. (`oauth_server_controller.ts`)
//!
//! The server itself is `mymcps_gateway::oauth`. These handlers read the
//! request, hold it to its rate limit, and write the answer.

use axum::Router;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use http::header::{
    ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, LOCATION, PRAGMA, WWW_AUTHENTICATE,
};
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use mymcps_core::limiter::Limiter;
use mymcps_core::models::User;
use mymcps_core::public_url::PublicUrlError;
use mymcps_gateway::oauth::{
    GatewayAuthorizationRequest, authorization_server_metadata, is_loopback_redirect_uri,
    oauth_redirect, oauth_token_response, protected_resource_metadata,
};
use mymcps_gateway::validators::gateway_oauth::CONSENT_APPROVAL;
use mymcps_gateway::{Error as OauthError, GatewayOauthError};
use serde_json::{Value, json};
use url::Url;

use crate::auth::Auth;
use crate::client_ip::ClientIp;
use crate::error::{AppError, is_fetch};
use crate::input::{Input, ParsedBody, parse_query};
use crate::respond::{navigate, page};
use crate::routes::FeatureRoutes;
use crate::session::Session;
use crate::state::AppState;
use crate::views::oauth::{Consent, authorize_page, leaving_page};
use crate::views::shell::PageContext;

/// Session key of the authorization request a person signs in to answer.
const RETURN_TO_KEY: &str = "oauthReturnTo";

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        protocol: Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(authorization_metadata),
            )
            .route(
                "/.well-known/oauth-protected-resource",
                get(protected_resource_metadata_document),
            )
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                get(protected_resource_metadata_document),
            )
            .route("/register", post(register))
            .route("/token", post(token))
            .route("/revoke", post(revoke)),
        // The consent screen is a page: it has a session and a CSRF token,
        // and looks at who is signed in itself.
        open: Router::new().route("/authorize", get(authorize).post(authorize)),
        ..Default::default()
    }
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [(CONTENT_TYPE, "application/json; charset=utf-8")],
        body.to_string(),
    )
        .into_response()
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Neither HTTP/1.1 caches nor the HTTP/1.0 ones that only know `Pragma`
/// keep the answer.
fn never_cached(response: Response) -> Response {
    let mut response = no_store(response);
    response
        .headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// What every endpoint answers while `APP_URL` cannot serve as the address
/// of the OAuth server.
fn unconfigured() -> Response {
    no_store(json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        &json!({
            "error": "temporarily_unavailable",
            "error_description": "OAuth requires APP_URL to be a public HTTPS origin",
        }),
    ))
}

/// The answer to a request the server refused, or failed to complete. A
/// failure is logged without anything a secret could hide in, and the
/// client is told nothing of its cause.
fn error_response(error: &OauthError) -> Response {
    if error.is_failure() {
        tracing::error!(error = %error.diagnostic(), "OAuth request failed");
    }
    let answer = error.to_oauth();
    let status = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = never_cached(json_response(status, &answer.body()));
    if let Some(challenge) = answer.www_authenticate() {
        response
            .headers_mut()
            .insert(WWW_AUTHENTICATE, HeaderValue::from_static(challenge));
    }
    response
}

fn refused(code: &'static str, message: &str, status: u16) -> Response {
    error_response(&GatewayOauthError::with_status(code, message, status).into())
}

/// Count one request of this client address. `false` once its allowance is used up.
async fn within_allowance(limiter: &Limiter, prefix: &str, ip: &str) -> Result<bool, AppError> {
    Ok(limiter.attempt(&format!("{prefix}:{ip}")).await?)
}

/// The `Authorization` header as the client sent it.
fn authorization_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
}

fn metadata_response(document: Result<Value, PublicUrlError>) -> Response {
    match document {
        Ok(document) => {
            let mut response = json_response(StatusCode::OK, &document);
            response.headers_mut().insert(
                CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            );
            response
        }
        Err(_) => unconfigured(),
    }
}

/// `GET /.well-known/oauth-authorization-server`
pub async fn authorization_metadata(State(state): State<AppState>) -> Response {
    metadata_response(authorization_server_metadata(&state.core.config))
}

/// `GET /.well-known/oauth-protected-resource` and `.../mcp`
pub async fn protected_resource_metadata_document(State(state): State<AppState>) -> Response {
    metadata_response(protected_resource_metadata(&state.core.config))
}

/// `POST /register`
pub async fn register(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    input: Input,
) -> Result<Response, AppError> {
    let oauth = &state.gateway.oauth;
    if oauth.ensure_configured().is_err() {
        return Ok(unconfigured());
    }
    if !within_allowance(&state.limiters.oauth_registration, "oauth-register", &ip).await? {
        return Ok(refused("too_many_requests", "Try again later", 429));
    }

    Ok(match oauth.register_client(&input.into_value()).await {
        Ok(client) => no_store(json_response(StatusCode::CREATED, &client)),
        Err(error) => error_response(&error),
    })
}

/// Host and port of a URL, as `URL.host` gives them.
fn host_of(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

/// Send the browser to the client's redirect URI, on another origin.
///
/// The consent form is answered here, and the policy of the pages
/// (`form-action 'self'`) stops a browser from following a redirect to
/// another site after a form. So the page's script is told where to go and
/// navigates there itself, and a form posted without it gets a page that
/// moves on. Only a request that is not a form is redirected.
fn send_to_client(
    method: &Method,
    headers: &HeaderMap,
    client_name: Option<&str>,
    location: &str,
) -> Response {
    let Ok(target) = HeaderValue::from_str(location) else {
        tracing::error!("OAuth request failed: the redirect URI cannot be written in a header");
        return refused(
            "server_error",
            "The OAuth request could not be completed",
            500,
        );
    };
    if is_fetch(headers) {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert("x-location", target);
        return response;
    }
    if method == Method::POST {
        let host = Url::parse(location)
            .map(|url| host_of(&url))
            .unwrap_or_default();
        let markup = leaving_page(client_name.unwrap_or(&host), &host, location);
        return Html(markup.into_string()).into_response();
    }
    // The location is complete: nothing of this request's query string is added to it.
    (StatusCode::FOUND, [(LOCATION, target)]).into_response()
}

struct AuthorizationCall<'a> {
    state: &'a AppState,
    method: &'a Method,
    headers: &'a HeaderMap,
    session: &'a Session,
    context: &'a PageContext,
    user: Option<&'a User>,
    input: &'a Value,
}

async fn answer_authorization(call: &AuthorizationCall<'_>) -> Result<Response, OauthError> {
    let oauth = &call.state.gateway.oauth;
    let request: GatewayAuthorizationRequest =
        oauth.parse_authorization_request(call.input).await?;

    let Some(user) = call.user else {
        call.session.put(RETURN_TO_KEY, request.return_path()?);
        // To the bare sign-in page: the request waits in the session.
        return Ok(navigate(call.headers, "/login"));
    };

    if call.method == Method::GET {
        let redirect_url = Url::parse(&request.redirect_uri)?;
        return Ok(page(authorize_page(
            call.context,
            &Consent {
                client_name: &request.client.client_name,
                redirect_host: &host_of(&redirect_url),
                is_loopback_redirect: is_loopback_redirect_uri(&request.redirect_uri),
                scope: &request.scopes,
                user_email: &user.email,
                client_id: &request.client.client_id,
                redirect_uri: &request.redirect_uri,
                state: request.state.as_deref(),
                code_challenge: &request.code_challenge,
                resource: &request.resource,
            },
        )));
    }

    let state = request.state.as_deref();
    let location = if CONSENT_APPROVAL
        .validate(call.input.get("decision"))
        .is_err()
    {
        oauth_redirect(
            &request.redirect_uri,
            &[
                ("error", Some("access_denied")),
                (
                    "error_description",
                    Some("The user denied the authorization request"),
                ),
                ("state", state),
            ],
        )?
    } else {
        let code = oauth.create_authorization_code(&request, user.id).await?;
        oauth_redirect(
            &request.redirect_uri,
            &[("code", Some(code.as_str())), ("state", state)],
        )?
    };
    Ok(send_to_client(
        call.method,
        call.headers,
        Some(&request.client.client_name),
        &location,
    ))
}

/// `GET /authorize` shows the consent screen, `POST /authorize` records the
/// decision.
#[allow(clippy::too_many_arguments)]
pub async fn authorize(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    session: Session,
    Auth { user }: Auth,
    ClientIp(ip): ClientIp,
    context: PageContext,
    ParsedBody(body): ParsedBody,
) -> Result<Response, AppError> {
    // The GET route also answers HEAD, which carries no CSRF token and no
    // body. Only GET shows the consent screen and only POST records a decision.
    if method != Method::GET && method != Method::POST {
        let mut response = refused("invalid_request", "Method not allowed", 405);
        response
            .headers_mut()
            .insert(ALLOW, HeaderValue::from_static("GET, POST"));
        return Ok(response);
    }

    if state.gateway.oauth.ensure_configured().is_err() {
        return Ok(unconfigured());
    }
    if !within_allowance(&state.limiters.oauth_authorization, "oauth-authorize", &ip).await? {
        return Ok(refused("temporarily_unavailable", "Try again later", 429));
    }

    // A decision is read from the CSRF-protected form body alone, never from
    // the query string.
    let input = Value::Object(if method == Method::GET {
        parse_query(uri.query().unwrap_or(""))
    } else {
        body
    });

    let call = AuthorizationCall {
        state: &state,
        method: &method,
        headers: &headers,
        session: &session,
        context: &context,
        user: user.as_ref(),
        input: &input,
    };
    let response = match answer_authorization(&call).await {
        Ok(response) => response,
        // Anyone can register a client, so a rejected request is only sent
        // back to a redirect URI on the user's own device. Any other target
        // would make this endpoint an open redirect; the error is shown here
        // instead.
        Err(OauthError::Oauth(GatewayOauthError {
            code,
            message,
            redirect_uri: Some(redirect_uri),
            state: client_state,
            ..
        })) if is_loopback_redirect_uri(&redirect_uri) => {
            let location = oauth_redirect(
                &redirect_uri,
                &[
                    ("error", Some(code)),
                    ("error_description", Some(message.as_str())),
                    ("state", client_state.as_deref()),
                ],
            );
            match location {
                Ok(location) => send_to_client(&method, &headers, None, &location),
                Err(error) => error_response(&OauthError::from(error)),
            }
        }
        Err(error) => error_response(&error),
    };
    Ok(no_store(response))
}

/// `POST /token`
pub async fn token(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    input: Input,
) -> Result<Response, AppError> {
    let oauth = &state.gateway.oauth;
    if oauth.ensure_configured().is_err() {
        return Ok(unconfigured());
    }
    if !within_allowance(&state.limiters.oauth_token, "oauth-token", &ip).await? {
        return Ok(refused("temporarily_unavailable", "Try again later", 429));
    }

    let issued = oauth
        .issue_tokens(
            authorization_header(&headers).as_deref(),
            &input.into_value(),
        )
        .await;
    Ok(match issued {
        Ok(created) => never_cached(json_response(
            StatusCode::OK,
            &oauth_token_response(&created),
        )),
        Err(error) => error_response(&error),
    })
}

/// `POST /revoke`
pub async fn revoke(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    input: Input,
) -> Result<Response, AppError> {
    let oauth = &state.gateway.oauth;
    if oauth.ensure_configured().is_err() {
        return Ok(unconfigured());
    }
    if !within_allowance(&state.limiters.oauth_token, "oauth-revoke", &ip).await? {
        return Ok(refused("temporarily_unavailable", "Try again later", 429));
    }

    let revoked = oauth
        .revoke_token(
            authorization_header(&headers).as_deref(),
            &input.into_value(),
        )
        .await;
    Ok(match revoked {
        Ok(()) => no_store(json_response(StatusCode::OK, &json!({}))),
        Err(error) => error_response(&error),
    })
}
