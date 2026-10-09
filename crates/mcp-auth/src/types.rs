//! The documents of an OAuth flow, as `shared/auth.js` of the SDK defines
//! them: each type is one zod schema, with the checks that schema applies.
//!
//! `parse` accepts and refuses what the schema does, and fails with the same
//! [`SchemaError`]. A value that holds a URL stays the string the document
//! gave (zod only trims it and drops tabs and line breaks): the caller decides
//! which of them it will fetch. `null` is not an absent member, and is refused.
//!
//! Serializing a document writes its members in the order zod returns them.

use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

use crate::json::Json;
use crate::schema::{ObjectReader, ObjectWriter, SchemaError, no_issue};

/// The Rust type of a member, by the name of its check.
macro_rules! member_type {
    (string) => { String };
    (safe_url) => { String };
    (url) => { String };
    (strings) => { Vec<String> };
    (safe_urls) => { Vec<String> };
    (opt_string) => { Option<String> };
    (opt_safe_url) => { Option<String> };
    (opt_url) => { Option<String> };
    (opt_safe_url_or_empty) => { Option<String> };
    (opt_strings) => { Option<Vec<String>> };
    (opt_safe_urls) => { Option<Vec<String>> };
    (opt_boolean) => { Option<bool> };
    (opt_number) => { Option<f64> };
    (opt_coerced_number) => { Option<f64> };
    (opt_any) => { Option<Value> };
}

/// One zod object schema: the struct, its `parse` and how it is written back.
/// Members are listed in the order of the schema, which is the order of the
/// issues of a refused document and of the members of an accepted one.
macro_rules! document {
    (
        $(#[$attribute:meta])*
        loose struct $name:ident {
            $( $(#[$member_attribute:meta])* $member:ident: $check:ident, )*
        }
    ) => {
        $(#[$attribute])*
        #[derive(Debug, Clone, PartialEq, Default)]
        pub struct $name {
            $( $(#[$member_attribute])* pub $member: member_type!($check), )*
            /// Members the schema does not name. They are kept as received.
            pub extra: Map<String, Value>,
        }

        impl $name {
            pub(crate) fn from_json(value: &Json) -> Result<Self, SchemaError> {
                let mut reader = ObjectReader::new(value)?;
                $( let $member = reader.$check(stringify!($member)); )*
                let extra = reader.finish_loose()?;
                Ok(Self { $( $member: $member.ok_or_else(no_issue)?, )* extra })
            }

            /// The document as a JSON object.
            pub fn to_json(&self) -> Map<String, Value> {
                let mut writer = ObjectWriter::loose(&self.extra);
                $( writer.put(stringify!($member), &self.$member); )*
                writer.finish_loose(&self.extra)
            }
        }

        document!(@common $name);
    };
    (
        $(#[$attribute:meta])*
        strip struct $name:ident {
            $( $(#[$member_attribute:meta])* $member:ident: $check:ident, )*
        }
    ) => {
        $(#[$attribute])*
        #[derive(Debug, Clone, PartialEq, Default)]
        pub struct $name {
            $( $(#[$member_attribute])* pub $member: member_type!($check), )*
        }

        impl $name {
            pub(crate) fn from_json(value: &Json) -> Result<Self, SchemaError> {
                let mut reader = ObjectReader::new(value)?;
                $( let $member = reader.$check(stringify!($member)); )*
                reader.finish_strip()?;
                Ok(Self { $( $member: $member.ok_or_else(no_issue)?, )* })
            }

            /// The document as a JSON object.
            pub fn to_json(&self) -> Map<String, Value> {
                let mut writer = ObjectWriter::default();
                $( writer.put(stringify!($member), &self.$member); )*
                writer.finish_strip()
            }
        }

        document!(@common $name);
    };
    (@common $name:ident) => {
        impl $name {
            /// Checks a document as the schema of the SDK does.
            pub fn parse(value: &Value) -> Result<Self, SchemaError> {
                Self::from_json(&Json::from_value(value))
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.to_json().serialize(serializer)
            }
        }
    };
}

document! {
    /// RFC 9728 OAuth Protected Resource Metadata
    /// (`OAuthProtectedResourceMetadataSchema`).
    loose struct OAuthProtectedResourceMetadata {
        resource: url,
        authorization_servers: opt_safe_urls,
        jwks_uri: opt_url,
        scopes_supported: opt_strings,
        bearer_methods_supported: opt_strings,
        resource_signing_alg_values_supported: opt_strings,
        resource_name: opt_string,
        resource_documentation: opt_string,
        resource_policy_uri: opt_url,
        resource_tos_uri: opt_url,
        tls_client_certificate_bound_access_tokens: opt_boolean,
        authorization_details_types_supported: opt_strings,
        dpop_signing_alg_values_supported: opt_strings,
        dpop_bound_access_tokens_required: opt_boolean,
    }
}

document! {
    /// RFC 8414 OAuth 2.0 Authorization Server Metadata (`OAuthMetadataSchema`).
    ///
    /// Everything but the four required members has a default, so that a
    /// document can be written by hand for a provider without discovery.
    loose struct OAuthMetadata {
        issuer: string,
        authorization_endpoint: safe_url,
        token_endpoint: safe_url,
        registration_endpoint: opt_safe_url,
        scopes_supported: opt_strings,
        response_types_supported: strings,
        response_modes_supported: opt_strings,
        grant_types_supported: opt_strings,
        token_endpoint_auth_methods_supported: opt_strings,
        token_endpoint_auth_signing_alg_values_supported: opt_strings,
        service_documentation: opt_safe_url,
        revocation_endpoint: opt_safe_url,
        revocation_endpoint_auth_methods_supported: opt_strings,
        revocation_endpoint_auth_signing_alg_values_supported: opt_strings,
        introspection_endpoint: opt_string,
        introspection_endpoint_auth_methods_supported: opt_strings,
        introspection_endpoint_auth_signing_alg_values_supported: opt_strings,
        code_challenge_methods_supported: opt_strings,
        client_id_metadata_document_supported: opt_boolean,
    }
}

document! {
    /// OpenID Connect Discovery 1.0 Provider Metadata, with the one OAuth
    /// member providers add to it (`OpenIdProviderDiscoveryMetadataSchema`).
    ///
    /// Unlike [`OAuthMetadata`], members the schema does not name are dropped.
    strip struct OpenIdProviderDiscoveryMetadata {
        issuer: string,
        authorization_endpoint: safe_url,
        token_endpoint: safe_url,
        userinfo_endpoint: opt_safe_url,
        jwks_uri: safe_url,
        registration_endpoint: opt_safe_url,
        scopes_supported: opt_strings,
        response_types_supported: strings,
        response_modes_supported: opt_strings,
        grant_types_supported: opt_strings,
        acr_values_supported: opt_strings,
        subject_types_supported: strings,
        id_token_signing_alg_values_supported: strings,
        id_token_encryption_alg_values_supported: opt_strings,
        id_token_encryption_enc_values_supported: opt_strings,
        userinfo_signing_alg_values_supported: opt_strings,
        userinfo_encryption_alg_values_supported: opt_strings,
        userinfo_encryption_enc_values_supported: opt_strings,
        request_object_signing_alg_values_supported: opt_strings,
        request_object_encryption_alg_values_supported: opt_strings,
        request_object_encryption_enc_values_supported: opt_strings,
        token_endpoint_auth_methods_supported: opt_strings,
        token_endpoint_auth_signing_alg_values_supported: opt_strings,
        display_values_supported: opt_strings,
        claim_types_supported: opt_strings,
        claims_supported: opt_strings,
        service_documentation: opt_string,
        claims_locales_supported: opt_strings,
        ui_locales_supported: opt_strings,
        claims_parameter_supported: opt_boolean,
        request_parameter_supported: opt_boolean,
        request_uri_parameter_supported: opt_boolean,
        require_request_uri_registration: opt_boolean,
        op_policy_uri: opt_safe_url,
        op_tos_uri: opt_safe_url,
        client_id_metadata_document_supported: opt_boolean,
        code_challenge_methods_supported: opt_strings,
    }
}

document! {
    /// OAuth 2.1 token response (`OAuthTokensSchema`, without the `issuer`
    /// stamp the SDK's own `auth()` adds to what it stores).
    strip struct OAuthTokens {
        access_token: string,
        /// Optional for OAuth 2.1, but necessary in OpenID Connect.
        id_token: opt_string,
        token_type: string,
        /// Seconds, as a JavaScript number: the schema converts what the
        /// server sent with `Number()`, so `"3600"` is 3600 and `null` is 0.
        expires_in: opt_coerced_number,
        scope: opt_string,
        refresh_token: opt_string,
    }
}

document! {
    /// RFC 7591 OAuth 2.0 Dynamic Client Registration metadata
    /// (`OAuthClientMetadataSchema`).
    strip struct OAuthClientMetadata {
        redirect_uris: safe_urls,
        token_endpoint_auth_method: opt_string,
        grant_types: opt_strings,
        response_types: opt_strings,
        client_name: opt_string,
        client_uri: opt_safe_url,
        /// An empty string is read as absent.
        logo_uri: opt_safe_url_or_empty,
        scope: opt_string,
        contacts: opt_strings,
        /// An empty string is read as absent.
        tos_uri: opt_safe_url_or_empty,
        policy_uri: opt_string,
        jwks_uri: opt_safe_url,
        jwks: opt_any,
        software_id: opt_string,
        software_version: opt_string,
        software_statement: opt_string,
    }
}

document! {
    /// RFC 7591 OAuth 2.0 Dynamic Client Registration full response: client
    /// metadata plus client information (`OAuthClientInformationFullSchema`,
    /// without the `issuer` stamp the SDK's own `auth()` adds to what it
    /// stores).
    strip struct OAuthClientInformationFull {
        redirect_uris: safe_urls,
        token_endpoint_auth_method: opt_string,
        grant_types: opt_strings,
        response_types: opt_strings,
        client_name: opt_string,
        client_uri: opt_safe_url,
        /// An empty string is read as absent.
        logo_uri: opt_safe_url_or_empty,
        scope: opt_string,
        contacts: opt_strings,
        /// An empty string is read as absent.
        tos_uri: opt_safe_url_or_empty,
        policy_uri: opt_string,
        jwks_uri: opt_safe_url,
        jwks: opt_any,
        software_id: opt_string,
        software_version: opt_string,
        software_statement: opt_string,
        client_id: string,
        client_secret: opt_string,
        client_id_issued_at: opt_number,
        client_secret_expires_at: opt_number,
    }
}

document! {
    /// OAuth 2.1 error response (`OAuthErrorResponseSchema`).
    strip struct OAuthErrorResponse {
        error: string,
        error_description: opt_string,
        error_uri: opt_string,
    }
}

/// What the SDK reads from `OAuthClientInformationMixed`, the union of the
/// client information a registration returns and of the one an application
/// keeps: the identifier, the secret of a confidential client, and the
/// authentication method the authorization server registered it with.
///
/// A secret that is present but empty still counts as a secret when the
/// authentication method is chosen, as it does in the SDK.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OAuthClientInformationMixed {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_endpoint_auth_method: Option<String>,
}

impl From<&OAuthClientInformationFull> for OAuthClientInformationMixed {
    fn from(client: &OAuthClientInformationFull) -> Self {
        Self {
            client_id: client.client_id.clone(),
            client_secret: client.client_secret.clone(),
            token_endpoint_auth_method: client.token_endpoint_auth_method.clone(),
        }
    }
}

impl From<OAuthClientInformationFull> for OAuthClientInformationMixed {
    fn from(client: OAuthClientInformationFull) -> Self {
        Self {
            client_id: client.client_id,
            client_secret: client.client_secret,
            token_endpoint_auth_method: client.token_endpoint_auth_method,
        }
    }
}

/// The metadata of an authorization server, from either document that
/// describes one: `OAuthMetadata | OpenIdProviderDiscoveryMetadata`.
///
/// Build one with `.into()` from either document. The accessors read the
/// members both documents have, which are all the helpers look at.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthorizationServerMetadata {
    OAuth(Box<OAuthMetadata>),
    OpenId(Box<OpenIdProviderDiscoveryMetadata>),
}

impl AuthorizationServerMetadata {
    pub fn issuer(&self) -> &str {
        match self {
            Self::OAuth(metadata) => &metadata.issuer,
            Self::OpenId(metadata) => &metadata.issuer,
        }
    }

    pub fn authorization_endpoint(&self) -> &str {
        match self {
            Self::OAuth(metadata) => &metadata.authorization_endpoint,
            Self::OpenId(metadata) => &metadata.authorization_endpoint,
        }
    }

    pub fn token_endpoint(&self) -> &str {
        match self {
            Self::OAuth(metadata) => &metadata.token_endpoint,
            Self::OpenId(metadata) => &metadata.token_endpoint,
        }
    }

    pub fn registration_endpoint(&self) -> Option<&str> {
        match self {
            Self::OAuth(metadata) => metadata.registration_endpoint.as_deref(),
            Self::OpenId(metadata) => metadata.registration_endpoint.as_deref(),
        }
    }

    pub fn scopes_supported(&self) -> Option<&[String]> {
        match self {
            Self::OAuth(metadata) => metadata.scopes_supported.as_deref(),
            Self::OpenId(metadata) => metadata.scopes_supported.as_deref(),
        }
    }

    pub fn response_types_supported(&self) -> &[String] {
        match self {
            Self::OAuth(metadata) => &metadata.response_types_supported,
            Self::OpenId(metadata) => &metadata.response_types_supported,
        }
    }

    pub fn grant_types_supported(&self) -> Option<&[String]> {
        match self {
            Self::OAuth(metadata) => metadata.grant_types_supported.as_deref(),
            Self::OpenId(metadata) => metadata.grant_types_supported.as_deref(),
        }
    }

    pub fn token_endpoint_auth_methods_supported(&self) -> Option<&[String]> {
        match self {
            Self::OAuth(metadata) => metadata.token_endpoint_auth_methods_supported.as_deref(),
            Self::OpenId(metadata) => metadata.token_endpoint_auth_methods_supported.as_deref(),
        }
    }

    pub fn code_challenge_methods_supported(&self) -> Option<&[String]> {
        match self {
            Self::OAuth(metadata) => metadata.code_challenge_methods_supported.as_deref(),
            Self::OpenId(metadata) => metadata.code_challenge_methods_supported.as_deref(),
        }
    }

    pub fn client_id_metadata_document_supported(&self) -> Option<bool> {
        match self {
            Self::OAuth(metadata) => metadata.client_id_metadata_document_supported,
            Self::OpenId(metadata) => metadata.client_id_metadata_document_supported,
        }
    }

    /// The document as a JSON object.
    pub fn to_json(&self) -> Map<String, Value> {
        match self {
            Self::OAuth(metadata) => metadata.to_json(),
            Self::OpenId(metadata) => metadata.to_json(),
        }
    }
}

impl From<OAuthMetadata> for AuthorizationServerMetadata {
    fn from(metadata: OAuthMetadata) -> Self {
        Self::OAuth(Box::new(metadata))
    }
}

impl From<OpenIdProviderDiscoveryMetadata> for AuthorizationServerMetadata {
    fn from(metadata: OpenIdProviderDiscoveryMetadata) -> Self {
        Self::OpenId(Box::new(metadata))
    }
}

impl Serialize for AuthorizationServerMetadata {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    /// Numbers by their bits and strings behind a marker, as the recorder
    /// wrote the reference outputs.
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Number(number) => {
                let bits = number.as_f64().unwrap().to_bits();
                Value::String(format!("#{bits:016x}"))
            }
            Value::String(text) => Value::String(format!("${text}")),
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            Value::Object(members) => Value::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn outcome(schema: &str, text: &str) -> (i64, String) {
        let input = json::parse(text).unwrap();
        let parsed = match schema {
            "oauthMetadata" => OAuthMetadata::from_json(&input).map(|document| document.to_json()),
            "openIdMetadata" => OpenIdProviderDiscoveryMetadata::from_json(&input)
                .map(|document| document.to_json()),
            "protectedResourceMetadata" => {
                OAuthProtectedResourceMetadata::from_json(&input).map(|document| document.to_json())
            }
            "tokens" => OAuthTokens::from_json(&input).map(|document| document.to_json()),
            "clientInformationFull" => {
                OAuthClientInformationFull::from_json(&input).map(|document| document.to_json())
            }
            "clientMetadata" => {
                OAuthClientMetadata::from_json(&input).map(|document| document.to_json())
            }
            "errorResponse" => {
                OAuthErrorResponse::from_json(&input).map(|document| document.to_json())
            }
            other => panic!("unknown schema {other}"),
        };
        match parsed {
            Ok(document) => (0, canonical(&Value::Object(document)).to_string()),
            Err(error) => (1, error.to_string()),
        }
    }

    #[test]
    fn accepts_and_refuses_what_the_zod_schemas_do_with_the_same_issues() {
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/fixtures/schemas.json")).unwrap();
        let cases = corpus["cases"].as_array().unwrap();
        assert!(cases.len() > 1000);
        let mut failures = Vec::new();
        for case in cases {
            let schema = case[0].as_str().unwrap();
            let text = case[1].as_str().unwrap();
            let expected = (case[2].as_i64().unwrap(), case[3].as_str().unwrap());
            let actual = outcome(schema, text);
            if (actual.0, actual.1.as_str()) != expected {
                failures.push(format!(
                    "{schema} {text}\n  node: {} {}\n  rust: {} {}",
                    expected.0, expected.1, actual.0, actual.1
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} cases differ from zod:\n{}",
            failures.len(),
            cases.len(),
            failures[..failures.len().min(12)].join("\n")
        );
    }

    #[test]
    fn checks_a_value_another_parser_read() {
        let metadata = OAuthMetadata::parse(&serde_json::json!({
            "issuer": "https://auth.example",
            "authorization_endpoint": " https://auth.example/authorize ",
            "token_endpoint": "https://auth.example/token",
            "response_types_supported": ["code"],
            "vendor": {"b": 1, "a": 2},
        }))
        .unwrap();
        assert_eq!(
            metadata.authorization_endpoint,
            "https://auth.example/authorize"
        );
        assert_eq!(
            serde_json::to_string(&metadata).unwrap(),
            r#"{"issuer":"https://auth.example","authorization_endpoint":"https://auth.example/authorize","token_endpoint":"https://auth.example/token","response_types_supported":["code"],"vendor":{"b":1,"a":2}}"#
        );

        let error = OAuthTokens::parse(&serde_json::json!({ "access_token": "a" })).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0]["path"], serde_json::json!(["token_type"]));
    }

    #[test]
    fn a_document_written_by_hand_only_states_what_it_knows() {
        let metadata = OAuthMetadata {
            issuer: "https://auth.example".to_owned(),
            authorization_endpoint: "https://auth.example/authorize".to_owned(),
            token_endpoint: "https://auth.example/token".to_owned(),
            response_types_supported: vec!["code".to_owned()],
            code_challenge_methods_supported: Some(vec!["S256".to_owned()]),
            ..OAuthMetadata::default()
        };
        let metadata = AuthorizationServerMetadata::from(metadata);
        assert_eq!(metadata.registration_endpoint(), None);
        assert_eq!(metadata.to_json().len(), 5);
    }
}
