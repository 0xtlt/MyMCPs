//! `tests/unit/vine_gateway_oauth_validators.spec.ts`

use std::any::Any;

use mymcps_gateway::validators::gateway_oauth::{
    AUTHORIZATION_CLIENT_ID, AUTHORIZATION_CODE_GRANT, AUTHORIZATION_REDIRECT_URI,
    AUTHORIZATION_RESPONSE_TYPE, AUTHORIZATION_STATE, AUTHORIZATION_STATE_LENGTH,
    CLIENT_AUTH_METHOD, CLIENT_GRANT_TYPES, CLIENT_NAME, CLIENT_REDIRECT_URIS,
    CLIENT_RESPONSE_TYPES, CONSENT_APPROVAL, GATEWAY_RESOURCE, GatewayResource, PKCE_CHALLENGE,
    PKCE_VERIFIER, POSTED_CLIENT_CREDENTIALS, REFRESH_SCOPE, REFRESH_TOKEN_GRANT, REQUESTED_SCOPE,
    REVOCATION_REQUEST, RegisteredRedirectUris, TOKEN_REQUEST,
};
use mymcps_vine::{FieldError, Validator};
use serde_json::{Value, json};

/// Values that stand where a string is expected without being one. The first
/// is `undefined`.
fn not_strings() -> Vec<Option<Value>> {
    let mut values = vec![None];
    values.extend(
        [
            json!(null),
            json!(0),
            json!(1),
            json!(true),
            json!(["a"]),
            json!(["a", "b"]),
            json!([]),
            json!({}),
            json!({ "a": "b" }),
        ]
        .map(Some),
    );
    values
}

fn strings<const N: usize>(values: [&str; N]) -> Vec<Option<Value>> {
    values.iter().map(|value| Some(json!(value))).collect()
}

/// The strings, then everything that is not one.
fn strings_and_not_strings<const N: usize>(values: [&str; N]) -> Vec<Option<Value>> {
    let mut all = strings(values);
    all.extend(not_strings());
    all
}

/// `{ ...base, [key]: value }`, where `None` leaves the key out.
fn with(base: &Value, key: &str, value: Option<Value>) -> Value {
    let mut object = base.clone();
    if let Some(object) = object.as_object_mut() {
        match value {
            Some(value) => object.insert(key.to_owned(), value),
            None => object.remove(key),
        };
    }
    object
}

/// The output of a value the validator must accept. `None` is `undefined`.
fn accepted(validator: &Validator, value: impl Into<Option<Value>>) -> Option<Value> {
    let value = value.into();
    match validator.validate_opt(&value) {
        Ok(output) => output,
        Err(error) => panic!("expected {value:?} to be accepted: {error:?}"),
    }
}

fn accepted_with(
    validator: &Validator,
    value: impl Into<Option<Value>>,
    meta: &dyn Any,
) -> Option<Value> {
    let value = value.into();
    match validator.validate_opt_with(&value, meta) {
        Ok(output) => output,
        Err(error) => panic!("expected {value:?} to be accepted: {error:?}"),
    }
}

/// The errors of a value the validator must refuse.
fn refused(validator: &Validator, value: impl Into<Option<Value>>) -> Vec<FieldError> {
    let value = value.into();
    match validator.validate_opt(&value) {
        Ok(output) => panic!("expected {value:?} to be refused, got {output:?}"),
        Err(error) => error.messages,
    }
}

fn refused_with(
    validator: &Validator,
    value: impl Into<Option<Value>>,
    meta: &dyn Any,
) -> Vec<FieldError> {
    let value = value.into();
    match validator.validate_opt_with(&value, meta) {
        Ok(output) => panic!("expected {value:?} to be refused, got {output:?}"),
        Err(error) => error.messages,
    }
}

// vine: gateway OAuth client registration

#[test]
fn accepts_https_loopback_http_and_the_approved_native_callback_as_redirect_uris() {
    let ten: Vec<String> = (0..10)
        .map(|index| format!("https://client.example/{index}"))
        .collect();
    let longest = format!(
        "https://client.example/{}",
        "a".repeat(2048 - "https://client.example/".len())
    );
    for uris in [
        json!(["https://client.example/callback"]),
        json!(["https://client.example:8443/callback?tenant=1"]),
        json!(["HTTPS://CLIENT.example/callback"]),
        json!(["http://localhost/callback"]),
        json!(["http://localhost:8080/callback"]),
        json!(["http://127.0.0.1:49152/callback"]),
        json!(["http://[::1]/callback"]),
        json!(["cursor://anysphere.cursor-mcp/oauth/callback"]),
        json!(["https://client.example/a", "http://127.0.0.1/b"]),
        json!(ten),
        json!([longest]),
    ] {
        assert_eq!(accepted(&CLIENT_REDIRECT_URIS, uris.clone()), Some(uris));
    }
}

#[test]
fn refuses_unsafe_malformed_oversized_and_miscounted_redirect_uris() {
    let eleven: Vec<String> = (0..11)
        .map(|index| format!("https://client.example/{index}"))
        .collect();
    let too_long = format!(
        "https://client.example/{}",
        "a".repeat(2049 - "https://client.example/".len())
    );
    for uris in [
        Some(json!([])),
        Some(json!(eleven)),
        Some(json!(["http://example.com/callback"])),
        Some(json!(["http://127.0.0.2/callback"])),
        Some(json!(["http://localhost./callback"])),
        Some(json!(["http://sub.localhost/callback"])),
        Some(json!(["https://client.example/callback#fragment"])),
        Some(json!(["https://user:password@client.example/callback"])),
        Some(json!(["https://user@client.example/callback"])),
        Some(json!(["cursor://anysphere.cursor-mcp/oauth/callback/"])),
        Some(json!(["cursor://anysphere.cursor-mcp/oauth/callback?x=1"])),
        Some(json!(["CURSOR://anysphere.cursor-mcp/oauth/callback"])),
        Some(json!(["cursor://attacker.example/oauth/callback"])),
        Some(json!(["vscode://vendor.extension/callback"])),
        Some(json!(["ftp://client.example/callback"])),
        Some(json!(["javascript:alert(1)"])),
        Some(json!(["client.example/callback"])),
        Some(json!(["/callback"])),
        Some(json!([""])),
        Some(json!([" "])),
        Some(json!([too_long])),
        Some(json!([
            "https://client.example/callback",
            "http://example.com/callback"
        ])),
        Some(json!([1])),
        Some(json!([null])),
        Some(json!([["https://client.example/callback"]])),
        Some(json!("https://client.example/callback")),
        Some(json!({ "0": "https://client.example/callback" })),
        None,
        Some(json!(null)),
    ] {
        refused(&CLIENT_REDIRECT_URIS, uris);
    }
}

#[test]
fn accepts_the_three_client_authentication_methods_and_defaults_to_client_secret_basic() {
    for method in ["none", "client_secret_post", "client_secret_basic"] {
        assert_eq!(
            accepted(&CLIENT_AUTH_METHOD, json!(method)),
            Some(json!(method))
        );
    }
    assert_eq!(
        accepted(&CLIENT_AUTH_METHOD, None),
        Some(json!("client_secret_basic"))
    );

    for method in [
        json!(""),
        json!(" "),
        json!(" none"),
        json!("none "),
        json!("NONE"),
        json!("private_key_jwt"),
        json!(1),
        json!(true),
        json!(["none"]),
    ] {
        refused(&CLIENT_AUTH_METHOD, method);
    }
}

#[test]
fn reduces_grant_types_to_their_distinct_values_and_requires_the_authorization_code() {
    assert_eq!(
        accepted(&CLIENT_GRANT_TYPES, None),
        Some(json!(["authorization_code", "refresh_token"]))
    );
    assert_eq!(
        accepted(&CLIENT_GRANT_TYPES, json!(["authorization_code"])),
        Some(json!(["authorization_code"]))
    );
    assert_eq!(
        accepted(
            &CLIENT_GRANT_TYPES,
            json!([
                "refresh_token",
                "authorization_code",
                "refresh_token",
                "authorization_code"
            ])
        ),
        Some(json!(["refresh_token", "authorization_code"]))
    );
    let mut repeated = vec!["authorization_code"; 5000];
    repeated.extend(vec!["refresh_token"; 5000]);
    assert_eq!(
        accepted(&CLIENT_GRANT_TYPES, json!(repeated)),
        Some(json!(["authorization_code", "refresh_token"]))
    );

    for grant_types in [
        json!([]),
        json!(["refresh_token"]),
        json!(["refresh_token", "refresh_token"]),
        json!(["authorization_code", "implicit"]),
        json!(["authorization_code", ""]),
        json!(["Authorization_code"]),
        json!(["client_credentials"]),
        json!([1]),
        json!([null]),
        json!("authorization_code"),
        json!({}),
    ] {
        refused(&CLIENT_GRANT_TYPES, grant_types);
    }
}

#[test]
fn bounds_type_lists_before_looking_at_their_members() {
    let unknown: Vec<String> = (0..5000).map(|index| format!("type-{index}")).collect();

    assert_eq!(refused(&CLIENT_GRANT_TYPES, json!(unknown)).len(), 1);
    assert_eq!(refused(&CLIENT_RESPONSE_TYPES, json!(unknown)).len(), 1);
}

#[test]
fn accepts_the_code_response_type_only_however_often_it_is_repeated() {
    assert_eq!(
        accepted(&CLIENT_RESPONSE_TYPES, None),
        Some(json!(["code"]))
    );
    assert_eq!(
        accepted(&CLIENT_RESPONSE_TYPES, json!(["code"])),
        Some(json!(["code"]))
    );
    assert_eq!(
        accepted(&CLIENT_RESPONSE_TYPES, json!(["code", "code"])),
        Some(json!(["code"]))
    );

    for response_types in [
        json!([]),
        json!(["token"]),
        json!(["code", "token"]),
        json!(["token", "code"]),
        json!(["Code"]),
        json!(["code "]),
        json!([""]),
        json!([1]),
        json!("code"),
        json!({}),
    ] {
        refused(&CLIENT_RESPONSE_TYPES, response_types);
    }
}

#[test]
fn accepts_a_scope_list_that_names_the_gateway_scope_and_nothing_else() {
    for scope in ["mcp:tools", " mcp:tools", "mcp:tools ", "   mcp:tools   "] {
        accepted(&REQUESTED_SCOPE, json!(scope));
    }
    // Left out, or sent in a shape that is not a scope string at all.
    for scope in not_strings() {
        accepted(&REQUESTED_SCOPE, scope);
    }

    for scope in [
        "",
        " ",
        "   ",
        "mcp:tools mcp:tools",
        "mcp:tools other",
        "other mcp:tools",
        "other",
        "MCP:TOOLS",
        "mcp:toolsx",
        "mcp: tools",
        "mcp:tools\t",
        "\tmcp:tools",
        "mcp:tools\n",
        "\u{a0}mcp:tools",
    ] {
        refused(&REQUESTED_SCOPE, json!(scope));
    }
}

#[test]
fn trims_a_client_name_and_limits_what_is_left_to_120_characters() {
    assert_eq!(
        accepted(&CLIENT_NAME, json!("  Claude Desktop  ")),
        Some(json!("Claude Desktop"))
    );
    assert_eq!(
        accepted(&CLIENT_NAME, json!("x".repeat(120))),
        Some(json!("x".repeat(120)))
    );
    assert_eq!(
        accepted(&CLIENT_NAME, json!(format!(" {} ", "x".repeat(120)))),
        Some(json!("x".repeat(120)))
    );
    // Nothing is left of these: the caller falls back to its default name.
    assert_eq!(accepted(&CLIENT_NAME, json!("")), Some(json!("")));
    assert_eq!(accepted(&CLIENT_NAME, json!(" \t\n")), Some(json!("")));
    assert_eq!(accepted(&CLIENT_NAME, None), None);

    for name in [
        json!("x".repeat(121)),
        json!(format!(" {} ", "x".repeat(121))),
        json!(1),
        json!(true),
        json!(["name"]),
        json!({}),
    ] {
        refused(&CLIENT_NAME, name);
    }
}

// vine: gateway OAuth authorization request

#[test]
fn takes_any_non_empty_string_as_a_client_id() {
    for client_id in ["mcp_client_abc", " ", " padded ", "0"] {
        assert_eq!(
            accepted(&AUTHORIZATION_CLIENT_ID, json!(client_id)),
            Some(json!(client_id))
        );
    }
    for client_id in strings_and_not_strings([""]) {
        refused(&AUTHORIZATION_CLIENT_ID, client_id);
    }
}

#[test]
fn matches_a_redirect_uri_exactly_apart_from_the_port_of_a_loopback_uri() {
    let loopback = RegisteredRedirectUris(vec!["http://127.0.0.1/callback?x=1".to_owned()]);
    for uri in [
        "http://127.0.0.1/callback?x=1",
        "http://127.0.0.1:49152/callback?x=1",
        "http://127.0.0.1:1/callback?x=1",
        "HTTP://127.0.0.1:49152/callback?x=1",
    ] {
        assert_eq!(
            accepted_with(&AUTHORIZATION_REDIRECT_URI, json!(uri), &loopback),
            Some(json!(uri))
        );
    }
    for uri in strings_and_not_strings([
        "http://127.0.0.1:49152/callback",
        "http://127.0.0.1:49152/callback?x=2",
        "http://127.0.0.1:49152/callback/?x=1",
        "http://127.0.0.1:49152/callback?x=1#fragment",
        "https://127.0.0.1:49152/callback?x=1",
        "http://localhost:49152/callback?x=1",
        "http://[::1]:49152/callback?x=1",
        "not a url",
        "",
        " ",
    ]) {
        refused_with(&AUTHORIZATION_REDIRECT_URI, uri, &loopback);
    }

    let remote = RegisteredRedirectUris(vec![
        "https://client.example/callback".to_owned(),
        "cursor://anysphere.cursor-mcp/oauth/callback".to_owned(),
    ]);
    accepted_with(
        &AUTHORIZATION_REDIRECT_URI,
        json!("https://client.example/callback"),
        &remote,
    );
    accepted_with(
        &AUTHORIZATION_REDIRECT_URI,
        json!("cursor://anysphere.cursor-mcp/oauth/callback"),
        &remote,
    );
    for uri in [
        "https://client.example:8443/callback",
        "https://client.example:443/callback",
        "https://CLIENT.example/callback",
        "https://client.example/callback/",
        "https://client.example/callback?x=1",
        "http://client.example/callback",
        "cursor://anysphere.cursor-mcp/oauth/callback/",
    ] {
        refused_with(&AUTHORIZATION_REDIRECT_URI, json!(uri), &remote);
    }
}

#[test]
fn never_matches_an_empty_redirect_uri_even_against_a_stored_empty_one() {
    refused_with(
        &AUTHORIZATION_REDIRECT_URI,
        json!(""),
        &RegisteredRedirectUris(vec![String::new()]),
    );
    refused_with(
        &AUTHORIZATION_REDIRECT_URI,
        json!("https://client.example/callback"),
        &RegisteredRedirectUris(Vec::new()),
    );
}

#[test]
fn reads_a_state_as_sent_and_judges_its_length_separately() {
    let long = "s".repeat(5000);
    for state in ["state-from-client", "", " ", long.as_str()] {
        assert_eq!(
            accepted(&AUTHORIZATION_STATE, json!(state)),
            Some(json!(state))
        );
    }
    assert_eq!(accepted(&AUTHORIZATION_STATE, None), None);
    assert_eq!(accepted(&AUTHORIZATION_STATE, json!(null)), None);
    for state in [json!(0), json!(1), json!(true), json!(["a"]), json!({})] {
        refused(&AUTHORIZATION_STATE, state);
    }

    for state in [
        json!(null),
        json!(""),
        json!(" "),
        json!("state"),
        json!("s".repeat(2048)),
    ] {
        accepted(&AUTHORIZATION_STATE_LENGTH, state);
    }
    refused(&AUTHORIZATION_STATE_LENGTH, json!("s".repeat(2049)));
}

#[test]
fn accepts_the_code_response_type_alone() {
    accepted(&AUTHORIZATION_RESPONSE_TYPE, json!("code"));
    for response_type in
        strings_and_not_strings(["", "token", "CODE", "code ", " code", "code token"])
    {
        refused(&AUTHORIZATION_RESPONSE_TYPE, response_type);
    }
}

#[test]
fn requires_a_base64url_s256_challenge_of_43_to_128_characters() {
    let pkce = |challenge: Option<Value>, method: Option<Value>| {
        let request = with(&json!({}), "code_challenge", challenge);
        with(&request, "code_challenge_method", method)
    };
    for challenge in ["a".repeat(43), "a".repeat(128), "A-z_09".repeat(8)] {
        assert_eq!(
            accepted(
                &PKCE_CHALLENGE,
                pkce(Some(json!(challenge)), Some(json!("S256")))
            ),
            Some(json!({ "code_challenge": challenge, "code_challenge_method": "S256" }))
        );
    }
    let mut challenges: Vec<Option<Value>> = [
        "a".repeat(42),
        "a".repeat(129),
        format!("{}.", "a".repeat(42)),
        format!("{}~", "a".repeat(42)),
        format!("{}=", "a".repeat(43)),
        format!("{}\n", "a".repeat(43)),
        format!("{} {}", "a".repeat(21), "a".repeat(21)),
        String::new(),
    ]
    .into_iter()
    .map(|challenge| Some(json!(challenge)))
    .collect();
    challenges.extend(not_strings());
    for challenge in challenges {
        refused(&PKCE_CHALLENGE, pkce(challenge, Some(json!("S256"))));
    }
    for method in strings_and_not_strings(["plain", "s256", "S256 ", " S256", ""]) {
        refused(&PKCE_CHALLENGE, pkce(Some(json!("a".repeat(43))), method));
    }
    refused(&PKCE_CHALLENGE, json!({}));
}

#[test]
fn accepts_the_gateway_resource_in_any_equivalent_spelling_of_its_url() {
    let gateway = GatewayResource("https://mcp.example.com/mcp".to_owned());
    for resource in [
        "https://mcp.example.com/mcp",
        "https://MCP.example.com/mcp",
        "HTTPS://mcp.example.com/mcp",
        "https://mcp.example.com:443/mcp",
        "https://mcp.example.com/a/../mcp",
        " https://mcp.example.com/mcp ",
    ] {
        assert_eq!(
            accepted_with(&GATEWAY_RESOURCE, json!(resource), &gateway),
            Some(json!(resource))
        );
    }
    for resource in strings_and_not_strings([
        "https://mcp.example.com/mcp/",
        "https://mcp.example.com/mcp?",
        "https://mcp.example.com/mcp?x=1",
        "https://mcp.example.com/mcp#",
        "https://mcp.example.com/mcp#fragment",
        "https://mcp.example.com/%6dcp",
        "https://mcp.example.com",
        "https://mcp.example.com:8443/mcp",
        "http://mcp.example.com/mcp",
        "https://user@mcp.example.com/mcp",
        "https://other.example.com/mcp",
        "/mcp",
        "not a url",
        "",
        " ",
    ]) {
        refused_with(&GATEWAY_RESOURCE, resource, &gateway);
    }
}

#[test]
fn takes_nothing_but_an_explicit_approval_as_consent() {
    accepted(&CONSENT_APPROVAL, json!("approve"));
    for decision in strings_and_not_strings(["deny", "", "APPROVE", " approve", "approve ", "true"])
    {
        refused(&CONSENT_APPROVAL, decision);
    }
    refused(&CONSENT_APPROVAL, json!(["approve"]));
}

// vine: gateway OAuth token and revocation requests

#[test]
fn reads_posted_client_credentials_and_ignores_a_secret_that_is_not_a_string() {
    assert_eq!(
        accepted(
            &POSTED_CLIENT_CREDENTIALS,
            json!({
                "client_id": "mcp_client_abc",
                "client_secret": "mcp_secret_abc",
                "grant_type": "refresh_token",
            })
        ),
        Some(json!({ "client_id": "mcp_client_abc", "client_secret": "mcp_secret_abc" }))
    );
    assert_eq!(
        accepted(
            &POSTED_CLIENT_CREDENTIALS,
            json!({ "client_id": "mcp_client_abc" })
        ),
        Some(json!({ "client_id": "mcp_client_abc" }))
    );
    // An empty secret is kept as sent; the caller treats it as no secret.
    assert_eq!(
        accepted(
            &POSTED_CLIENT_CREDENTIALS,
            json!({ "client_id": " ", "client_secret": "" })
        ),
        Some(json!({ "client_id": " ", "client_secret": "" }))
    );
    for secret in not_strings() {
        assert_eq!(
            accepted(
                &POSTED_CLIENT_CREDENTIALS,
                with(
                    &json!({ "client_id": "mcp_client_abc" }),
                    "client_secret",
                    secret
                )
            ),
            Some(json!({ "client_id": "mcp_client_abc" }))
        );
    }

    for client_id in strings_and_not_strings([""]) {
        refused(
            &POSTED_CLIENT_CREDENTIALS,
            with(
                &json!({ "client_secret": "mcp_secret_abc" }),
                "client_id",
                client_id,
            ),
        );
    }
}

#[test]
fn names_the_first_missing_parameter_of_a_token_or_revocation_request() {
    let first_missing =
        |validator: &Validator, input: Value| refused(validator, input)[0].field.clone();

    assert_eq!(first_missing(&TOKEN_REQUEST, json!({})), "grant_type");
    assert_eq!(first_missing(&REVOCATION_REQUEST, json!({})), "token");

    let code = json!({ "code": "c", "code_verifier": "v", "redirect_uri": "r", "resource": "x" });
    assert_eq!(
        accepted(&AUTHORIZATION_CODE_GRANT, code.clone()),
        Some(code.clone())
    );
    assert_eq!(first_missing(&AUTHORIZATION_CODE_GRANT, json!({})), "code");
    for field in ["code", "code_verifier", "redirect_uri", "resource"] {
        for value in strings_and_not_strings([""]) {
            assert_eq!(
                first_missing(&AUTHORIZATION_CODE_GRANT, with(&code, field, value)),
                field
            );
        }
    }
    let two_missing = with(&code, "code_verifier", Some(json!("")));
    assert_eq!(
        first_missing(
            &AUTHORIZATION_CODE_GRANT,
            with(&two_missing, "resource", Some(json!([])))
        ),
        "code_verifier"
    );

    let refresh = json!({ "refresh_token": "t", "resource": "x" });
    assert_eq!(
        accepted(&REFRESH_TOKEN_GRANT, refresh.clone()),
        Some(refresh)
    );
    assert_eq!(
        first_missing(&REFRESH_TOKEN_GRANT, json!({})),
        "refresh_token"
    );
    assert_eq!(
        first_missing(&REFRESH_TOKEN_GRANT, json!({ "refresh_token": "t" })),
        "resource"
    );
    assert_eq!(
        first_missing(
            &REFRESH_TOKEN_GRANT,
            json!({ "resource": "x", "refresh_token": ["t"] })
        ),
        "refresh_token"
    );
}

#[test]
fn accepts_blank_parameters_which_are_for_the_grant_itself_to_refuse() {
    let blank = json!({ "code": " ", "code_verifier": " ", "redirect_uri": " ", "resource": " " });
    assert_eq!(
        accepted(&AUTHORIZATION_CODE_GRANT, blank.clone()),
        Some(blank)
    );
    assert_eq!(
        accepted(&REVOCATION_REQUEST, json!({ "token": " " })),
        Some(json!({ "token": " " }))
    );
}

#[test]
fn keeps_the_scope_of_a_refresh_request_only_when_it_is_a_string() {
    let refresh = json!({ "refresh_token": "t", "resource": "x" });
    for scope in ["mcp:tools", "other", "", " "] {
        assert_eq!(
            accepted(
                &REFRESH_TOKEN_GRANT,
                with(&refresh, "scope", Some(json!(scope)))
            ),
            // The schema lists the scope before the resource.
            Some(json!({ "refresh_token": "t", "scope": scope, "resource": "x" }))
        );
    }
    for scope in not_strings() {
        assert_eq!(
            accepted(&REFRESH_TOKEN_GRANT, with(&refresh, "scope", scope)),
            Some(refresh.clone())
        );
    }
}

#[test]
fn lets_a_refresh_request_repeat_the_gateway_scope_and_ask_for_no_other() {
    accepted(&REFRESH_SCOPE, json!(null));
    accepted(&REFRESH_SCOPE, json!("mcp:tools"));
    for scope in [
        "",
        " ",
        " mcp:tools",
        "mcp:tools ",
        "mcp:tools mcp:tools",
        "other",
    ] {
        refused(&REFRESH_SCOPE, json!(scope));
    }
}

#[test]
fn requires_a_code_verifier_of_43_to_128_unreserved_characters() {
    for verifier in ["a".repeat(43), "a".repeat(128), "aZ09-._~".repeat(6)] {
        assert_eq!(
            accepted(&PKCE_VERIFIER, json!(verifier)),
            Some(json!(verifier))
        );
    }
    let mut verifiers: Vec<Option<Value>> = [
        "a".repeat(42),
        "a".repeat(129),
        format!("{}!", "a".repeat(43)),
        format!("{} ", "a".repeat(43)),
        format!(" {}", "a".repeat(43)),
        format!("{}\n", "a".repeat(43)),
        format!("{}é", "a".repeat(42)),
        String::new(),
    ]
    .into_iter()
    .map(|verifier| Some(json!(verifier)))
    .collect();
    verifiers.extend(not_strings());
    for verifier in verifiers {
        refused(&PKCE_VERIFIER, verifier);
    }
}
