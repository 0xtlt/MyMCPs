//! The screens before sign-in: sign in, and the first-run onboarding.

use maud::{Markup, html};

use crate::forms::FormState;
use crate::views::icon::{icon, logo};
use crate::views::shell::{PageContext, auth_page};

/// The first error of a form, above its fields.
pub fn form_banner(form: &FormState) -> Markup {
    html! {
        @if let Some(message) = form.first_error() {
            div class="banner banner--critical" role="alert" {
                (icon("circle-x"))
                div class="banner__content" { p class="banner__title" { (message) } }
            }
        }
    }
}

/// A text field of an auth screen, with its error under it.
fn text_field(
    form: &FormState,
    id: &str,
    name: &str,
    label: &str,
    input_type: &str,
    autocomplete: &str,
) -> Markup {
    let error = form.error(name);
    let error_id = format!("{id}-error");
    html! {
        div class="field" {
            label class="field__label" for=(id) { (label) }
            input class="input" id=(id) name=(name) type=(input_type) autocomplete=(autocomplete) value=(form.old(name))
                aria-invalid=[error.map(|_| "true")] aria-describedby=[error.map(|_| error_id.as_str())];
            @if let Some(message) = error { p class="field__error" id=(error_id) { (message) } }
        }
    }
}

/// A password field with its show/hide button. Its value is never rendered.
pub(crate) fn password_field(
    form: &FormState,
    id: &str,
    name: &str,
    label: &str,
    autocomplete: &str,
    autofocus: bool,
) -> Markup {
    let error = form.error(name);
    let error_id = format!("{id}-error");
    html! {
        div class="field" {
            label class="field__label" for=(id) { (label) }
            div class="input-group" {
                input class="input-group__control" id=(id) name=(name) type="password" autocomplete=(autocomplete)
                    autofocus[autofocus] aria-invalid=[error.map(|_| "true")] aria-describedby=[error.map(|_| error_id.as_str())];
                button type="button" class="input-group__action" data-password-toggle=(format!("#{id}")) aria-pressed="false"
                    aria-label="Show password" data-label-pressed="Hide password" { (icon("eye")) (icon("eye-off")) }
            }
            @if let Some(message) = error { p class="field__error" id=(error_id) { (message) } }
        }
    }
}

pub fn login_page(context: &PageContext, form: &FormState) -> Markup {
    let content = html! {
        section class="auth-card control-lg" aria-labelledby="sign-in-title" {
            (logo())
            div class="auth-card__heading" {
                h1 class="auth-card__title" id="sign-in-title" { "Sign in" }
                p class="auth-card__subtitle" { "Access this self-hosted MyMCPs instance. New users join by invite only." }
            }
            form class="form" method="post" action="/login" {
                (context.csrf_field())
                div class="stack" {
                    (form_banner(form))
                    (text_field(form, "email", "email", "Email", "email", "username"))
                    // After a refusal the email is kept and the password is what to type again.
                    (password_field(form, "password", "password", "Password", "current-password", form.has_errors()))
                }
                button type="submit" class="button button--primary button--block" { "Sign in" }
            }
        }
    };
    auth_page(context, "Sign in", content)
}

pub fn onboarding_page(context: &PageContext, form: &FormState) -> Markup {
    let content = html! {
        section class="auth-card control-lg" aria-labelledby="onboarding-title" {
            (logo())
            div class="auth-card__heading" {
                h1 class="auth-card__title" id="onboarding-title" { "Set up MyMCPs" }
                p class="auth-card__subtitle" { "Create the admin account for this self-hosted instance." }
            }
            form class="form" method="post" action="/onboarding" {
                (context.csrf_field())
                div class="stack" {
                    (form_banner(form))
                    (text_field(form, "full-name", "fullName", "Full name", "text", "name"))
                    (text_field(form, "email", "email", "Email", "email", "email"))
                    (password_field(form, "password", "password", "Password", "new-password", false))
                    (password_field(form, "password-confirmation", "passwordConfirmation", "Confirm password", "new-password", false))
                }
                button type="submit" class="button button--primary button--block" { "Create admin" }
            }
        }
    };
    auth_page(context, "Set up MyMCPs", content)
}
