//! Differential tests of the helpers that make no request.
//!
//! `fixtures/helpers.json` holds what the functions of the real SDK returned or
//! threw for a table of inputs: resource indicators, the choice of a client
//! authentication method, the locations of metadata, error bodies,
//! `WWW-Authenticate` headers and PKCE pairs.

use http::header::WWW_AUTHENTICATE;
use http::{HeaderMap, HeaderValue};
use mymcps_mcp_auth::{
    MetadataDocument, OAuthClientInformationMixed, build_discovery_urls, check_resource_allowed,
    extract_www_authenticate_params, generate_challenge, parse_error_body, pkce_challenge,
    resource_url_from_server_url, select_client_auth_method,
};
use serde_json::{Value, json};
use url::Url;

fn latin1_header(value: &str) -> HeaderValue {
    let bytes: Vec<u8> = value.chars().map(|character| character as u8).collect();
    HeaderValue::from_bytes(&bytes).unwrap()
}

/// What the port does for one row, written as the recorder wrote the outcome
/// of the SDK.
fn outcome(helper: &str, input: &Value) -> Value {
    let invalid_url = || json!({ "error": { "class": "TypeError", "name": "TypeError", "message": "Invalid URL" } });
    match helper {
        "checkResourceAllowed" => {
            let requested = Url::parse(input["requestedResource"].as_str().unwrap()).unwrap();
            let configured = Url::parse(input["configuredResource"].as_str().unwrap()).unwrap();
            json!({ "returned": check_resource_allowed(&requested, &configured) })
        }
        "resourceUrlFromServerUrl" => {
            let url = Url::parse(input.as_str().unwrap()).unwrap();
            json!({ "returned": resource_url_from_server_url(&url).as_str() })
        }
        "selectClientAuthMethod" => {
            let client = &input["clientInformation"];
            let client = OAuthClientInformationMixed {
                client_id: client["client_id"].as_str().unwrap().to_owned(),
                client_secret: client["client_secret"].as_str().map(str::to_owned),
                token_endpoint_auth_method: client["token_endpoint_auth_method"]
                    .as_str()
                    .map(str::to_owned),
            };
            let supported: Vec<String> =
                serde_json::from_value(input["supportedMethods"].clone()).unwrap();
            json!({ "returned": select_client_auth_method(&client, &supported).as_str() })
        }
        "buildDiscoveryUrls" => {
            let url = Url::parse(input.as_str().unwrap()).unwrap();
            match build_discovery_urls(&url) {
                Ok(locations) => {
                    let locations: Vec<Value> = locations
                        .iter()
                        .map(|location| {
                            json!({
                                "url": location.url.as_str(),
                                "type": match location.document {
                                    MetadataDocument::OAuth => "oauth",
                                    MetadataDocument::OpenId => "oidc",
                                },
                            })
                        })
                        .collect();
                    json!({ "returned": locations })
                }
                Err(error) => {
                    assert_eq!(error.to_string(), "Invalid URL");
                    assert_eq!(error.name(), Some("TypeError"));
                    invalid_url()
                }
            }
        }
        "parseErrorResponse" => {
            let error = parse_error_body(input.as_str().unwrap());
            json!({
                "returned": {
                    "class": error.name(),
                    "name": error.name(),
                    "message": error.message(),
                    "errorCode": error.error_code(),
                    "errorUri": error.error_uri(),
                },
            })
        }
        "extractWWWAuthenticateParams" => {
            let mut headers = HeaderMap::new();
            match input {
                Value::Null => {}
                Value::Array(fields) => {
                    for field in fields {
                        headers.append(WWW_AUTHENTICATE, latin1_header(field.as_str().unwrap()));
                    }
                }
                field => {
                    headers.insert(WWW_AUTHENTICATE, latin1_header(field.as_str().unwrap()));
                }
            }
            let params = extract_www_authenticate_params(&headers);
            json!({
                "returned": {
                    "resourceMetadataUrl": params.resource_metadata_url.map(String::from),
                    "scope": params.scope,
                    "error": params.error,
                },
            })
        }
        "generateChallenge" => json!({ "returned": generate_challenge(input.as_str().unwrap()) }),
        other => panic!("unknown helper {other}"),
    }
}

#[test]
fn the_helpers_without_requests_return_what_the_sdk_returns() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/helpers.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let mut compared = 0;
    let mut failures = Vec::new();
    for case in cases {
        let helper = case[0].as_str().unwrap();
        if helper == "pkceChallenge" {
            continue;
        }
        compared += 1;
        let actual = outcome(helper, &case[1]);
        if actual != case[2] {
            failures.push(format!(
                "{helper} {}\n   sdk: {}\n  rust: {actual}",
                case[1], case[2]
            ));
        }
    }
    assert!(compared > 800);
    assert!(
        failures.is_empty(),
        "{} of {compared} rows differ from the SDK:\n{}",
        failures.len(),
        failures[..failures.len().min(30)].join("\n")
    );
}

#[test]
fn pkce_pairs_are_made_like_those_of_pkce_challenge() {
    fn is_verifier(text: &str) -> bool {
        text.len() == 43
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
    }

    let fixture: Value = serde_json::from_str(include_str!("fixtures/helpers.json")).unwrap();
    let pairs: Vec<&Value> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case[0] == json!("pkceChallenge"))
        .map(|case| &case[2]["returned"])
        .collect();
    assert!(pairs.len() >= 20);
    // What the package generated is what this port would derive and accept.
    for pair in pairs {
        let verifier = pair["code_verifier"].as_str().unwrap();
        assert!(is_verifier(verifier));
        assert_eq!(
            generate_challenge(verifier),
            pair["code_challenge"].as_str().unwrap()
        );
    }

    let pair = pkce_challenge();
    assert!(is_verifier(&pair.code_verifier));
    assert_eq!(pair.code_challenge, generate_challenge(&pair.code_verifier));
}
