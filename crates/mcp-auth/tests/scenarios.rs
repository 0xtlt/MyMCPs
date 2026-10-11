//! Differential tests against `@modelcontextprotocol/sdk`.
//!
//! `fixtures/scenarios.json` was recorded by running each helper of the real
//! SDK against a local HTTP server that played the resource server and the
//! authorization servers of a scenario. For every scenario it holds the
//! requests the SDK made, in order, the reply each one got, and what the
//! helper returned or threw.
//!
//! Here the same call is made with a fetch that replays those replies. The
//! port has to make the same requests in the same order (method, URL, headers
//! and body) and to end with the same value, or with an error of the same
//! class and message.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http::header::HeaderName;
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use mymcps_mcp_auth::{
    AuthorizationServerMetadata, AuthorizationServerMetadataOptions, Error,
    ExchangeAuthorizationOptions, HttpFetch, HttpFetchError, HttpRequest, HttpResponse,
    OAuthClientInformationMixed, OAuthMetadata, OAuthServerInfo, OpenIdProviderDiscoveryMetadata,
    ProtectedResourceMetadataOptions, RefreshAuthorizationOptions, RegisterClientOptions,
    ServerInfoOptions, StartAuthorizationOptions, discover_authorization_server_metadata,
    discover_oauth_protected_resource_metadata, discover_oauth_server_info, exchange_authorization,
    extract_www_authenticate_params, generate_challenge, refresh_authorization, register_client,
    start_authorization,
};
use serde_json::{Value, json};
use url::Url;

/// What the recorder threw for an address the application's guard refuses.
#[derive(Debug)]
struct RestrictedEndpointError {
    hostname: String,
}

impl fmt::Display for RestrictedEndpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OAuth endpoint host \"{}\" is a loopback, private or link-local address, which a remote MCP may not send MyMCPs to",
            self.hostname
        )
    }
}

impl std::error::Error for RestrictedEndpointError {}

/// A request the SDK did not make.
#[derive(Debug)]
struct NotRecorded;

impl fmt::Display for NotRecorded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the SDK made no such request")
    }
}

impl std::error::Error for NotRecorded {}

/// What `fetch` rejects with when the server closes the connection.
#[derive(Debug)]
struct FetchFailed;

impl fmt::Display for FetchFailed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("fetch failed")
    }
}

impl std::error::Error for FetchFailed {}

/// Gives each request the reply the SDK got for it, and notes every
/// difference between the request made and the one recorded.
struct Replay<'a> {
    recorded: &'a [Value],
    state: Mutex<(usize, Vec<String>)>,
}

impl<'a> Replay<'a> {
    fn new(recorded: &'a [Value]) -> Self {
        Self {
            recorded,
            state: Mutex::new((0, Vec::new())),
        }
    }

    fn answer(&self, request: &HttpRequest) -> Result<HttpResponse, HttpFetchError> {
        let mut state = self.state.lock().unwrap();
        let index = state.0;
        state.0 += 1;
        let Some(recorded) = self.recorded.get(index) else {
            state.1.push(format!(
                "request {index} was not made by the SDK: {} {}",
                request.method, request.url
            ));
            return Err(HttpFetchError::other(NotRecorded));
        };

        let headers: BTreeMap<String, String> = request
            .headers
            .iter()
            .map(|(name, value)| (name.to_string(), latin1(value.as_bytes())))
            .collect();
        let made = json!({
            "method": request.method.as_str(),
            "url": request.url.as_str(),
            "headers": headers,
            "body": String::from_utf8_lossy(&request.body),
        });
        let expected = json!({
            "method": recorded["method"],
            "url": recorded["url"],
            "headers": recorded["headers"],
            "body": recorded["body"],
        });
        if made != expected {
            state.1.push(format!(
                "request {index} differs\n   sdk: {expected}\n  rust: {made}"
            ));
        }

        let reply = &recorded["reply"];
        if reply["networkError"] == json!(true) {
            return Err(HttpFetchError::network(FetchFailed));
        }
        if reply["refused"] == json!(true) {
            return Err(HttpFetchError::other(RestrictedEndpointError {
                hostname: request.url.host_str().unwrap_or_default().to_owned(),
            }));
        }
        let body = match reply["bodyBase64"].as_str() {
            Some(encoded) => STANDARD.decode(encoded).unwrap(),
            None => reply["body"].as_str().unwrap().as_bytes().to_vec(),
        };
        let status = StatusCode::from_u16(reply["status"].as_u64().unwrap() as u16).unwrap();
        let mut response = HttpResponse::new(status, body);
        // A fetch that followed redirects of its own answers from another URL.
        response.url = reply["url"].as_str().map(|url| Url::parse(url).unwrap());
        for (name, value) in reply["headers"].as_object().unwrap() {
            let name = HeaderName::from_bytes(name.as_bytes()).unwrap();
            // A list is a header sent as several fields.
            let values = match value {
                Value::Array(values) => values.clone(),
                value => vec![value.clone()],
            };
            for value in values {
                let bytes: Vec<u8> = value.as_str().unwrap().chars().map(|c| c as u8).collect();
                response
                    .headers
                    .append(name.clone(), HeaderValue::from_bytes(&bytes).unwrap());
            }
        }
        Ok(response)
    }

    fn differences(self) -> Vec<String> {
        let (made, mut differences) = self.state.into_inner().unwrap();
        if made < self.recorded.len() {
            let missing = &self.recorded[made];
            differences.push(format!(
                "{} of the {} requests of the SDK were made, the next one was {} {}",
                made,
                self.recorded.len(),
                missing["method"],
                missing["url"]
            ));
        }
        differences
    }
}

impl HttpFetch for Replay<'_> {
    async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, HttpFetchError> {
        self.answer(&request)
    }
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

fn text<'a>(call: &'a Value, key: &str) -> Option<&'a str> {
    call[key].as_str()
}

fn url(call: &Value, key: &str) -> Option<Url> {
    text(call, key).map(|url| Url::parse(url).unwrap())
}

/// The metadata argument: what discovery returned for the document, or a
/// document the application wrote itself.
fn metadata(call: &Value) -> Option<AuthorizationServerMetadata> {
    let document = &call["metadata"];
    if document.is_null() {
        return None;
    }
    Some(match text(call, "metadataKind") {
        Some("oidc") => OpenIdProviderDiscoveryMetadata::parse(document)
            .unwrap()
            .into(),
        Some("manual") => {
            let member = |key: &str| document[key].as_str().map(str::to_owned);
            let list = |key: &str| {
                document[key].as_array().map(|items| {
                    items
                        .iter()
                        .map(|item| item.as_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                })
            };
            OAuthMetadata {
                issuer: member("issuer").unwrap(),
                authorization_endpoint: member("authorization_endpoint").unwrap(),
                token_endpoint: member("token_endpoint").unwrap(),
                registration_endpoint: member("registration_endpoint"),
                response_types_supported: list("response_types_supported").unwrap(),
                code_challenge_methods_supported: list("code_challenge_methods_supported"),
                token_endpoint_auth_methods_supported: list(
                    "token_endpoint_auth_methods_supported",
                ),
                ..OAuthMetadata::default()
            }
            .into()
        }
        _ => OAuthMetadata::parse(document).unwrap().into(),
    })
}

fn client_information(call: &Value) -> OAuthClientInformationMixed {
    let client = &call["clientInformation"];
    OAuthClientInformationMixed {
        client_id: client["client_id"].as_str().unwrap().to_owned(),
        client_secret: client["client_secret"].as_str().map(str::to_owned),
        token_endpoint_auth_method: client["token_endpoint_auth_method"]
            .as_str()
            .map(str::to_owned),
    }
}

fn header_value(call: &Value, key: &str) -> Option<HeaderValue> {
    text(call, key).map(|value| HeaderValue::from_str(value).unwrap())
}

fn server_info(info: &OAuthServerInfo) -> Value {
    let mut value = json!({ "authorizationServerUrl": info.authorization_server_url });
    if let Some(metadata) = &info.authorization_server_metadata {
        value["authorizationServerMetadata"] = json!(metadata);
    }
    if let Some(metadata) = &info.resource_metadata {
        value["resourceMetadata"] = json!(metadata);
    }
    value
}

/// Makes the call of a scenario. What it returns is written as the recorder
/// wrote what the SDK returned.
async fn run(call: &Value, fetch: &Replay<'_>) -> Result<Value, Error> {
    let authorization_server_url = url(call, "authorizationServerUrl");
    let metadata = metadata(call);
    let resource = url(call, "resource");
    match text(call, "fn").unwrap() {
        "discoverOAuthProtectedResourceMetadata" => {
            let metadata = discover_oauth_protected_resource_metadata(
                fetch,
                &url(call, "serverUrl").unwrap(),
                ProtectedResourceMetadataOptions {
                    protocol_version: header_value(call, "protocolVersion"),
                    resource_metadata_url: url(call, "resourceMetadataUrl"),
                },
            )
            .await?;
            Ok(json!(metadata))
        }
        "discoverAuthorizationServerMetadata" => {
            let metadata = discover_authorization_server_metadata(
                fetch,
                &authorization_server_url.unwrap(),
                AuthorizationServerMetadataOptions {
                    protocol_version: header_value(call, "protocolVersion"),
                },
            )
            .await?;
            Ok(json!(metadata))
        }
        "discoverOAuthServerInfo" => {
            let info = discover_oauth_server_info(
                fetch,
                &url(call, "serverUrl").unwrap(),
                ServerInfoOptions {
                    resource_metadata_url: url(call, "resourceMetadataUrl"),
                },
            )
            .await?;
            Ok(server_info(&info))
        }
        "discoverOAuthServerInfoAfter401" => {
            let server_url = url(call, "serverUrl").unwrap();
            let response = fetch
                .fetch(HttpRequest {
                    method: Method::GET,
                    url: server_url.clone(),
                    headers: HeaderMap::new(),
                    body: Bytes::new(),
                })
                .await
                .map_err(Error::Fetch)?;
            let params = extract_www_authenticate_params(&response.headers);
            let info = discover_oauth_server_info(
                fetch,
                &server_url,
                ServerInfoOptions {
                    resource_metadata_url: params.resource_metadata_url.clone(),
                },
            )
            .await?;
            Ok(json!({
                "status": response.status.as_u16(),
                "params": {
                    "resourceMetadataUrl": params.resource_metadata_url.map(String::from),
                    "scope": params.scope,
                    "error": params.error,
                },
                "info": server_info(&info),
            }))
        }
        "registerClient" => {
            let client = register_client(
                fetch,
                &authorization_server_url.unwrap(),
                RegisterClientOptions {
                    metadata: metadata.as_ref(),
                    client_metadata: &call["clientMetadata"],
                    scope: text(call, "scope"),
                },
            )
            .await?;
            Ok(json!(client))
        }
        "startAuthorization" => {
            let start = start_authorization(
                &authorization_server_url.unwrap(),
                StartAuthorizationOptions {
                    metadata: metadata.as_ref(),
                    client_information: &client_information(call),
                    redirect_url: text(call, "redirectUrl").unwrap(),
                    scope: text(call, "scope"),
                    state: text(call, "state"),
                    resource: resource.as_ref(),
                },
            )?;
            Ok(json!({
                "authorizationUrl": start.authorization_url.as_str(),
                "codeVerifier": start.code_verifier,
            }))
        }
        "exchangeAuthorization" => {
            let tokens = exchange_authorization(
                fetch,
                &authorization_server_url.unwrap(),
                ExchangeAuthorizationOptions {
                    metadata: metadata.as_ref(),
                    client_information: &client_information(call),
                    authorization_code: text(call, "authorizationCode").unwrap(),
                    code_verifier: text(call, "codeVerifier").unwrap(),
                    redirect_uri: text(call, "redirectUri").unwrap(),
                    resource: resource.as_ref(),
                },
            )
            .await?;
            Ok(json!(tokens))
        }
        "refreshAuthorization" => {
            let tokens = refresh_authorization(
                fetch,
                &authorization_server_url.unwrap(),
                RefreshAuthorizationOptions {
                    metadata: metadata.as_ref(),
                    client_information: &client_information(call),
                    refresh_token: text(call, "refreshToken").unwrap(),
                    resource: resource.as_ref(),
                },
            )
            .await?;
            Ok(json!(tokens))
        }
        other => panic!("unknown call {other}"),
    }
}

/// The verifier is random on both sides: what has to agree is the URL around
/// the challenge, and that each challenge is the one of its verifier.
fn without_challenge(started: &Value) -> Value {
    let verifier = started["codeVerifier"].as_str().unwrap();
    assert_eq!(verifier.len(), 43);
    assert!(
        verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
    );
    let challenge = generate_challenge(verifier);
    let url = started["authorizationUrl"].as_str().unwrap();
    assert_eq!(
        url.matches(&format!("code_challenge={challenge}")).count(),
        1,
        "{url} does not carry the challenge of {verifier}"
    );
    json!(url.replace(&challenge, "CHALLENGE"))
}

/// The error as the recorder described the one the SDK threw.
fn describe(error: &Error) -> Value {
    let mut described = json!({ "message": error.to_string() });
    match error {
        Error::Fetch(fetch_error) => {
            if fetch_error
                .downcast_ref::<RestrictedEndpointError>()
                .is_some()
            {
                assert!(!fetch_error.is_network());
                described["class"] = json!("RestrictedEndpointError");
                described["name"] = json!("RestrictedEndpointError");
                described["refused"] = json!(true);
            } else if fetch_error.downcast_ref::<FetchFailed>().is_some() {
                assert!(fetch_error.is_network());
                described["class"] = json!("TypeError");
                described["name"] = json!("TypeError");
                described["networkError"] = json!(true);
            }
        }
        Error::OAuth(oauth) => {
            described["class"] = json!(oauth.name());
            described["name"] = json!(oauth.name());
            described["errorCode"] = json!(oauth.error_code());
            described["errorUri"] = json!(oauth.error_uri());
            assert_eq!(oauth.message(), error.to_string());
        }
        other => {
            let name = other.name().unwrap();
            // `btoa` throws a DOMException, whose name says which.
            described["class"] = json!(if name == "InvalidCharacterError" {
                "DOMException"
            } else {
                name
            });
            described["name"] = json!(name);
        }
    }
    described
}

#[tokio::test]
async fn makes_the_requests_of_the_sdk_and_ends_as_it_does() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/scenarios.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() > 400);

    let mut failures = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let call = &case["call"];
        let replay = Replay::new(case["requests"].as_array().unwrap());
        let outcome = run(call, &replay).await;
        let mut differences = replay.differences();

        let starts_authorization = call["fn"] == json!("startAuthorization");
        match (&outcome, case.get("returned"), case.get("error")) {
            (Ok(returned), Some(expected), None) => {
                let (returned, expected) = if starts_authorization {
                    (without_challenge(returned), without_challenge(expected))
                } else {
                    (returned.clone(), expected.clone())
                };
                if returned != expected {
                    differences.push(format!("returned\n   sdk: {expected}\n  rust: {returned}"));
                }
            }
            (Err(error), None, Some(expected)) => {
                let described = describe(error);
                if &described != expected {
                    differences.push(format!("threw\n   sdk: {expected}\n  rust: {described}"));
                }
            }
            (Ok(returned), _, Some(expected)) => {
                differences.push(format!(
                    "the SDK threw {expected}\n  rust returned {returned}"
                ));
            }
            (Err(error), Some(expected), _) => {
                differences.push(format!(
                    "the SDK returned {expected}\n  rust failed with {error:?}"
                ));
            }
            _ => panic!("{name} records neither a value nor an error"),
        }

        if !differences.is_empty() {
            failures.push(format!("{name}\n  {}", differences.join("\n  ")));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} scenarios differ from the SDK:\n\n{}",
        failures.len(),
        cases.len(),
        failures[..failures.len().min(30)].join("\n\n")
    );
}

/// A fetch that refuses every address but those of one host, the way the
/// application's fetch refuses what its address guard does.
struct OnlyHost(&'static str);

impl HttpFetch for OnlyHost {
    async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, HttpFetchError> {
        let hostname = request.url.host_str().unwrap_or_default();
        if hostname != self.0 {
            return Err(HttpFetchError::other(RestrictedEndpointError {
                hostname: hostname.to_owned(),
            }));
        }
        let body = if request
            .url
            .path()
            .starts_with("/.well-known/oauth-protected-resource")
        {
            r#"{"resource":"https://mcp.example/mcp","authorization_servers":["http://169.254.169.254"]}"#
        } else {
            "not found"
        };
        let status = if body.starts_with('{') {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        Ok(HttpResponse::new(status, body))
    }
}

#[tokio::test]
async fn the_error_of_the_fetch_comes_back_as_the_fetch_returned_it() {
    let server_url = Url::parse("https://mcp.example/mcp").unwrap();
    let error = discover_oauth_server_info(
        &OnlyHost("mcp.example"),
        &server_url,
        ServerInfoOptions::default(),
    )
    .await
    .unwrap_err();

    let refused = error
        .downcast_fetch_error::<RestrictedEndpointError>()
        .expect("the error of the fetch");
    assert_eq!(refused.hostname, "169.254.169.254");
    assert_eq!(error.to_string(), refused.to_string());
    assert!(matches!(&error, Error::Fetch(HttpFetchError::Other(_))));
    assert_eq!(error.name(), None);
    // Nothing else is taken for it.
    assert!(error.downcast_fetch_error::<FetchFailed>().is_none());
    assert!(
        Error::InvalidUrl
            .downcast_fetch_error::<RestrictedEndpointError>()
            .is_none()
    );
}

#[test]
fn the_calls_can_be_awaited_on_any_thread() {
    fn assert_send<T: Send>(_: &T) {}
    fn assert_error<T: std::error::Error + Send + Sync + 'static>() {}
    assert_error::<Error>();
    assert_error::<HttpFetchError>();

    let fetch = OnlyHost("mcp.example");
    let url = Url::parse("https://mcp.example/mcp").unwrap();
    let client = OAuthClientInformationMixed {
        client_id: "client".to_owned(),
        ..OAuthClientInformationMixed::default()
    };
    let client_metadata = json!({});
    assert_send(&discover_oauth_server_info(
        &fetch,
        &url,
        ServerInfoOptions::default(),
    ));
    assert_send(&discover_authorization_server_metadata(
        &fetch,
        &url,
        AuthorizationServerMetadataOptions::default(),
    ));
    assert_send(&discover_oauth_protected_resource_metadata(
        &fetch,
        &url,
        ProtectedResourceMetadataOptions::default(),
    ));
    assert_send(&register_client(
        &fetch,
        &url,
        RegisterClientOptions {
            metadata: None,
            client_metadata: &client_metadata,
            scope: None,
        },
    ));
    assert_send(&exchange_authorization(
        &fetch,
        &url,
        ExchangeAuthorizationOptions {
            metadata: None,
            client_information: &client,
            authorization_code: "code",
            code_verifier: "verifier",
            redirect_uri: "https://app.example/callback",
            resource: None,
        },
    ));
    assert_send(&refresh_authorization(
        &fetch,
        &url,
        RefreshAuthorizationOptions {
            metadata: None,
            client_information: &client,
            refresh_token: "refresh",
            resource: None,
        },
    ));
}
