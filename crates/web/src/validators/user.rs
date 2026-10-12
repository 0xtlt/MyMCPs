//! Accounts and teams: sign-in, sign-up, onboarding, invites, members, and
//! the settings of a user. (`app/validators/user.ts`)

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
        "registrationEnabled" => vine::boolean().optional(),
    })
});

/// First-run onboarding: create the first super admin, who owns the Root Team.
pub static ONBOARDING_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "fullName" => vine::string().trim().min_length(1).max_length(120),
        "email" => email().use_rule(vine::lucid::unique()),
        "password" => password().confirmed("passwordConfirmation"),
        "passwordConfirmation" => vine::string(),
    })
});

/// Create an account on the sign-up page, while the instance lets anyone
/// register. The address must not belong to an account: the caller answers
/// the `unique` check.
pub static REGISTER_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "fullName" => vine::string().trim().min_length(1).max_length(120),
        "email" => email().use_rule(vine::lucid::unique()),
        "password" => password().confirmed("passwordConfirmation"),
        "passwordConfirmation" => vine::string(),
    })
});

/// What a team is called, on the form that creates one and the form that
/// renames it.
pub static TEAM_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "name" => vine::string().trim().min_length(1).max_length(120),
        "description" => vine::string().trim().max_length(500).optional(),
    })
});

/// The team to work in, from the team switcher.
pub static SWITCH_TEAM_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global()
        .create(vine::object! { "teamId" => vine::number().without_decimals().positive() })
});

/// What a member of a team becomes. Whether the person who asks may give
/// that role is for the handler to say.
pub static MEMBER_ROLE_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! { "role" => vine::enum_(["member", "admin", "owner"]) })
});

/// Delete an account, from the Users page of those who run the instance:
/// the password of the person who deletes it, to confirm it is them.
pub static DELETE_USER_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! { "currentPassword" => vine::string().min_length(1) })
});

/// Make an account a super admin or take that from it, from the Users page:
/// what it becomes, and the password of the person who decides.
pub static SUPER_ADMIN_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "superAdmin" => vine::boolean(),
        "currentPassword" => vine::string().min_length(1),
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

/// An administrator of a team invites an email address into it. Without a
/// role the person joins as a member.
pub static CREATE_INVITE_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "email" => email(),
        "role" => vine::enum_(["member", "admin", "owner"]).optional(),
    })
});

/// Login credentials.
pub static LOGIN_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "email" => email(),
        "password" => vine::string().min_length(1),
    })
});
