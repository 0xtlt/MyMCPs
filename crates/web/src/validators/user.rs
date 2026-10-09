//! Accounts: sign-in, onboarding, invites, and the settings of a user.
//! (`app/validators/user.ts`)

use std::sync::LazyLock;

use mymcps_vine as vine;

use crate::validators::cron::is_valid_five_field_cron;

fn email() -> vine::VineString {
    vine::string().email().max_length(254)
}

fn password() -> vine::VineString {
    vine::string().min_length(8).max_length(32)
}

fn five_field_cron() -> vine::Rule {
    vine::rule(|value, field| {
        let Some(expression) = value.as_str() else {
            return;
        };
        if !is_valid_five_field_cron(expression) {
            field.report(
                "The {{ field }} field must be a valid 5-field cron expression",
                "cron",
            );
        }
    })
}

/// Change the signed-in user's email address. The address must not belong
/// to another user: the caller answers the `unique` check.
pub static UPDATE_EMAIL_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "email" => email().use_rule(vine::lucid::unique()),
        "currentPassword" => vine::string().min_length(1),
    })
});

/// Change the signed-in user's password.
pub static UPDATE_PASSWORD_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "currentPassword" => vine::string().min_length(1),
        "newPassword" => password().confirmed("passwordConfirmation"),
        "passwordConfirmation" => vine::string(),
    })
});

/// Server-side account recovery without the current password.
pub static RESET_PASSWORD_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "newPassword" => password().confirmed("passwordConfirmation"),
        "passwordConfirmation" => vine::string(),
    })
});

pub static UPDATE_MCP_LOGGING_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "gatewayToolMode" => vine::enum_(["eager", "lazy"]),
        "mcpLogLevel" => vine::enum_(["off", "metadata", "arguments", "responses"]),
        "mcpLogRetentionDays" => vine::number().without_decimals().min(1).max(365),
        // A switch submits "on" when checked, and nothing when unchecked.
        "mcpAutoUpdateEnabled" => vine::boolean().optional(),
        "mcpAutoUpdateCron" => vine::string().trim().max_length(64).optional().use_rule(five_field_cron()),
    })
});

/// First-run onboarding: create the instance admin.
pub static ONBOARDING_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "fullName" => vine::string().trim().min_length(1).max_length(120),
        "email" => email().use_rule(vine::lucid::unique()),
        "password" => password().confirmed("passwordConfirmation"),
        "passwordConfirmation" => vine::string(),
    })
});

/// Accept an invite and create a member account.
pub static ACCEPT_INVITE_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "fullName" => vine::string().trim().min_length(1).max_length(120),
        "password" => password().confirmed("passwordConfirmation"),
        "passwordConfirmation" => vine::string(),
    })
});

/// Admin creates an invite for an email address.
pub static CREATE_INVITE_VALIDATOR: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(vine::object! { "email" => email() }));

/// Login credentials.
pub static LOGIN_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "email" => email(),
        "password" => vine::string().min_length(1),
    })
});
