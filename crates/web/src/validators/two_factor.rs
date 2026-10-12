//! Sign-in protection: the forms of the authenticator app, of the recovery
//! codes and of passkeys, and what the page's script sends for a passkey.

use std::sync::LazyLock;

use mymcps_vine as vine;

/// The longest text one field of a passkey answer may be. The largest, the
/// statement an authenticator makes about itself, is a few kilobytes of
/// certificates in base64.
const MAX_PASSKEY_FIELD: usize = 64 * 1024;

fn current_password() -> vine::VineString {
    vine::string().min_length(1)
}

fn encoded(max_length: usize) -> vine::VineString {
    vine::string().min_length(1).max_length(max_length)
}

/// Something that lowers or replaces the protection of an account: the
/// password of the person who asks, to confirm it is them.
pub static CONFIRM_PASSWORD_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! { "currentPassword" => current_password() })
});

/// Turn the authenticator app on: a code it shows, and the password.
pub static ENABLE_AUTHENTICATOR_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "code" => vine::string().trim().min_length(1).max_length(16),
        "currentPassword" => current_password(),
    })
});

/// The second step of a sign-in: a code of the app, or a recovery code.
pub static SECOND_STEP_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "code" => vine::string().trim().min_length(1).max_length(64),
    })
});

/// Ask to add a passkey: what to call it, and the password.
pub static NEW_PASSKEY_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "name" => vine::string().trim().max_length(120).optional(),
        "currentPassword" => current_password(),
    })
});

/// What `navigator.credentials.create()` answered, in base64url.
pub static PASSKEY_REGISTRATION_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "id" => encoded(1400),
        "clientDataJSON" => encoded(MAX_PASSKEY_FIELD),
        "attestationObject" => encoded(MAX_PASSKEY_FIELD),
    })
});

/// What `navigator.credentials.get()` answered, in base64url.
pub static PASSKEY_ASSERTION_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "id" => encoded(1400),
        "clientDataJSON" => encoded(MAX_PASSKEY_FIELD),
        "authenticatorData" => encoded(MAX_PASSKEY_FIELD),
        "signature" => encoded(MAX_PASSKEY_FIELD),
    })
});
