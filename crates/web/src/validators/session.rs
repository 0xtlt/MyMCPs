//! Values the app reads back from the browser session.
//! (`app/validators/session.ts`; the session stamp is checked in `auth`.)

use std::sync::LazyLock;

use mymcps_vine as vine;

/// Where sign-in returns to when it interrupted a gateway authorization
/// request. The app only ever stores a path on its own authorization endpoint.
pub static OAUTH_RETURN_TO_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::string().starts_with("/authorize?").max_length(1536))
});

/// Where sign-in returns to when it interrupted someone opening an approval
/// link. The app only ever stores the path of one request.
pub static APPROVAL_RETURN_TO_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| {
        vine::global().create(vine::string().regex(
            vine::js::regex(r"^\/approvals\/[A-Za-z0-9_-]{32}$", "").expect("a static pattern"),
        ))
    });

/// The id of a record flashed for the next page, such as the MCP whose
/// dialog should reopen.
pub static FLASHED_RECORD_ID_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(vine::number().strict().without_decimals().positive()));

/// A text flashed for the next page, such as an access token shown once.
pub static FLASHED_TEXT_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(vine::string()));
