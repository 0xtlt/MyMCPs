//! Passkeys (WebAuthn): the ceremonies that register one and sign in with
//! one, around `webauthn-rs`.
//!
//! The relying party is `APP_URL`: its host is the RP ID and its origin the
//! only one accepted. Without a valid `APP_URL` there are no passkeys.
//!
//! The state of a ceremony between its options and its answer holds the
//! challenge, and must not reach the browser: it stays in memory here, under
//! a random key the session carries. A restart forgets ceremonies under way,
//! which the person then starts again.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use mymcps_core::Config;
use mymcps_core::crypto::random_base64url;
use mymcps_core::models::{User, UserPasskey};
use serde_json::Value;
use webauthn_rs::prelude::{
    CredentialID, DiscoverableAuthentication, DiscoverableKey, Passkey, PasskeyAuthentication,
    PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential, Url, Uuid, Webauthn,
    WebauthnBuilder,
};

use crate::session::Session;

/// Session key of the ceremony under way in this browser.
const CEREMONY_KEY: &str = "passkeyCeremony";

/// How long the browser has to answer, as long as it is told to wait.
const CEREMONY_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// Ceremonies kept at once: options are asked for without signing in, and
/// must not fill the memory of the server.
const MAX_CEREMONIES: usize = 1000;

/// What a ceremony is for, with the state `webauthn-rs` needs to finish it.
enum Ceremony {
    /// A signed-in user adds a passkey called `name`.
    Register {
        user_id: i64,
        name: String,
        user_handle: Uuid,
        state: PasskeyRegistration,
    },
    /// Anyone signs in with a passkey alone.
    SignIn(DiscoverableAuthentication),
    /// A user who typed their password proves the second step.
    SecondStep {
        user_id: i64,
        state: PasskeyAuthentication,
    },
}

struct Entry {
    ceremony: Ceremony,
    expires_at: Instant,
}

/// Why a ceremony could not finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasskeyFailure {
    /// No ceremony of this kind under way for this browser, or it expired.
    NoCeremony,
    /// The answer of the browser does not parse, or does not verify.
    Rejected,
    /// The passkey is not one this instance knows.
    Unknown,
}

/// A passkey that was just registered, ready to store.
pub struct NewPasskey {
    pub user_id: i64,
    pub name: String,
    pub credential_id: String,
    pub user_handle: String,
    pub passkey: String,
}

/// A passkey that signed someone in, with its counter brought up to date.
pub struct UsedPasskey {
    pub stored: UserPasskey,
}

pub struct Passkeys {
    webauthn: Option<Webauthn>,
    ceremonies: Mutex<HashMap<String, Entry>>,
}

/// The credential id as the table stores it.
pub fn credential_id_text(id: &CredentialID) -> String {
    let bytes: &[u8] = id.as_ref();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn build(config: &Config) -> Option<Webauthn> {
    let origin = Url::parse(&config.require_public_app_url().ok()?).ok()?;
    let rp_id = origin.host_str()?.to_string();
    let built = WebauthnBuilder::new(&rp_id, &origin)
        .and_then(|builder| builder.rp_name("MyMCPs").timeout(CEREMONY_LIFETIME).build());
    match built {
        Ok(webauthn) => Some(webauthn),
        Err(error) => {
            tracing::warn!(%error, "Passkeys are off: APP_URL cannot be a WebAuthn relying party");
            None
        }
    }
}

impl Passkeys {
    pub fn new(config: &Config) -> Self {
        Self {
            webauthn: build(config),
            ceremonies: Mutex::new(HashMap::new()),
        }
    }

    /// Whether passkeys can be registered and used on this instance.
    pub fn is_available(&self) -> bool {
        self.webauthn.is_some()
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.ceremonies
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Keep a ceremony for this browser, replacing the one it had. `None`
    /// when too many are under way.
    fn begin(&self, session: &Session, ceremony: Ceremony) -> Option<()> {
        let now = Instant::now();
        let mut entries = self.entries();
        if let Some(previous) = session.get_as::<String>(CEREMONY_KEY) {
            entries.remove(&previous);
        }
        if entries.len() >= MAX_CEREMONIES {
            entries.retain(|_, entry| entry.expires_at > now);
            if entries.len() >= MAX_CEREMONIES {
                return None;
            }
        }
        let key = random_base64url(24);
        entries.insert(
            key.clone(),
            Entry {
                ceremony,
                expires_at: now + CEREMONY_LIFETIME,
            },
        );
        session.put(CEREMONY_KEY, key);
        Some(())
    }

    /// The ceremony of this browser, which can be finished once only.
    fn take(&self, session: &Session) -> Option<Ceremony> {
        let key = session.pull(CEREMONY_KEY)?;
        let entry = self.entries().remove(key.as_str()?)?;
        (entry.expires_at > Instant::now()).then_some(entry.ceremony)
    }

    /// The options of `navigator.credentials.create()` for a new passkey of
    /// `user`, who already has `existing`. `None` when passkeys are off or
    /// the server is busy.
    pub fn start_registration(
        &self,
        session: &Session,
        user: &User,
        name: &str,
        existing: &[UserPasskey],
    ) -> Option<Value> {
        let webauthn = self.webauthn.as_ref()?;
        // Every passkey of a user carries the same handle: it is how a
        // discoverable passkey names its account at sign-in.
        let user_handle = existing
            .iter()
            .find_map(|passkey| Uuid::parse_str(&passkey.user_handle).ok())
            .unwrap_or_else(Uuid::new_v4);
        let exclude: Vec<CredentialID> = existing
            .iter()
            .filter_map(|stored| serde_json::from_str::<Passkey>(&stored.passkey).ok())
            .map(|passkey| passkey.cred_id().clone())
            .collect();
        let display_name = user
            .full_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&user.email);
        let (options, state) = webauthn
            .start_passkey_registration(user_handle, &user.email, display_name, Some(exclude))
            .ok()?;
        let mut options = serde_json::to_value(options).ok()?;
        // Discoverable, so that the passkey signs in without an email first. The browser is only
        // asked: a key that cannot store it still registers, and then serves as a second step.
        if let Some(selection) = options
            .pointer_mut("/publicKey/authenticatorSelection")
            .and_then(Value::as_object_mut)
        {
            selection.insert("residentKey".into(), Value::from("required"));
            selection.insert("requireResidentKey".into(), Value::from(true));
        }
        self.begin(
            session,
            Ceremony::Register {
                user_id: user.id,
                name: name.to_string(),
                user_handle,
                state,
            },
        )?;
        Some(options)
    }

    /// The ID of the credential a registration answer carries, as stored.
    pub fn registered_id(credential: &str) -> Option<String> {
        serde_json::from_str::<RegisterPublicKeyCredential>(credential)
            .ok()
            .map(|credential| {
                let bytes: &[u8] = credential.raw_id.as_ref();
                URL_SAFE_NO_PAD.encode(bytes)
            })
    }

    /// Verify the answer to [`Passkeys::start_registration`].
    pub fn finish_registration(
        &self,
        session: &Session,
        user: &User,
        credential: &str,
    ) -> Result<NewPasskey, PasskeyFailure> {
        let webauthn = self.webauthn.as_ref().ok_or(PasskeyFailure::NoCeremony)?;
        let Some(Ceremony::Register {
            user_id,
            name,
            user_handle,
            state,
        }) = self.take(session)
        else {
            return Err(PasskeyFailure::NoCeremony);
        };
        if user_id != user.id {
            return Err(PasskeyFailure::NoCeremony);
        }
        let credential: RegisterPublicKeyCredential =
            serde_json::from_str(credential).map_err(|_| PasskeyFailure::Rejected)?;
        let passkey = webauthn
            .finish_passkey_registration(&credential, &state)
            .map_err(|error| {
                tracing::info!(%error, "A passkey registration was refused");
                PasskeyFailure::Rejected
            })?;
        Ok(NewPasskey {
            user_id,
            name,
            credential_id: credential_id_text(passkey.cred_id()),
            user_handle: user_handle.hyphenated().to_string(),
            passkey: serde_json::to_string(&passkey).map_err(|_| PasskeyFailure::Rejected)?,
        })
    }

    /// The options of `navigator.credentials.get()` to sign in with any
    /// passkey of this instance.
    pub fn start_sign_in(&self, session: &Session) -> Option<Value> {
        let webauthn = self.webauthn.as_ref()?;
        let (mut options, state) = webauthn.start_discoverable_authentication().ok()?;
        // `webauthn-rs` asks for conditional mediation (autofill); the
        // button of the sign-in page asks for a passkey outright.
        options.mediation = None;
        self.begin(session, Ceremony::SignIn(state))?;
        serde_json::to_value(options).ok()
    }

    /// The options of `navigator.credentials.get()` for the second step of
    /// the user, restricted to their passkeys.
    pub fn start_second_step(
        &self,
        session: &Session,
        user_id: i64,
        passkeys: &[UserPasskey],
    ) -> Option<Value> {
        let webauthn = self.webauthn.as_ref()?;
        let credentials: Vec<Passkey> = passkeys
            .iter()
            .filter_map(|stored| serde_json::from_str(&stored.passkey).ok())
            .collect();
        if credentials.is_empty() {
            return None;
        }
        let (options, state) = webauthn.start_passkey_authentication(&credentials).ok()?;
        self.begin(session, Ceremony::SecondStep { user_id, state })?;
        serde_json::to_value(options).ok()
    }

    /// Verify the answer to [`Passkeys::start_sign_in`]. `find` looks the
    /// passkey up by its credential id.
    pub async fn finish_sign_in<F, Fut>(
        &self,
        session: &Session,
        credential: &str,
        find: F,
    ) -> Result<Result<UsedPasskey, PasskeyFailure>, sqlx::Error>
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = Result<Option<UserPasskey>, sqlx::Error>>,
    {
        let Some(webauthn) = self.webauthn.as_ref() else {
            return Ok(Err(PasskeyFailure::NoCeremony));
        };
        let Some(Ceremony::SignIn(state)) = self.take(session) else {
            return Ok(Err(PasskeyFailure::NoCeremony));
        };
        let Ok(credential) = serde_json::from_str::<PublicKeyCredential>(credential) else {
            return Ok(Err(PasskeyFailure::Rejected));
        };
        let Ok((user_handle, credential_id)) =
            webauthn.identify_discoverable_authentication(&credential)
        else {
            return Ok(Err(PasskeyFailure::Rejected));
        };
        let Some(stored) = find(URL_SAFE_NO_PAD.encode(credential_id)).await? else {
            return Ok(Err(PasskeyFailure::Unknown));
        };
        if Uuid::parse_str(&stored.user_handle).ok() != Some(user_handle) {
            return Ok(Err(PasskeyFailure::Unknown));
        }
        let Ok(mut passkey) = serde_json::from_str::<Passkey>(&stored.passkey) else {
            return Ok(Err(PasskeyFailure::Unknown));
        };
        let result = webauthn.finish_discoverable_authentication(
            &credential,
            state,
            &[DiscoverableKey::from(&passkey)],
        );
        Ok(match result {
            Ok(result) => {
                passkey.update_credential(&result);
                Ok(used(stored, &passkey))
            }
            Err(error) => {
                tracing::info!(%error, "A passkey sign-in was refused");
                Err(PasskeyFailure::Rejected)
            }
        })
    }

    /// Verify the answer to [`Passkeys::start_second_step`] for `user_id`,
    /// whose passkeys are `passkeys`.
    pub fn finish_second_step(
        &self,
        session: &Session,
        user_id: i64,
        credential: &str,
        passkeys: Vec<UserPasskey>,
    ) -> Result<UsedPasskey, PasskeyFailure> {
        let webauthn = self.webauthn.as_ref().ok_or(PasskeyFailure::NoCeremony)?;
        let Some(Ceremony::SecondStep {
            user_id: started_for,
            state,
        }) = self.take(session)
        else {
            return Err(PasskeyFailure::NoCeremony);
        };
        if started_for != user_id {
            return Err(PasskeyFailure::NoCeremony);
        }
        let credential: PublicKeyCredential =
            serde_json::from_str(credential).map_err(|_| PasskeyFailure::Rejected)?;
        let result = webauthn
            .finish_passkey_authentication(&credential, &state)
            .map_err(|error| {
                tracing::info!(%error, "A passkey second step was refused");
                PasskeyFailure::Rejected
            })?;
        let id = credential_id_text(result.cred_id());
        let stored = passkeys
            .into_iter()
            .find(|stored| stored.credential_id == id)
            .ok_or(PasskeyFailure::Unknown)?;
        let mut passkey: Passkey =
            serde_json::from_str(&stored.passkey).map_err(|_| PasskeyFailure::Unknown)?;
        passkey.update_credential(&result);
        Ok(used(stored, &passkey))
    }
}

fn used(mut stored: UserPasskey, passkey: &Passkey) -> UsedPasskey {
    if let Ok(serialised) = serde_json::to_string(passkey) {
        stored.passkey = serialised;
    }
    stored.last_used_at = Some(mymcps_core::Timestamp::now());
    UsedPasskey { stored }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(app_url: &str) -> Config {
        let mut config = Config::for_tests(std::env::temp_dir());
        config.app_url = Some(app_url.to_string());
        config
    }

    #[test]
    fn the_relying_party_is_app_url() {
        assert!(Passkeys::new(&config("http://localhost:3333")).is_available());
        assert!(Passkeys::new(&config("https://mcp.example.com")).is_available());
        // Not an origin: no passkeys rather than ones bound to a wrong site.
        assert!(!Passkeys::new(&config("https://mcp.example.com/gateway")).is_available());
        let mut unset = config("https://mcp.example.com");
        unset.app_url = None;
        assert!(!Passkeys::new(&unset).is_available());
    }

    #[test]
    fn a_ceremony_is_finished_once_and_by_its_browser_only() {
        let passkeys = Passkeys::new(&config("http://localhost:3333"));
        let session = Session::default();
        let options = passkeys.start_sign_in(&session).unwrap();
        assert!(options["publicKey"]["challenge"].is_string());
        assert!(options.get("mediation").is_none());
        assert!(matches!(passkeys.take(&session), Some(Ceremony::SignIn(_))));
        assert!(passkeys.take(&session).is_none());

        // Starting again replaces the ceremony of the browser.
        passkeys.start_sign_in(&session).unwrap();
        passkeys.start_sign_in(&session).unwrap();
        assert_eq!(passkeys.entries().len(), 1);
        let other = Session::default();
        assert!(passkeys.take(&other).is_none());
    }

    #[test]
    fn registration_options_ask_for_a_discoverable_verified_passkey() {
        let passkeys = Passkeys::new(&config("http://localhost:3333"));
        let session = Session::default();
        let user = User {
            id: 7,
            email: "ada@example.com".into(),
            ..Default::default()
        };
        let options = passkeys
            .start_registration(&session, &user, "Laptop", &[])
            .unwrap();
        let public_key = &options["publicKey"];
        assert_eq!(public_key["rp"]["id"], "localhost");
        assert_eq!(public_key["user"]["name"], "ada@example.com");
        assert_eq!(
            public_key["authenticatorSelection"]["residentKey"],
            "required"
        );
        assert_eq!(
            public_key["authenticatorSelection"]["userVerification"],
            "required"
        );
    }
}
