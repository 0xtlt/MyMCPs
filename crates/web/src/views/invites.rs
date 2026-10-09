//! The Team page (members and invites) and the page behind an invite link.

use maud::{Markup, html};
use mymcps_core::models::{Invite, User};

use crate::forms::FormState;
use crate::views::auth::form_banner;
use crate::views::icon::{icon, logo};
use crate::views::shell::{PageContext, app_page, auth_page};
use crate::views::{date, grouped, time_minute};

/// What a control that needs the public address says while there is none.
const NO_APP_URL: &str = "Set APP_URL to enable public links";

/// What the Team page shows.
pub struct TeamPage<'a> {
    /// Oldest first.
    pub members: &'a [User],
    /// Newest first.
    pub invites: &'a [Invite],
    /// The signed-in administrator, who cannot remove their own account.
    pub current_user_id: i64,
    /// The "Create invite" form: what was refused when it was posted
    /// without the page's script.
    pub form: &'a FormState,
    /// The invite the previous request created: its link is shown once.
    pub created: Option<&'a Invite>,
}

/// The address of an invite link, when `APP_URL` is set.
fn invite_url(context: &PageContext, invite: &Invite) -> Option<String> {
    context
        .app_url
        .as_ref()
        .map(|app_url| format!("{app_url}/invite/{}", invite.token))
}

/// The name a member goes by: their name, or their email when they have none.
fn display_name(member: &User) -> &str {
    member
        .full_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&member.email)
}

fn has_name(member: &User) -> bool {
    member
        .full_name
        .as_deref()
        .is_some_and(|name| !name.is_empty())
}

fn role_badge(member: &User) -> Markup {
    html! {
        span class=(if member.is_admin() { "badge badge--info" } else { "badge" }) { (member.role.as_str()) }
    }
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

/// The form that removes a member, around the button that sends it.
fn remove_member_form(context: &PageContext, member: &User, button: Markup) -> Markup {
    let name = display_name(member);
    html! {
        form method="post" action=(format!("/members/{}?_method=DELETE", member.id))
            data-confirm=(format!("{name} will no longer be able to sign in. Their access tokens are revoked and their MCPs are transferred to you."))
            data-confirm-title=(format!("Remove {name}?")) data-confirm-label="Remove" data-confirm-tone="critical" {
            (context.csrf_field())
            (button)
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

fn card_header(id: &str, title: &str, subtitle: &str) -> Markup {
    html! {
        header class="card__header" {
            div class="card__heading" {
                h2 class="card__title" id=(id) { (title) }
                p class="card__subtitle" { (subtitle) }
            }
        }
    }
}

/// Members: a table on desktop, a list under 768px.
fn members(context: &PageContext, team: &TeamPage) -> Markup {
    let subtitle = match team.members.len() {
        1 => "1 person can sign in to this instance".to_string(),
        count => format!("{} people can sign in to this instance", grouped(count)),
    };
    html! {
        section class="card hide-mobile" aria-labelledby="members-title" {
            (card_header("members-title", "Members", &subtitle))
            div class="table-scroll" {
                table class="table" {
                    thead {
                        tr { th { "Name" } th class="col-120" { "Role" } th class="col-180" { "Joined" } th class="col-120 cell-end" { "Actions" } }
                    }
                    tbody {
                        @for member in team.members {
                            tr {
                                td {
                                    div class="cell-media" {
                                        span class="avatar avatar--md" { (member.initials()) }
                                        div class="cell-stack" {
                                            span class="cell-title" { (display_name(member)) }
                                            @if has_name(member) { span class="cell-sub cell-sub--secondary" { (member.email) } }
                                        }
                                    }
                                }
                                td { (role_badge(member)) }
                                td class="cell-secondary" { (time_minute(member.created_at)) }
                                // The signed-in person cannot remove their own account.
                                @if member.id == team.current_user_id {
                                    td class="cell-end cell-tertiary" { "You" }
                                } @else {
                                    td class="cell-end" { span class="row-actions" { (remove_member_form(context, member, row_button())) } }
                                }
                            }
                        }
                    }
                }
            }
        }

        section class="card hide-desktop" aria-labelledby="members-list-title" {
            (card_header("members-list-title", "Members", &subtitle))
            ul {
                @for member in team.members {
                    li class="list-row" {
                        span class="avatar avatar--md" { (member.initials()) }
                        span class="list-row__text" {
                            span class="list-row__title" { (display_name(member)) }
                            @if has_name(member) { span class="list-row__subtitle" { (member.email) } }
                        }
                        (role_badge(member))
                        @if member.id == team.current_user_id {
                            span class="list-row__meta list-row__meta--slot" { "You" }
                        } @else {
                            @let menu = format!("member-{}-menu", member.id);
                            button type="button" class="icon-button" popovertarget=(menu)
                                aria-label=(format!("Actions for {}", display_name(member))) { (icon("ellipsis")) }
                            div class="menu" id=(menu) popover role="menu" {
                                (remove_member_form(context, member, menu_button("Remove member")))
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Invites: newest first. Only a pending invite has a link to copy.
fn invites(context: &PageContext, team: &TeamPage) -> Markup {
    const SUBTITLE: &str = "One-time join links, valid for 7 days";
    if team.invites.is_empty() {
        return html! {
            section class="card" aria-labelledby="invites-title" {
                (card_header("invites-title", "Invites", SUBTITLE))
                div class="empty-state" {
                    span class="empty-state__icon" { (icon("user-plus")) }
                    div class="empty-state__text" {
                        p class="empty-state__title" { "No invites yet" }
                        p class="empty-state__description" { "Create an invite link to add a teammate." }
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
                        tr { th { "Email" } th class="col-180" { "Expires" } th class="col-120" { "Status" } th class="col-180 cell-end" { "Actions" } }
                    }
                    tbody {
                        @for invite in team.invites {
                            @let status = InviteStatus::of(invite);
                            tr {
                                td class="cell-strong" { (invite.email) }
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
                @for invite in team.invites {
                    @let status = InviteStatus::of(invite);
                    @let menu = format!("invite-{}-menu", invite.id);
                    li class="list-row" {
                        span class="list-row__text" {
                            span class="list-row__title" { (invite.email) }
                            span class="list-row__subtitle" { "Expires " (date(invite.expires_at)) }
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
/// and what a refused submission of the page's script gets back.
pub fn create_invite_form(context: &PageContext, form: &FormState) -> Markup {
    let error = form.error("email");
    html! {
        form method="post" action="/invites" data-async {
            (context.csrf_field())
            div class="dialog__header" {
                div class="dialog__heading" {
                    h2 class="dialog__title" id="create-invite-title" { "Create invite" }
                    p class="dialog__subtitle" { "Send a one-time join link to a teammate" }
                }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="dialog__body" {
                div class="field" {
                    label class="field__label" for="invite-email" { "Email" }
                    // An invalid field takes the focus by itself.
                    input class="input" id="invite-email" name="email" type="email" placeholder="teammate@example.com"
                        value=(form.old("email")) autofocus[error.is_none()]
                        aria-invalid=[error.map(|_| "true")] aria-describedby=[error.map(|_| "invite-email-error")];
                    @if let Some(message) = error { p class="field__error" id="invite-email-error" { (message) } }
                }
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                button type="submit" class="button button--primary" { "Create invite" }
            }
        }
    }
}

/// The link of the invite that was just created, shown once.
fn invite_created_dialog(invite: &Invite, url: &str) -> Markup {
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

pub fn team_page(context: &PageContext, team: &TeamPage) -> Markup {
    // Without APP_URL there is no link to show: the toast says the invite exists.
    let created = team
        .created
        .filter(|invite| invite.is_usable())
        .and_then(|invite| Some((invite, invite_url(context, invite)?)));
    let refused = team.form.has_errors();

    // What a dialog of the page already says is not said again in a toast.
    let mut context = context.clone();
    if created.is_some() {
        context.flash_success = None;
    }
    if refused && context.flash_error.as_deref() == team.form.first_error() {
        context.flash_error = None;
    }
    let context = &context;

    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "Team" }
                p class="page-header__subtitle" {
                    "Invite teammates with a link. There is no public registration—only people you invite can join this instance."
                }
            }
            div class="page-header__actions" {
                button type="button" class="button button--primary" data-dialog-open="#create-invite" { (icon("plus")) "Create invite" }
            }
        }
        (members(context, team))
        (invites(context, team))
    };
    let overlays = html! {
        dialog class="dialog dialog--sm" id="create-invite" aria-labelledby="create-invite-title" data-dialog-reset data-open[refused] {
            div data-fragment { (create_invite_form(context, team.form)) }
        }
        @if let Some((invite, url)) = &created { (invite_created_dialog(invite, url)) }
    };
    app_page(context, "Team", content, overlays)
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

/// `GET /invite/{token}`: the invited person chooses a name and a password.
/// The email comes from the invite, there is no email field.
pub fn accept_page(context: &PageContext, invite: &Invite, form: &FormState) -> Markup {
    // After a refusal, the first field to correct takes the focus.
    let first_invalid = ["fullName", "password", "passwordConfirmation"]
        .into_iter()
        .find(|name| form.error(name).is_some());
    let focused = |name: &str| first_invalid == Some(name);
    let content = html! {
        section class="auth-card control-lg" aria-labelledby="invite-title" {
            (logo())
            div class="auth-card__heading" {
                h1 class="auth-card__title" id="invite-title" { "Join MyMCPs" }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        for (count, text) in [
            (0, "0"),
            (7, "7"),
            (999, "999"),
            (1_000, "1,000"),
            (12_345, "12,345"),
            (1_234_567, "1,234,567"),
        ] {
            assert_eq!(grouped(count), text);
        }
    }
}
