//! Vine schemas for the gateway's own OAuth 2.1 authorization server. Each
//! validator stands for one decision of the protocol: the code that runs it
//! answers a failure with the one OAuth error that decision has.

use std::collections::HashSet;
use std::sync::LazyLock;

use mymcps_vine as vine;
use serde_json::{Value, json};
use url::Url;
use vine::{Rule, Validator, Vine, VineString};

use crate::oauth::constants::{GATEWAY_OAUTH_SCOPE, has_loopback_host};

/// The app converts blank strings to null before validating, which suits
/// HTML forms. An OAuth parameter is judged as the client sent it: `scope=`
/// names no scope and a blank `client_id` names an unknown client, neither
/// was left out. The validators below are therefore created by an instance
/// that leaves values as they are.
static VERBATIM: LazyLock<Vine> = LazyLock::new(Vine::new);

const NATIVE_APP_REDIRECT_URIS: [&str; 1] = ["cursor://anysphere.cursor-mcp/oauth/callback"];
const AUTH_METHODS: [&str; 3] = ["none", "client_secret_post", "client_secret_basic"];
const GRANT_TYPES: [&str; 2] = ["authorization_code", "refresh_token"];

/// The redirect URIs a client registered: what
/// [`AUTHORIZATION_REDIRECT_URI`] is validated with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredRedirectUris(pub Vec<String>);

/// The URL of the gateway: what [`GATEWAY_RESOURCE`] is validated with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayResource(pub String);

fn static_regex(source: &str) -> vine::JsRegex {
    vine::js::regex(source, "").expect("a valid static pattern")
}

/// Query strings and form bodies can carry a parameter several times or as a
/// nested value. An optional parameter sent in such a shape has always been
/// read as not sent, rather than refused.
fn string_or_absent(value: Option<Value>) -> Option<Value> {
    value.filter(Value::is_string)
}

/// RFC 6749, section 3.3: scopes travel as one space-delimited string.
fn scope_list(value: Option<Value>) -> Option<Value> {
    match value {
        Some(Value::String(scope)) => Some(
            scope
                .split(' ')
                .filter(|name| !name.is_empty())
                .map(Value::from)
                .collect(),
        ),
        _ => None,
    }
}

/// What tells two members of a list apart, for the values that can be equal.
#[derive(PartialEq, Eq, Hash)]
enum Member<'a> {
    Null,
    Bool(bool),
    Number(u64),
    String(&'a str),
}

/// A value repeated in a list is dropped, not refused.
fn distinct_values(value: Value) -> Value {
    let Value::Array(items) = value else {
        return value;
    };
    let mut seen = HashSet::new();
    let distinct = items
        .iter()
        .filter(|item| {
            let member = match item {
                Value::Null => Member::Null,
                Value::Bool(flag) => Member::Bool(*flag),
                // Zero has one sign here, as it has for a JavaScript `Set`.
                Value::Number(number) => {
                    Member::Number((number.as_f64().unwrap_or_default() + 0.0).to_bits())
                }
                Value::String(text) => Member::String(text),
                // Two lists or two objects are never the same one.
                Value::Array(_) | Value::Object(_) => return true,
            };
            seen.insert(member)
        })
        .cloned()
        .collect();
    Value::Array(distinct)
}

/// A parameter the request cannot do without: any string but the empty one.
fn required_parameter() -> VineString {
    vine::string().min_length(1)
}

/// Where a client may ask to be sent back: HTTPS anywhere, plain HTTP on the
/// user's own device, or a native-app callback approved by name.
fn redirect_target() -> Rule {
    vine::rule(|value, field| {
        let Some(value) = value.as_str() else {
            return;
        };
        // Cursor historically uses this private-use callback when its localhost
        // listener is unavailable. Keep the exception exact so arbitrary custom
        // schemes cannot be registered as OAuth redirect targets.
        if NATIVE_APP_REDIRECT_URIS.contains(&value) {
            return;
        }
        let allowed = Url::parse(value).is_ok_and(|url| {
            let is_secure =
                url.scheme() == "https" || (url.scheme() == "http" && has_loopback_host(&url));
            is_secure
                && url.fragment().is_none_or(str::is_empty)
                && url.username().is_empty()
                && url.password().is_none_or(str::is_empty)
        });
        if !allowed {
            field.report(
                "The {{ field }} field must be an allowed redirect URI",
                "redirectTarget",
            );
        }
    })
}

/// RFC 8252 allows native loopback clients to choose their callback port at runtime.
fn redirect_uri_matches(requested: &str, registered: &str) -> bool {
    if requested == registered {
        return true;
    }

    let (Ok(request_url), Ok(registered_url)) = (Url::parse(requested), Url::parse(registered))
    else {
        return false;
    };
    if !has_loopback_host(&request_url) || !has_loopback_host(&registered_url) {
        return false;
    }

    // An empty query or fragment reads the same as none.
    let query = |url: &Url| {
        url.query()
            .filter(|query| !query.is_empty())
            .map(str::to_owned)
    };
    let fragment = |url: &Url| {
        url.fragment()
            .filter(|fragment| !fragment.is_empty())
            .map(str::to_owned)
    };
    request_url.scheme() == registered_url.scheme()
        && request_url.host_str() == registered_url.host_str()
        && request_url.path() == registered_url.path()
        && query(&request_url) == query(&registered_url)
        && fragment(&request_url) == fragment(&registered_url)
}

/// An authorization request may only name a redirect URI its client registered.
fn registered_redirect_uri() -> Rule {
    vine::rule(|value, field| {
        let Some(value) = value.as_str() else {
            return;
        };
        let is_registered = field
            .meta::<RegisteredRedirectUris>()
            .is_some_and(|registered| {
                registered
                    .0
                    .iter()
                    .any(|uri| redirect_uri_matches(value, uri))
            });
        if !is_registered {
            field.report(
                "The {{ field }} field must be a redirect URI the client registered",
                "registeredRedirectUri",
            );
        }
    })
}

/// Tokens are only issued for this gateway. Clients spell its URL in
/// equivalent ways, so the comparison is between normalized URLs.
fn gateway_resource() -> Rule {
    vine::rule(|value, field| {
        let Some(value) = value.as_str() else {
            return;
        };
        let requested = Url::parse(value).ok();
        let gateway = field
            .meta::<GatewayResource>()
            .map(|gateway| gateway.0.as_str());
        if requested.as_ref().map(Url::as_str) != gateway {
            field.report(
                "The {{ field }} field must be the gateway resource",
                "gatewayResource",
            );
        }
    })
}

/// Refresh tokens extend a grant that an authorization code has to start.
fn with_authorization_code() -> Rule {
    vine::rule(|value, field| {
        if let Some(grant_types) = value.as_array()
            && !grant_types
                .iter()
                .any(|grant_type| grant_type.as_str() == Some("authorization_code"))
        {
            field.report(
                "The {{ field }} field must include authorization_code",
                "withAuthorizationCode",
            );
        }
    })
}

/// Redirect URIs of a client registering itself. The schema of RFC 7591 has
/// checked the shape of the metadata by then; this and the validators that
/// follow decide what the gateway accepts of it.
pub static CLIENT_REDIRECT_URIS: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(
        vine::array(vine::string().max_length(2048).use_rule(redirect_target()))
            .min_length(1)
            .max_length(10),
    )
});

/// How a registering client authenticates at the token endpoint. RFC 7591
/// makes `client_secret_basic` the default.
pub static CLIENT_AUTH_METHOD: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(vine::enum_(AUTH_METHODS).parse(|value, _| {
        Some(
            value
                .filter(|value| !value.is_null())
                .unwrap_or_else(|| json!("client_secret_basic")),
        )
    }))
});

/// Grant types of a registering client, both by default. The list is stored
/// and parsed again on every /authorize and /token request, so it is reduced
/// to its distinct values and bounded before its members are looked at.
pub static CLIENT_GRANT_TYPES: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(
        vine::array(vine::enum_(GRANT_TYPES))
            .parse(|value, _| {
                Some(distinct_values(
                    value
                        .filter(|value| !value.is_null())
                        .unwrap_or_else(|| json!(GRANT_TYPES)),
                ))
            })
            .max_length(GRANT_TYPES.len())
            .use_rule(with_authorization_code()),
    )
});

/// Response types of a registering client, reduced and bounded like its grant
/// types. OAuth 2.1 only keeps the authorization code flow.
pub static CLIENT_RESPONSE_TYPES: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(
        vine::array(vine::literal("code"))
            .parse(|value, _| {
                Some(distinct_values(
                    value
                        .filter(|value| !value.is_null())
                        .unwrap_or_else(|| json!(["code"])),
                ))
            })
            .fixed_length(1),
    )
});

/// The `scope` of a registration or an authorization request. When sent, it
/// must name the gateway scope and nothing else.
pub static REQUESTED_SCOPE: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(
        vine::array(vine::literal(GATEWAY_OAUTH_SCOPE))
            .parse(|value, _| scope_list(value))
            .fixed_length(1)
            .optional(),
    )
});

/// Name of a registering client, shown on the consent screen and in the list
/// of connections.
pub static CLIENT_NAME: LazyLock<Validator> =
    LazyLock::new(|| VERBATIM.create(vine::string().trim().max_length(120).optional()));

/// `client_id` of an authorization request.
pub static AUTHORIZATION_CLIENT_ID: LazyLock<Validator> =
    LazyLock::new(|| VERBATIM.create(required_parameter()));

/// `redirect_uri` of an authorization request, judged against the URIs of the
/// client the request names: validate it with a [`RegisteredRedirectUris`].
pub static AUTHORIZATION_REDIRECT_URI: LazyLock<Validator> =
    LazyLock::new(|| VERBATIM.create(required_parameter().use_rule(registered_redirect_uri())));

/// `state` as the client sent it, to be echoed with whatever answer the
/// request gets. In any shape but a string it is no state at all.
pub static AUTHORIZATION_STATE: LazyLock<Validator> =
    LazyLock::new(|| VERBATIM.create(vine::string().optional()));

/// The longest `state` an authorization request may carry.
pub static AUTHORIZATION_STATE_LENGTH: LazyLock<Validator> =
    LazyLock::new(|| VERBATIM.create(vine::string().max_length(2048).nullable()));

/// `response_type` of an authorization request.
pub static AUTHORIZATION_RESPONSE_TYPE: LazyLock<Validator> =
    LazyLock::new(|| VERBATIM.create(vine::literal("code")));

/// PKCE (RFC 7636) parameters of an authorization request: a base64url
/// SHA-256 challenge, the only method accepted.
pub static PKCE_CHALLENGE: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(vine::object! {
        "code_challenge" => vine::string().regex(static_regex(r"^[A-Za-z0-9_-]{43,128}$")),
        "code_challenge_method" => vine::literal("S256"),
    })
});

/// `resource` of an authorization or token request (RFC 8707): validate it
/// with a [`GatewayResource`].
pub static GATEWAY_RESOURCE: LazyLock<Validator> =
    LazyLock::new(|| VERBATIM.create(vine::string().use_rule(gateway_resource())));

/// Answer of the consent form. Only an explicit approval grants access.
pub static CONSENT_APPROVAL: LazyLock<Validator> =
    LazyLock::new(|| VERBATIM.create(vine::literal("approve")));

/// Client credentials sent in the request body: `client_secret_post`, or a
/// public client naming itself.
pub static POSTED_CLIENT_CREDENTIALS: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(vine::object! {
        "client_id" => required_parameter(),
        "client_secret" => vine::string().parse(|value, _| string_or_absent(value)).optional(),
    })
});

/// What every token request must say. This schema and the three request
/// schemas below only ask for parameters to be present, so the first field
/// that fails names a missing parameter.
pub static TOKEN_REQUEST: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(vine::object! {
        "grant_type" => required_parameter(),
    })
});

/// Parameters of the `authorization_code` grant.
pub static AUTHORIZATION_CODE_GRANT: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(vine::object! {
        "code" => required_parameter(),
        "code_verifier" => required_parameter(),
        "redirect_uri" => required_parameter(),
        "resource" => required_parameter(),
    })
});

/// Parameters of the `refresh_token` grant.
pub static REFRESH_TOKEN_GRANT: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(vine::object! {
        "refresh_token" => required_parameter(),
        "scope" => vine::string().parse(|value, _| string_or_absent(value)).optional(),
        "resource" => required_parameter(),
    })
});

/// Parameters of a revocation request (RFC 7009).
pub static REVOCATION_REQUEST: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(vine::object! {
        "token" => required_parameter(),
    })
});

/// PKCE code verifier, in the form RFC 7636 gives it.
pub static PKCE_VERIFIER: LazyLock<Validator> = LazyLock::new(|| {
    VERBATIM.create(vine::string().regex(static_regex(r"^[A-Za-z0-9._~-]{43,128}$")))
});

/// `scope` of a refresh request: it may repeat the scope of the grant and
/// cannot ask for another.
pub static REFRESH_SCOPE: LazyLock<Validator> =
    LazyLock::new(|| VERBATIM.create(vine::literal(GATEWAY_OAUTH_SCOPE).nullable()));
