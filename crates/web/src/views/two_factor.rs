//! Two-step verification and passkeys: the section of the Settings page and
//! its dialogs, the setup of an authenticator app, the recovery codes, and
//! the second step of a sign-in.

use std::fmt::Write as _;

use maud::{Markup, html};
use mymcps_core::models::UserPasskey;
use mymcps_core::two_factor::{RECOVERY_CODE_COUNT, TotpKey, TwoFactorStatus};
use qrcodegen::{QrCode, QrCodeEcc};

use crate::forms::FormState;
use crate::views::auth::form_banner;
use crate::views::icon::{icon, logo};
use crate::views::settings::{
    FieldError, dialog_footer, dialog_header, form_dialog, password_field, password_field_with_help,
};
use crate::views::shell::{PageContext, app_page, auth_page};
use crate::views::{date, relative_time};

pub const SETUP_TOTP_DIALOG: &str = "setup-totp";
pub const DISABLE_TOTP_DIALOG: &str = "disable-totp";
pub const REGENERATE_CODES_DIALOG: &str = "regenerate-recovery-codes";
pub const ADD_PASSKEY_DIALOG: &str = "add-passkey";

pub fn rename_passkey_dialog_id(id: i64) -> String {
    format!("rename-passkey-{id}")
}

pub fn remove_passkey_dialog_id(id: i64) -> String {
    format!("remove-passkey-{id}")
}

/// What the Settings page shows of two-step verification.
pub struct SecurityView<'a> {
    pub status: TwoFactorStatus,
    pub passkeys: &'a [UserPasskey],
    /// Whether `APP_URL` makes a WebAuthn relying party.
    pub passkeys_available: bool,
    /// The dialog a post without the page's script refused, by id, and
    /// the form it refused.
    pub refused: Option<(&'a str, &'a FormState)>,
}

impl SecurityView<'_> {
    fn form(&self, dialog: &str) -> FormState {
        match self.refused {
            Some((refused, form)) if refused == dialog => form.clone(),
            _ => FormState::default(),
        }
    }
}

/// A dialog footer whose action is destructive.
fn critical_footer(submit: &str) -> Markup {
    html! {
        div class="dialog__footer" {
            button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
            button type="submit" class="button button--critical" { (submit) }
        }
    }
}

fn passkey_name_field(form: &FormState, id: &str, value: &str, autofocus: bool) -> Markup {
    let error = FieldError::of(form, "name", id);
    let help_id = format!("{id}-help");
    html! {
        div class="field" {
            label class="field__label" for=(id) { "Name" }
            input class="input" id=(id) name="name" type="text" value=(value) maxlength="60" autocomplete="off" required
                autofocus[autofocus] aria-invalid=[error.invalid()] aria-describedby=[error.described_by(Some(&help_id))];
            (error.line())
            p class="field__help" id=(help_id) { "To tell your passkeys apart, such as “MacBook” or “YubiKey”." }
        }
    }
}

/// The form of the "Add a passkey" dialog. The page's script asks the
/// server for the options with the name and the password, lets the browser
/// create the passkey, then sends it.
pub fn add_passkey_form(context: &PageContext, form: &FormState) -> Markup {
    let fresh = !form.has_errors();
    html! {
        form method="post" action="/settings/passkeys" data-async data-passkey="register"
            data-passkey-options="/settings/passkeys/options" {
            (context.csrf_field())
            input type="hidden" name="credential";
            (dialog_header("add-passkey-title", "Add a passkey", "Your device asks for your fingerprint, face, or screen lock"))
            div class="dialog__body" {
                @if let Some(message) = form.error("credential") {
                    div class="banner banner--critical" role="alert" {
                        (icon("circle-x"))
                        div class="banner__content" { p class="banner__title" { (message) } }
                    }
                }
                (passkey_name_field(form, "passkey-name", &form.old("name"), fresh))
                (password_field(form, "passkey-current-password", "currentPassword", "Current password", "current-password", false))
            }
            (dialog_footer("Add passkey"))
        }
    }
}

pub fn rename_passkey_form(
    context: &PageContext,
    passkey: &UserPasskey,
    form: &FormState,
) -> Markup {
    let fresh = !form.has_errors();
    let name = if fresh {
        passkey.name.clone()
    } else {
        form.old("name")
    };
    let title_id = format!("{}-title", rename_passkey_dialog_id(passkey.id));
    html! {
        form method="post" action=(format!("/settings/passkeys/{}?_method=PATCH", passkey.id)) data-async {
            (context.csrf_field())
            (dialog_header(&title_id, "Rename passkey", "The name only helps you tell your passkeys apart"))
            div class="dialog__body" {
                (passkey_name_field(form, &format!("passkey-{}-name", passkey.id), &name, fresh))
            }
            (dialog_footer("Save name"))
        }
    }
}

pub fn remove_passkey_form(
    context: &PageContext,
    passkey: &UserPasskey,
    form: &FormState,
) -> Markup {
    let fresh = !form.has_errors();
    let title_id = format!("{}-title", remove_passkey_dialog_id(passkey.id));
    html! {
        form method="post" action=(format!("/settings/passkeys/{}?_method=DELETE", passkey.id)) data-async {
            (context.csrf_field())
            (dialog_header(&title_id, "Remove passkey", &format!("“{}” will no longer sign you in", passkey.name)))
            div class="dialog__body" {
                (password_field(form, &format!("passkey-{}-current-password", passkey.id), "currentPassword", "Current password", "current-password", fresh))
            }
            (critical_footer("Remove passkey"))
        }
    }
}

pub fn setup_totp_form(context: &PageContext, form: &FormState) -> Markup {
    let fresh = !form.has_errors();
    html! {
        form method="post" action="/settings/two-factor/totp" data-async {
            (context.csrf_field())
            (dialog_header("setup-totp-title", "Set up an authenticator app", "Confirm it is you, then scan a QR code with the app"))
            div class="dialog__body" {
                (password_field(form, "totp-current-password", "currentPassword", "Current password", "current-password", fresh))
            }
            (dialog_footer("Continue"))
        }
    }
}

pub fn disable_totp_form(context: &PageContext, form: &FormState) -> Markup {
    let fresh = !form.has_errors();
    html! {
        form method="post" action="/settings/two-factor/totp?_method=DELETE" data-async {
            (context.csrf_field())
            (dialog_header("disable-totp-title", "Turn off the authenticator app", "Its codes will no longer be asked for, nor accepted"))
            div class="dialog__body" {
                (password_field(form, "disable-totp-current-password", "currentPassword", "Current password", "current-password", fresh))
            }
            (critical_footer("Turn off"))
        }
    }
}

pub fn regenerate_codes_form(context: &PageContext, form: &FormState) -> Markup {
    let fresh = !form.has_errors();
    html! {
        form method="post" action="/settings/two-factor/recovery-codes" data-async {
            (context.csrf_field())
            (dialog_header("regenerate-recovery-codes-title", "Generate new recovery codes", "Your current recovery codes stop working"))
            div class="dialog__body" {
                (password_field_with_help(
                    form,
                    "recovery-current-password",
                    "currentPassword",
                    "Current password",
                    "current-password",
                    fresh,
                    Some("The new codes are shown once, on the next page."),
                ))
            }
            (dialog_footer("Generate codes"))
        }
    }
}

fn passkey_row(passkey: &UserPasskey) -> Markup {
    html! {
        div class="list-row" {
            span class="tile" { (icon("key-round")) }
            span class="list-row__text" {
                span class="list-row__title" { (passkey.name) }
                span class="list-row__subtitle" {
                    "Added " (date(passkey.created_at)) " · "
                    @if let Some(used) = passkey.last_used_at { "Last used " (relative_time(used)) } @else { "Never used" }
                }
            }
            span class="row-actions" {
                button type="button" class="button" data-dialog-open=(format!("#{}", rename_passkey_dialog_id(passkey.id))) { "Rename" }
                button type="button" class="button" data-dialog-open=(format!("#{}", remove_passkey_dialog_id(passkey.id))) { "Remove" }
            }
        }
    }
}

fn on_badge(on: bool) -> Markup {
    html! {
        @if on { span class="badge badge--success" { "On" } } @else { span class="badge" { "Off" } }
    }
}

/// The "Sign-in security" section of the Settings page.
pub fn security_section(view: &SecurityView) -> Markup {
    let status = view.status;
    html! {
        section class="section" aria-labelledby="security-title" {
            div class="section__intro" {
                h2 class="section__title" id="security-title" { "Sign-in security" }
                p class="section__description" {
                    "A passkey signs you in on its own. Once you have a passkey or an authenticator app, your password alone no longer signs you in."
                }
            }
            div class="card" {
                @if !view.passkeys_available {
                    div class="card__body" {
                        div class="banner banner--warning" role="status" {
                            (icon("triangle-alert"))
                            div class="banner__content" {
                                p class="banner__title" { "Passkeys need APP_URL" }
                                p { "Set APP_URL to the public HTTPS origin of this instance, then redeploy. Passkeys are bound to that address." }
                            }
                        }
                    }
                }
                div class="detail-row" {
                    div class="detail-row__text" {
                        span class="detail-row__label" { "Passkeys " (on_badge(status.passkeys > 0)) }
                        span class="detail-row__value" { "Sign in with your fingerprint, face, screen lock, or security key instead of your password." }
                    }
                    @if view.passkeys_available {
                        button type="button" class="button button--secondary" data-dialog-open=(format!("#{ADD_PASSKEY_DIALOG}"))
                            data-passkey-supported hidden { "Add passkey" }
                    }
                }
                @for passkey in view.passkeys { (passkey_row(passkey)) }
                div class="detail-row" {
                    div class="detail-row__text" {
                        span class="detail-row__label" { "Authenticator app " (on_badge(status.totp)) }
                        span class="detail-row__value" {
                            @if status.totp { "A 6-digit code of the app follows your password when you sign in." }
                            @else { "Add a 6-digit code from an app such as 1Password, Bitwarden, or Google Authenticator after your password." }
                        }
                    }
                    @if status.totp {
                        button type="button" class="button button--secondary" data-dialog-open=(format!("#{DISABLE_TOTP_DIALOG}")) { "Turn off" }
                    } @else {
                        button type="button" class="button button--secondary" data-dialog-open=(format!("#{SETUP_TOTP_DIALOG}")) { "Set up" }
                    }
                }
                @if status.is_enabled() {
                    div class="detail-row" {
                        div class="detail-row__text" {
                            span class="detail-row__label" { "Recovery codes" }
                            span class="detail-row__value" {
                                (status.recovery_codes) " of " (RECOVERY_CODE_COUNT) " left. Each one replaces the second step once, when your passkeys and app are out of reach."
                            }
                        }
                        button type="button" class="button button--secondary" data-dialog-open=(format!("#{REGENERATE_CODES_DIALOG}")) { "New codes" }
                    }
                }
            }
        }
    }
}

/// The dialogs of [`security_section`], for the overlays of the page.
pub fn security_dialogs(context: &PageContext, view: &SecurityView) -> Markup {
    let open = |dialog: &str| view.refused.is_some_and(|(refused, _)| refused == dialog);
    html! {
        @if view.passkeys_available {
            (form_dialog(ADD_PASSKEY_DIALOG, "add-passkey-title", open(ADD_PASSKEY_DIALOG), add_passkey_form(context, &view.form(ADD_PASSKEY_DIALOG))))
        }
        @for passkey in view.passkeys {
            @let rename = rename_passkey_dialog_id(passkey.id);
            @let remove = remove_passkey_dialog_id(passkey.id);
            (form_dialog(&rename, &format!("{rename}-title"), open(&rename), rename_passkey_form(context, passkey, &view.form(&rename))))
            (form_dialog(&remove, &format!("{remove}-title"), open(&remove), remove_passkey_form(context, passkey, &view.form(&remove))))
        }
        @if view.status.totp {
            (form_dialog(DISABLE_TOTP_DIALOG, "disable-totp-title", open(DISABLE_TOTP_DIALOG), disable_totp_form(context, &view.form(DISABLE_TOTP_DIALOG))))
        } @else {
            (form_dialog(SETUP_TOTP_DIALOG, "setup-totp-title", open(SETUP_TOTP_DIALOG), setup_totp_form(context, &view.form(SETUP_TOTP_DIALOG))))
        }
        @if view.status.is_enabled() {
            (form_dialog(REGENERATE_CODES_DIALOG, "regenerate-recovery-codes-title", open(REGENERATE_CODES_DIALOG), regenerate_codes_form(context, &view.form(REGENERATE_CODES_DIALOG))))
        }
    }
}

/// `text` as a QR code, drawn by the server: one path of unit squares on a
/// light square, four modules of quiet zone around it.
pub fn qr_code(text: &str, label: &str) -> Markup {
    let Ok(qr) = QrCode::encode_text(text, QrCodeEcc::Medium) else {
        return html! {};
    };
    const QUIET_ZONE: i32 = 4;
    let size = qr.size();
    let total = size + 2 * QUIET_ZONE;
    let mut modules = String::new();
    for y in 0..size {
        for x in 0..size {
            if qr.get_module(x, y) {
                let _ = write!(modules, "M{} {}h1v1h-1z", x + QUIET_ZONE, y + QUIET_ZONE);
            }
        }
    }
    html! {
        svg class="qr-code" viewBox=(format!("0 0 {total} {total}")) role="img" aria-label=(label) shape-rendering="crispEdges" {
            rect class="qr-code__background" width=(total) height=(total) {}
            path class="qr-code__modules" d=(modules) {}
        }
    }
}

/// The secret in groups of four, as it is easier to type.
fn grouped_secret(secret: &str) -> String {
    secret
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The setup of an authenticator app: the QR code, the key, and the code
/// that confirms both.
pub fn totp_setup_page(context: &PageContext, key: &TotpKey, form: &FormState) -> Markup {
    let error = FieldError::of(form, "code", "totp-code");
    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                nav class="breadcrumb" aria-label="Breadcrumb" { a href="/settings" { "Settings" } (icon("chevron-right")) span aria-current="page" { "Authenticator app" } }
                h1 class="page-header__title" { "Set up an authenticator app" }
                p class="page-header__subtitle" { "Use an app such as 1Password, Bitwarden, Google Authenticator, or Aegis." }
            }
        }
        section class="card" aria-label="Authenticator app setup" {
            div class="card__body" {
                ol class="steps steps--detailed" aria-label="Authenticator app setup" {
                    li class="step" {
                        div class="step__body" {
                            h3 class="step__title" { "Scan this QR code with the app" }
                            (qr_code(&key.otpauth_url(), "QR code of the authenticator app key"))
                        }
                    }
                    li class="step" {
                        div class="step__body" {
                            h3 class="step__title" { "Or type this key in the app" }
                            div class="copy-field copy-field--wrap" {
                                span class="copy-field__value" id="totp-secret" { (grouped_secret(&key.base32())) }
                                button type="button" class="icon-button" data-copy=(key.base32()) aria-label="Copy key" { (icon("copy")) (icon("check")) }
                            }
                        }
                    }
                    li class="step" aria-current="step" {
                        div class="step__body" {
                            h3 class="step__title" { "Enter the code the app shows" }
                            form class="form" method="post" action="/settings/two-factor/totp/confirm" {
                                (context.csrf_field())
                                div class="field" {
                                    label class="field__label" for="totp-code" { "Code" }
                                    input class="input input--mono" id="totp-code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code"
                                        maxlength="7" required autofocus aria-invalid=[error.invalid()] aria-describedby=[error.described_by(None)];
                                    (error.line())
                                }
                                div class="cluster" {
                                    button type="submit" class="button button--primary" { "Turn on" }
                                    a class="button button--secondary" href="/settings" { "Cancel" }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    app_page(context, "Authenticator app", content, html! {})
}

/// Recovery codes that were just generated: the only time they are shown.
pub fn recovery_codes_page(context: &PageContext, codes: &[String]) -> Markup {
    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                nav class="breadcrumb" aria-label="Breadcrumb" { a href="/settings" { "Settings" } (icon("chevron-right")) span aria-current="page" { "Recovery codes" } }
                h1 class="page-header__title" { "Save your recovery codes" }
                p class="page-header__subtitle" { "Each code replaces the second step of a sign-in once, if you lose your passkeys and your authenticator app." }
            }
        }
        div class="banner banner--warning" role="status" {
            (icon("triangle-alert"))
            div class="banner__content" {
                p class="banner__title" { "They are shown once" }
                p { "Keep them in a password manager or print them. Generating new codes in Settings replaces these." }
            }
        }
        section class="card" aria-label="Recovery codes" {
            div class="card__body" {
                div class="code-block" {
                    div class="code-block__header" {
                        span class="code-block__title" { (codes.len()) " recovery codes" }
                        button type="button" class="icon-button" data-copy-target="#recovery-codes" aria-label="Copy recovery codes" { (icon("copy")) (icon("check")) }
                    }
                    pre class="code-block__body" id="recovery-codes" { (codes.join("\n")) }
                }
            }
            footer class="card__footer" {
                a class="button button--primary" href="/settings" { "I saved them" }
            }
        }
    };
    app_page(context, "Recovery codes", content, html! {})
}

/// The ways to prove the second step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMethod {
    Passkey,
    Totp,
    Recovery,
}

impl VerifyMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passkey => "passkey",
            Self::Totp => "totp",
            Self::Recovery => "recovery",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "passkey" => Some(Self::Passkey),
            "totp" => Some(Self::Totp),
            "recovery" => Some(Self::Recovery),
            _ => None,
        }
    }

    fn switch_label(self) -> &'static str {
        match self {
            Self::Passkey => "Use a passkey",
            Self::Totp => "Use your authenticator app",
            Self::Recovery => "Use a recovery code",
        }
    }
}

/// What the second step of a sign-in shows.
pub struct VerifyPage<'a> {
    pub method: VerifyMethod,
    /// The other methods the account has.
    pub others: &'a [VerifyMethod],
    pub form: &'a FormState,
}

fn code_form(context: &PageContext, action: &str, form: &FormState, recovery: bool) -> Markup {
    let error = FieldError::of(form, "code", "code");
    html! {
        form class="form" method="post" action=(action) {
            (context.csrf_field())
            div class="field" {
                @if recovery {
                    label class="field__label" for="code" { "Recovery code" }
                    input class="input input--mono" id="code" name="code" type="text" autocomplete="off" autocapitalize="off" spellcheck="false"
                        maxlength="24" required autofocus aria-invalid=[error.invalid()] aria-describedby=[error.described_by(None)];
                } @else {
                    label class="field__label" for="code" { "Code" }
                    input class="input input--mono" id="code" name="code" type="text" inputmode="numeric" autocomplete="one-time-code"
                        maxlength="7" required autofocus aria-invalid=[error.invalid()] aria-describedby=[error.described_by(None)];
                }
                (error.line())
            }
            button type="submit" class="button button--primary button--block" { "Verify" }
        }
    }
}

pub fn verify_page(context: &PageContext, view: &VerifyPage) -> Markup {
    let subtitle = match view.method {
        VerifyMethod::Passkey => "Confirm it is you with a passkey of this account.",
        VerifyMethod::Totp => "Enter the 6-digit code your authenticator app shows for MyMCPs.",
        VerifyMethod::Recovery => {
            "Enter one of the recovery codes you saved. Each code works once."
        }
    };
    // The banner says what was refused, except for a code: the field does.
    let banner_form = if view.method == VerifyMethod::Passkey {
        view.form.clone()
    } else {
        FormState::default()
    };
    let content = html! {
        section class="auth-card control-lg" aria-labelledby="verify-title" {
            (logo())
            div class="auth-card__heading" {
                h1 class="auth-card__title" id="verify-title" { "Two-step verification" }
                p class="auth-card__subtitle" { (subtitle) }
            }
            @match view.method {
                VerifyMethod::Passkey => {
                    form class="form" method="post" action="/login/verify/passkey" data-passkey="authenticate"
                        data-passkey-options="/login/verify/passkey/options" {
                        (context.csrf_field())
                        input type="hidden" name="credential";
                        (form_banner(&banner_form))
                        p class="text-secondary" data-passkey-unsupported { "This browser cannot use passkeys. Choose another way below." }
                        button type="submit" class="button button--primary button--block" data-passkey-supported hidden { (icon("key-round")) "Use a passkey" }
                    }
                }
                VerifyMethod::Totp => { (code_form(context, "/login/verify/totp", view.form, false)) }
                VerifyMethod::Recovery => { (code_form(context, "/login/verify/recovery", view.form, true)) }
            }
            div class="stack gap-300" {
                @for other in view.others {
                    a class="button button--secondary button--block" href=(format!("/login/verify?method={}", other.as_str())) { (other.switch_label()) }
                }
                a class="button button--block" href="/login" { "Back to sign in" }
            }
        }
    };
    auth_page(context, "Two-step verification", content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_code_is_one_path_on_its_quiet_zone() {
        let markup = qr_code("otpauth://totp/MyMCPs:ada?secret=JBSWY3DP", "QR").into_string();
        assert!(markup.starts_with("<svg class=\"qr-code\" viewBox=\"0 0 "));
        assert_eq!(markup.matches("<path").count(), 1);
        assert!(
            markup.contains("M4 4h1v1h-1z"),
            "the finder pattern starts at the corner"
        );
        assert!(!markup.contains("style"));
    }

    #[test]
    fn the_secret_is_shown_in_groups_of_four() {
        assert_eq!(grouped_secret("JBSWY3DPEHPK3PXP"), "JBSW Y3DP EHPK 3PXP");
    }
}
