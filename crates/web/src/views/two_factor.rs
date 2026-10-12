//! Sign-in protection: the section of the Settings page where someone turns
//! on an authenticator app or adds a passkey, the pages that set the app up
//! and show the recovery codes, and the second step of a sign-in.

use std::fmt::Write;

use maud::{Markup, html};
use mymcps_core::Timestamp;
use mymcps_core::crypto::totp;
use mymcps_core::models::{Passkey, RECOVERY_CODES};

use crate::forms::FormState;
use crate::views::auth::form_banner;
use crate::views::date;
use crate::views::icon::{icon, logo};
use crate::views::settings::{dialog_header, form_dialog, password_field_with_help};
use crate::views::shell::{PageContext, app_page, auth_page};

/// The name authenticator apps and passkey prompts show for this instance.
pub const ISSUER: &str = "MyMCPs";

/// The dialogs of the section, which are also the fragments the page
/// script asks for.
pub const AUTHENTICATOR_OFF_DIALOG: &str = "authenticator-off";
pub const ADD_PASSKEY_DIALOG: &str = "add-passkey";
pub const REMOVE_PASSKEY_DIALOG: &str = "remove-passkey";

const PASSWORD_HELP: &str = "Your account password, to confirm it is you.";

/// What the "Sign-in protection" section of the Settings page shows.
pub struct Protection<'a> {
    /// Since when a code of an authenticator app is asked for.
    pub authenticator_since: Option<Timestamp>,
    /// Oldest first.
    pub passkeys: &'a [Passkey],
    /// How many recovery codes the account has not used.
    pub recovery_codes_left: i64,
    /// The form that turns the authenticator app off, after it was refused.
    pub authenticator_form: &'a FormState,
    /// The passkey the page shows the "Remove passkey" dialog open on, with
    /// what was refused when its form was posted without the page's script.
    pub removing: Option<(&'a Passkey, &'a FormState)>,
}

impl Protection<'_> {
    fn is_on(&self) -> bool {
        self.authenticator_since.is_some() || !self.passkeys.is_empty()
    }

    /// The forms a refused post reopens, for the page to say their error
    /// once.
    pub fn forms(&self) -> impl Iterator<Item = &FormState> {
        std::iter::once(self.authenticator_form).chain(self.removing.map(|(_, form)| form))
    }
}

fn confirm_password(form: &FormState, id: &str, autofocus: bool) -> Markup {
    password_field_with_help(
        form,
        id,
        "currentPassword",
        "Your password",
        "current-password",
        autofocus,
        Some(PASSWORD_HELP),
    )
}

/// Where the dialog that removes a passkey is drawn open: the address its
/// row links to.
pub fn remove_passkey_href(passkey: &Passkey) -> String {
    format!("/settings/passkeys/{}/delete", passkey.id)
}

/// The section of the Settings page.
pub fn protection_section(protection: &Protection) -> Markup {
    html! {
        section class="section" aria-labelledby="protection-title" {
            div class="section__intro" {
                h2 class="section__title" id="protection-title" { "Sign-in protection" }
                p class="section__description" {
                    "Ask for more than your password when you sign in: a code of an authenticator app, or a passkey."
                }
            }
            div class="card" {
                div class="detail-row" {
                    div class="detail-row__text" {
                        span class="detail-row__label" { "Authenticator app" }
                        span class="detail-row__value" {
                            @if let Some(since) = protection.authenticator_since {
                                "On since " (date(since)) ". A 6-digit code is asked after your password."
                            } @else {
                                "An app such as 1Password, Google Authenticator or Authy shows a 6-digit code, asked after your password."
                            }
                        }
                    }
                    @if protection.authenticator_since.is_some() {
                        button type="button" class="button button--secondary" data-dialog-open=(format!("#{AUTHENTICATOR_OFF_DIALOG}")) { "Turn off" }
                    } @else {
                        a class="button button--secondary" href="/settings/two-factor" { "Set up" }
                    }
                }
                @for passkey in protection.passkeys {
                    div class="detail-row" {
                        div class="detail-row__text" {
                            span class="detail-row__label" { (passkey.name) }
                            span class="detail-row__value" {
                                "Passkey added " (date(passkey.created_at)) ", "
                                @if let Some(used) = passkey.last_used_at { "last used " (date(used)) "." } @else { "never used." }
                            }
                        }
                        a class="button button--secondary" href=(remove_passkey_href(passkey))
                            data-dialog-open=(format!("#{REMOVE_PASSKEY_DIALOG}")) data-dialog-fetch { "Remove" }
                    }
                }
                div class="detail-row" {
                    div class="detail-row__text" {
                        span class="detail-row__label" { "Passkeys" }
                        span class="detail-row__value" {
                            "Sign in with your fingerprint, your face or a security key. A passkey works without your password, and in place of a code."
                        }
                    }
                    button type="button" class="button button--secondary" data-dialog-open=(format!("#{ADD_PASSKEY_DIALOG}")) { "Add passkey" }
                }
                @if protection.is_on() {
                    div class="detail-row" {
                        div class="detail-row__text" {
                            span class="detail-row__label" { "Recovery codes" }
                            span class="detail-row__value" {
                                (protection.recovery_codes_left) " of " (RECOVERY_CODES)
                                " left. Each one signs you in once, when you have neither your app nor a passkey at hand."
                            }
                        }
                        a class="button button--secondary" href="/settings/recovery-codes" { "New codes" }
                    }
                }
            }
        }
    }
}

/// The form of the dialog that turns the authenticator app off.
pub fn authenticator_off_form(context: &PageContext, form: &FormState) -> Markup {
    html! {
        form method="post" action="/settings/two-factor?_method=DELETE" data-async {
            (context.csrf_field())
            (dialog_header(
                "authenticator-off-title",
                "Turn off the authenticator app?",
                "Signing in no longer asks for a code of the app.",
            ))
            div class="dialog__body" {
                (confirm_password(form, "authenticator-off-password", !form.has_errors()))
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                button type="submit" class="button button--critical" { "Turn off" }
            }
        }
    }
}

/// The form of the "Add passkey" dialog. The page script sends it: it asks
/// the server what the browser must sign, the browser for a passkey, and
/// sends the passkey back.
pub fn add_passkey_form(context: &PageContext, form: &FormState) -> Markup {
    let error = form.error("name");
    html! {
        form method="post" action="/settings/passkeys" data-passkey="create" data-passkey-options="/settings/passkeys/options" {
            (context.csrf_field())
            (dialog_header(
                "add-passkey-title",
                "Add a passkey",
                "Your browser then asks where to keep it: this device, a phone, a password manager or a security key.",
            ))
            div class="dialog__body" {
                div class="field" {
                    label class="field__label" for="passkey-name" { "Name " span class="field__optional" { "Optional" } }
                    input class="input" id="passkey-name" name="name" type="text" autocomplete="off" maxlength="120" value=(form.old("name"))
                        autofocus[!form.has_errors()] aria-invalid=[error.map(|_| "true")] aria-describedby="passkey-name-help";
                    @if let Some(message) = error { p class="field__error" { (message) } }
                    p class="field__help" id="passkey-name-help" { "To tell your passkeys apart, such as \"MacBook\" or \"YubiKey\"." }
                }
                (confirm_password(form, "passkey-password", false))
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                button type="submit" class="button button--primary" { "Continue" }
            }
        }
    }
}

/// The form of the "Remove passkey" dialog: what its `[data-fragment]`
/// holds, and what a refused submission of the page's script gets back.
pub fn remove_passkey_form(context: &PageContext, passkey: &Passkey, form: &FormState) -> Markup {
    html! {
        form method="post" action=(format!("/settings/passkeys/{}?_method=DELETE", passkey.id)) data-async {
            (context.csrf_field())
            (dialog_header(
                "remove-passkey-title",
                &format!("Remove {}?", passkey.name),
                "It no longer signs you in. Delete it from the device or the password manager that keeps it as well.",
            ))
            div class="dialog__body" {
                (confirm_password(form, "remove-passkey-password", !form.has_errors()))
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                button type="submit" class="button button--critical" { "Remove passkey" }
            }
        }
    }
}

/// The dialogs of the section, for the Settings page.
pub fn protection_dialogs(context: &PageContext, protection: &Protection) -> Markup {
    html! {
        @if protection.authenticator_since.is_some() {
            (form_dialog(
                AUTHENTICATOR_OFF_DIALOG,
                "authenticator-off-title",
                protection.authenticator_form.has_errors(),
                authenticator_off_form(context, protection.authenticator_form),
            ))
        }
        // Its form is sent by the page script, which puts a refusal in place.
        (form_dialog(
            ADD_PASSKEY_DIALOG,
            "add-passkey-title",
            false,
            add_passkey_form(context, &FormState::default()),
        ))
        dialog class="dialog dialog--sm" id=(REMOVE_PASSKEY_DIALOG) aria-labelledby="remove-passkey-title" data-dialog-reset
            data-dialog-return="/settings" data-open[protection.removing.is_some()] {
            div data-fragment {
                @if let Some((passkey, form)) = protection.removing { (remove_passkey_form(context, passkey, form)) }
            }
        }
    }
}

/// A QR code, drawn by the server as one SVG path the stylesheet colours:
/// dark on light whatever the theme, which is what a camera reads.
fn qr_code(text: &str, label: &str) -> Markup {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return html! {};
    };
    // The margin a reader needs around the code, in modules.
    const QUIET: usize = 4;
    let width = code.width();
    let modules = code.to_colors();
    let mut path = String::new();
    for (y, row) in modules.chunks(width).enumerate() {
        let mut x = 0;
        while x < width {
            let start = x;
            while x < width && row[x] == qrcode::Color::Dark {
                x += 1;
            }
            if x > start {
                // One rectangle for a run of dark modules.
                let _ = write!(
                    path,
                    "M{} {}h{}v1h-{}z",
                    start + QUIET,
                    y + QUIET,
                    x - start,
                    x - start
                );
            }
            x += 1;
        }
    }
    let size = width + 2 * QUIET;
    html! {
        svg class="qr" viewBox=(format!("0 0 {size} {size}")) role="img" aria-label=(label) {
            path d=(path);
        }
    }
}

/// A secret as an app shows it and as people type it: in groups of four.
fn spaced(secret: &str) -> String {
    secret
        .as_bytes()
        .chunks(4)
        .map(|group| String::from_utf8_lossy(group))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `GET /settings/two-factor`: the secret for the app, as a QR code and as
/// text, and the form that proves the app has it.
pub fn authenticator_setup_page(
    context: &PageContext,
    account: &str,
    secret: &str,
    form: &FormState,
) -> Markup {
    let uri = totp::provisioning_uri(ISSUER, account, secret);
    let code_error = form.error("code");
    let content = html! {
        div class="page page--narrow" {
            header class="page-header" {
                div class="page-header__text" {
                    h1 class="page-header__title" { "Set up an authenticator app" }
                    p class="page-header__subtitle" {
                        "Once it is on, signing in asks for your password, then for a code of the app."
                    }
                }
            }
            section class="card" aria-label="Authenticator app" {
                form method="post" action="/settings/two-factor" {
                    (context.csrf_field())
                    div class="card__body" {
                        ol class="steps steps--detailed" aria-label="Authenticator app setup" {
                            li class="step" {
                                div class="step__body" {
                                    h3 class="step__title" { "Scan this code with your app" }
                                    div class="stack" {
                                        p class="text-secondary" { "1Password, Google Authenticator, Authy and most password managers read it." }
                                        (qr_code(&uri, "QR code of the authenticator secret"))
                                        p class="text-secondary" { "Or type this key in the app:" }
                                        div class="copy-field copy-field--wrap" {
                                            span class="copy-field__value" id="authenticator-secret" { (spaced(secret)) }
                                            button type="button" class="icon-button" data-copy=(secret) aria-label="Copy the key" { (icon("copy")) (icon("check")) }
                                        }
                                    }
                                }
                            }
                            li class="step" {
                                div class="step__body" {
                                    h3 class="step__title" { "Type the code the app shows" }
                                    div class="field" {
                                        label class="field__label" for="authenticator-code" { "6-digit code" }
                                        input class="input" id="authenticator-code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code"
                                            maxlength="16" required autofocus[code_error.is_some() || !form.has_errors()]
                                            aria-invalid=[code_error.map(|_| "true")] aria-describedby=[code_error.map(|_| "authenticator-code-error")];
                                        @if let Some(message) = code_error { p class="field__error" id="authenticator-code-error" { (message) } }
                                    }
                                }
                            }
                            li class="step" {
                                div class="step__body" {
                                    h3 class="step__title" { "Confirm it is you" }
                                    (confirm_password(form, "authenticator-password", form.error("currentPassword").is_some()))
                                }
                            }
                        }
                    }
                    footer class="card__footer" {
                        a class="button button--secondary" href="/settings" { "Cancel" }
                        button type="submit" class="button button--primary" { "Turn on" }
                    }
                }
            }
        }
    };
    app_page(context, "Authenticator app", content, html! {})
}

/// `GET /settings/recovery-codes`: the codes that were just made, shown
/// this once, or the form that makes new ones.
pub fn recovery_codes_page(
    context: &PageContext,
    codes: Option<&[String]>,
    left: i64,
    form: &FormState,
) -> Markup {
    let content = html! {
        div class="page page--narrow" {
            header class="page-header" {
                div class="page-header__text" {
                    h1 class="page-header__title" { "Recovery codes" }
                    p class="page-header__subtitle" {
                        "Each code signs you in once, in place of your authenticator app or your passkey."
                    }
                }
            }
            @if let Some(codes) = codes {
                section class="card" aria-label="Your recovery codes" {
                    div class="card__body" {
                        div class="stack" {
                            div class="banner banner--warning" role="status" {
                                (icon("triangle-alert"))
                                div class="banner__content" {
                                    p class="banner__title" { "Save these codes now" }
                                    p { "They are shown this once. Keep them where you keep your passwords, not on the device that holds your app or your passkey." }
                                }
                            }
                            div class="code-block" {
                                div class="code-block__header" {
                                    span class="code-block__title" { "MyMCPs recovery codes" }
                                    button type="button" class="icon-button" data-copy-target="#recovery-codes" aria-label="Copy the codes" { (icon("copy")) (icon("check")) }
                                }
                                pre class="code-block__body" id="recovery-codes" { (codes.join("\n")) }
                            }
                        }
                    }
                    footer class="card__footer" {
                        a class="button button--primary" href="/settings" { "I saved them" }
                    }
                }
            } @else {
                section class="card" aria-label="New recovery codes" {
                    form method="post" action="/settings/recovery-codes" {
                        (context.csrf_field())
                        div class="card__body" {
                            div class="stack" {
                                p class="text-secondary" {
                                    (left) " of " (RECOVERY_CODES) " codes are left. New codes replace them: the ones you saved stop working."
                                }
                                (confirm_password(form, "recovery-password", true))
                            }
                        }
                        footer class="card__footer" {
                            a class="button button--secondary" href="/settings" { "Cancel" }
                            button type="submit" class="button button--primary" { "Make new codes" }
                        }
                    }
                }
            }
        }
    };
    app_page(context, "Recovery codes", content, html! {})
}

/// The form a passkey signs in with. It is hidden: the page script shows
/// it where the browser has passkeys, and sends it.
pub fn passkey_sign_in(context: &PageContext, label: &str, primary: bool) -> Markup {
    html! {
        form class="form" method="post" action="/login/passkey" data-passkey="get" data-passkey-options="/login/passkey/options" hidden {
            (context.csrf_field())
            button type="submit" class=(if primary { "button button--primary button--block" } else { "button button--block" }) {
                (icon("key-round")) (label)
            }
        }
    }
}

/// What the second step of a sign-in can be answered with.
#[derive(Debug, Clone, Copy)]
pub struct SecondStep<'a> {
    /// The address of the account that typed its password.
    pub email: &'a str,
    pub authenticator: bool,
    pub passkeys: bool,
}

/// `GET /login/two-factor`: after the password, the code of the app, a
/// passkey, or a recovery code.
pub fn second_step_page(context: &PageContext, step: SecondStep, form: &FormState) -> Markup {
    let error = form.error("code");
    let (label, help) = if step.authenticator {
        (
            "Authentication code",
            "The 6 digits your authenticator app shows, or one of your recovery codes.",
        )
    } else {
        (
            "Recovery code",
            "One of the codes you saved when you added your passkey.",
        )
    };
    let content = html! {
        section class="auth-card control-lg" aria-labelledby="second-step-title" {
            (logo())
            div class="auth-card__heading" {
                h1 class="auth-card__title" id="second-step-title" { "Confirm it is you" }
                p class="auth-card__subtitle" {
                    "The password of " (step.email) " is right. "
                    @if step.authenticator && step.passkeys {
                        "Use a passkey, or type a code of your authenticator app."
                    } @else if step.authenticator {
                        "Type a code of your authenticator app."
                    } @else {
                        "Use your passkey to sign in."
                    }
                }
            }
            @if step.passkeys { (passkey_sign_in(context, "Use a passkey", !step.authenticator)) }
            form class="form" method="post" action="/login/two-factor" {
                (context.csrf_field())
                div class="stack" {
                    (form_banner(form))
                    div class="field" {
                        label class="field__label" for="second-step-code" { (label) }
                        input class="input" id="second-step-code" name="code" type="text" autocomplete="one-time-code" autocapitalize="none"
                            spellcheck="false" maxlength="64" required autofocus[step.authenticator]
                            aria-invalid=[error.map(|_| "true")] aria-describedby="second-step-code-help";
                        p class="field__help" id="second-step-code-help" { (help) }
                    }
                }
                div class="stack gap-300" {
                    button type="submit" class=(if step.authenticator { "button button--primary button--block" } else { "button button--block" }) { "Verify" }
                    a class="button button--block" href="/login" { "Use another account" }
                }
            }
        }
    };
    auth_page(context, "Confirm it is you", content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_qr_code_as_one_path_with_a_margin() {
        let drawn = qr_code(
            "otpauth://totp/MyMCPs:ada@example.com?secret=MZXW6YTBOI",
            "QR",
        )
        .into_string();
        assert!(drawn.starts_with("<svg class=\"qr\" viewBox=\"0 0 "));
        assert!(drawn.contains("role=\"img\" aria-label=\"QR\"><path d=\"M4 4h7v1h-7z"));
        // No style and no colour of its own: the stylesheet has both.
        assert!(!drawn.contains("style") && !drawn.contains("fill"));
        assert_eq!(drawn.matches("<path").count(), 1);
    }

    #[test]
    fn writes_a_secret_in_groups_of_four() {
        assert_eq!(spaced("MZXW6YTBOIMZXW6Y"), "MZXW 6YTB OIMZ XW6Y");
        assert_eq!(spaced("MZXW6"), "MZXW 6");
        assert_eq!(spaced(""), "");
    }
}
