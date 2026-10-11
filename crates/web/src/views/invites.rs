//! Invites: the list and the dialogs of the Team page, and the page behind
//! an invite link, where someone joins a team with the account they have
//! or with one they make there.

use maud::{Markup, html};
use mymcps_core::models::{Invite, Team, TeamRole};

use crate::forms::FormState;
use crate::views::auth::form_banner;
use crate::views::icon::{icon, logo};
use crate::views::shell::{PageContext, auth_page};
use crate::views::teams::{card_header, role_badge};
use crate::views::{date, time_minute};

/// What a control that needs the public address says while there is none.
const NO_APP_URL: &str = "Set APP_URL to enable public links";

/// The address of an invite link, when `APP_URL` is set.
pub(crate) fn invite_url(context: &PageContext, invite: &Invite) -> Option<String> {
    context
        .app_url
        .as_ref()
        .map(|app_url| format!("{app_url}/invite/{}", invite.token))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InviteStatus {
    Pending,
    Accepted,
    Expired,
}

impl InviteStatus {
    fn of(invite: &Invite) -> Self {
        if invite.is_accepted() {
            Self::Accepted
        } else if invite.is_usable() {
            Self::Pending
        } else {
            Self::Expired
        }
    }

    fn badge(self) -> Markup {
        match self {
            Self::Pending => html! { span class="badge badge--info" { "Pending" } },
            Self::Accepted => html! { span class="badge badge--success" { "Accepted" } },
            Self::Expired => html! { span class="badge" { "Expired" } },
        }
    }

    /// What removing the invite changes, for the confirm prompt.
    fn removal(self, invite: &Invite) -> String {
        match self {
            Self::Pending => format!("The join link sent to {} stops working.", invite.email),
            Self::Accepted => {
                "The invite leaves this list. The account created with it is not affected.".into()
            }
            Self::Expired => {
                "The invite leaves this list. Its join link has already expired.".into()
            }
        }
    }
}

/// The form that removes an invite, around the button that sends it.
fn remove_invite_form(context: &PageContext, invite: &Invite, button: Markup) -> Markup {
    html! {
        form method="post" action=(format!("/invites/{}?_method=DELETE", invite.id))
            data-confirm=(InviteStatus::of(invite).removal(invite))
            data-confirm-title="Remove this invite?" data-confirm-label="Remove" data-confirm-tone="critical" {
            (context.csrf_field())
            (button)
        }
    }
}

fn row_button() -> Markup {
    html! { button type="submit" class="button" { "Remove" } }
}

fn menu_button(label: &str) -> Markup {
    html! {
        button type="submit" class="menu__item menu__item--critical" role="menuitem" { (icon("trash")) (label) }
    }
}

/// Invites: newest first. Only a pending invite has a link to copy.
pub(crate) fn invites(context: &PageContext, invites: &[Invite]) -> Markup {
    const SUBTITLE: &str = "One-time join links to this team, valid for 7 days";
    if invites.is_empty() {
        return html! {
            section class="card" aria-labelledby="invites-title" {
                (card_header("invites-title", "Invites", SUBTITLE))
                div class="empty-state" {
                    span class="empty-state__icon" { (icon("user-plus")) }
                    div class="empty-state__text" {
                        p class="empty-state__title" { "No invites yet" }
                        p class="empty-state__description" { "Create an invite link to add someone to this team." }
                    }
                }
            }
        };
    }

    html! {
        section class="card hide-mobile" aria-labelledby="invites-title" {
            (card_header("invites-title", "Invites", SUBTITLE))
            div class="table-scroll" {
                table class="table" {
                    thead {
                        tr { th { "Email" } th class="col-120" { "Role" } th class="col-180" { "Expires" } th class="col-120" { "Status" } th class="col-180 cell-end" { "Actions" } }
                    }
                    tbody {
                        @for invite in invites {
                            @let status = InviteStatus::of(invite);
                            tr {
                                td class="cell-strong" { (invite.email) }
                                td { (role_badge(invite.role)) }
                                td class="cell-secondary" { (time_minute(invite.expires_at)) }
                                td { (status.badge()) }
                                td class="cell-end" {
                                    span class="row-actions" {
                                        @if status == InviteStatus::Pending {
                                            @match invite_url(context, invite) {
                                                Some(url) => {
                                                    button type="button" class="button" data-copy=(url) { span data-copy-label { "Copy link" } }
                                                }
                                                None => {
                                                    button type="button" class="button" disabled title=(NO_APP_URL) { "Copy link" }
                                                }
                                            }
                                        }
                                        (remove_invite_form(context, invite, row_button()))
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        section class="card hide-desktop" aria-labelledby="invites-list-title" {
            (card_header("invites-list-title", "Invites", SUBTITLE))
            ul {
                @for invite in invites {
                    @let status = InviteStatus::of(invite);
                    @let menu = format!("invite-{}-menu", invite.id);
                    li class="list-row" {
                        span class="list-row__text" {
                            span class="list-row__title" { (invite.email) }
                            span class="list-row__subtitle" { "As " (invite.role.as_str()) " · expires " (date(invite.expires_at)) }
                        }
                        (status.badge())
                        @if status == InviteStatus::Pending {
                            @match invite_url(context, invite) {
                                Some(url) => {
                                    button type="button" class="icon-button" data-copy=(url) aria-label="Copy link" { (icon("copy")) (icon("check")) }
                                }
                                None => {
                                    button type="button" class="icon-button" disabled title=(NO_APP_URL) aria-label="Copy link" { (icon("copy")) }
                                }
                            }
                        }
                        button type="button" class="icon-button" popovertarget=(menu)
                            aria-label=(format!("Actions for {}", invite.email)) { (icon("ellipsis")) }
                        div class="menu" id=(menu) popover role="menu" {
                            (remove_invite_form(context, invite, menu_button("Remove invite")))
                        }
                    }
                }
            }
        }
    }
}

/// The form of the "Create invite" dialog: what its `[data-fragment]` holds,
/// and what a refused submission of the page's script gets back. `actor` is
/// what the person who invites is in the team: they give no role above it.
pub fn create_invite_form(context: &PageContext, form: &FormState, actor: TeamRole) -> Markup {
    let error = form.error("email");
    let role_error = form.error("role");
    let chosen = form.old("role");
    html! {
        form method="post" action="/invites" data-async {
            (context.csrf_field())
            div class="dialog__header" {
                div class="dialog__heading" {
                    h2 class="dialog__title" id="create-invite-title" { "Create invite" }
                    p class="dialog__subtitle" { "Send a one-time join link to this team" }
                }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="dialog__body" {
                div class="stack" {
                    div class="field" {
                        label class="field__label" for="invite-email" { "Email" }
                        // An invalid field takes the focus by itself.
                        input class="input" id="invite-email" name="email" type="email" placeholder="teammate@example.com"
                            value=(form.old("email")) autofocus[error.is_none()]
                            aria-invalid=[error.map(|_| "true")] aria-describedby=[error.map(|_| "invite-email-error")];
                        @if let Some(message) = error { p class="field__error" id="invite-email-error" { (message) } }
                    }
                    div class="field" {
                        label class="field__label" for="invite-role" { "Role" }
                        div class="select" {
                            select class="input" id="invite-role" name="role"
                                aria-invalid=[role_error.map(|_| "true")] aria-describedby="invite-role-help" {
                                @for role in TeamRole::ALL.iter().filter(|role| actor.may_grant(**role)) {
                                    option value=(role.as_str()) selected[chosen == role.as_str()] {
                                        (match role {
                                            TeamRole::Member => "Member",
                                            TeamRole::Admin => "Admin",
                                            TeamRole::Owner => "Owner",
                                        })
                                    }
                                }
                            }
                            (icon("chevron-down"))
                        }
                        @if let Some(message) = role_error { p class="field__error" { (message) } }
                        p class="field__help" id="invite-role-help" {
                            "A member uses the MCPs of the team. An admin also manages them and the members."
                        }
                    }
                }
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                button type="submit" class="button button--primary" { "Create invite" }
            }
        }
    }
}

/// The "Create invite" dialog of the Team page, open on what was refused.
pub(crate) fn create_invite_dialog(
    context: &PageContext,
    form: &FormState,
    actor: TeamRole,
    open: bool,
) -> Markup {
    html! {
        dialog class="dialog dialog--sm" id="create-invite" aria-labelledby="create-invite-title" data-dialog-reset data-open[open] {
            div data-fragment { (create_invite_form(context, form, actor)) }
        }
    }
}

/// The link of the invite that was just created, shown once.
pub(crate) fn invite_created_dialog(invite: &Invite, url: &str) -> Markup {
    html! {
        dialog class="dialog dialog--sm" id="invite-created" aria-labelledby="invite-created-title" data-open {
            div class="dialog__header" {
                div class="dialog__heading" {
                    h2 class="dialog__title" id="invite-created-title" { "Invite created" }
                    p class="dialog__subtitle" { "Send this one-time join link to " (invite.email) }
                }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="dialog__body" {
                div class="field" {
                    span class="field__label" id="invite-link-label" { "Join link" }
                    div class="copy-field" role="group" aria-labelledby="invite-link-label" {
                        span class="copy-field__value" id="invite-link" { (url) }
                        button type="button" class="icon-button" data-copy-target="#invite-link" aria-label="Copy link" autofocus { (icon("copy")) (icon("check")) }
                    }
                    p class="field__help" {
                        "Valid for 7 days, until " (date(invite.expires_at)) ". You can copy it again from the Invites list."
                    }
                }
            }
            div class="dialog__footer" {
                button type="button" class="button button--primary" data-dialog-close { "Done" }
            }
        }
    }
}

/// A text field of the invite screen, with its error under it.
fn text_field(
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
            input class="input" id=(id) name=(name) type="text" autocomplete=(autocomplete) value=(form.old(name))
                autofocus[autofocus] aria-invalid=[error.map(|_| "true")] aria-describedby=[error.map(|_| error_id.as_str())];
            @if let Some(message) = error { p class="field__error" id=(error_id) { (message) } }
        }
    }
}

/// A password field for a password that is being chosen.
fn password_field(form: &FormState, id: &str, name: &str, label: &str, autofocus: bool) -> Markup {
    super::auth::password_field(form, id, name, label, "new-password", autofocus)
}

/// `GET /invite/{token}` for an address that has no account: the invited
/// person chooses a name and a password. The email comes from the invite,
/// there is no email field.
pub fn accept_page(
    context: &PageContext,
    invite: &Invite,
    team: &Team,
    form: &FormState,
) -> Markup {
    // After a refusal, the first field to correct takes the focus.
    let first_invalid = ["fullName", "password", "passwordConfirmation"]
        .into_iter()
        .find(|name| form.error(name).is_some());
    let focused = |name: &str| first_invalid == Some(name);
    let content = html! {
        section class="auth-card control-lg" aria-labelledby="invite-title" {
            (logo())
            div class="auth-card__heading" {
                h1 class="auth-card__title" id="invite-title" { "Join " (team.name) }
                p class="auth-card__subtitle" { "You were invited as " (invite.email) }
            }
            form class="form" method="post" action=(format!("/invite/{}", invite.token)) {
                (context.csrf_field())
                div class="stack" {
                    (form_banner(form))
                    (text_field(form, "full-name", "fullName", "Full name", "name", focused("fullName")))
                    (password_field(form, "password", "password", "Password", focused("password")))
                    (password_field(form, "password-confirmation", "passwordConfirmation", "Confirm password", focused("passwordConfirmation")))
                }
                button type="submit" class="button button--primary button--block" { "Create account" }
            }
        }
    };
    auth_page(context, "Join MyMCPs", content)
}

/// `GET /invite/{token}` for someone signed in with the invited address:
/// one button joins the team.
pub fn join_page(context: &PageContext, invite: &Invite, team: &Team) -> Markup {
    let content = html! {
        section class="auth-card control-lg" aria-labelledby="invite-title" {
            (logo())
            div class="auth-card__heading" {
                h1 class="auth-card__title" id="invite-title" { "Join " (team.name) }
                p class="auth-card__subtitle" {
                    "You were invited as " (invite.email) ", to join this team as " (invite.role.as_str()) "."
                }
            }
            form class="form" method="post" action=(format!("/invite/{}", invite.token)) {
                (context.csrf_field())
                div class="stack gap-300" {
                    button type="submit" class="button button--primary button--block" { "Join team" }
                    a class="button button--block" href="/" { "Not now" }
                }
            }
        }
    };
    auth_page(context, "Join a team", content)
}
