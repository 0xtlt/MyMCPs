//! The Team page (the team someone works in: its name, its members, its
//! invites), the dialog that creates a team, and the accounts of the
//! instance for those who run it.

use maud::{Markup, html};
use mymcps_core::models::{Invite, Membership, TeamRole, User};

use crate::forms::FormState;
use crate::routes::teams::MemberRow;
use crate::views::icon::icon;
use crate::views::invites::{create_invite_dialog, invite_created_dialog, invite_url, invites};
use crate::views::settings::password_field_with_help;
use crate::views::shell::{PageContext, app_page};
use crate::views::{grouped, time_minute};

/// What the Team page shows.
pub struct TeamPage<'a> {
    /// The team, and what the signed-in person is in it.
    pub team: &'a Membership,
    /// In the order they joined.
    pub members: &'a [MemberRow],
    /// Newest first. Empty for a member, who does not invite.
    pub invites: &'a [Invite],
    pub current_user_id: i64,
    /// How many MCPs the team has: it is not deleted while it has some.
    pub mcps: i64,
    /// The form that renames the team, after it was refused.
    pub team_form: &'a FormState,
    /// The "Create invite" form: what was refused when it was posted
    /// without the page's script.
    pub invite_form: &'a FormState,
    /// The "Create team" form, and whether the page is the one that shows
    /// its dialog open.
    pub create_form: &'a FormState,
    pub open_create: bool,
    /// The invite the previous request created: its link is shown once.
    pub created: Option<&'a Invite>,
}

pub(crate) fn has_name(user: &User) -> bool {
    user.full_name
        .as_deref()
        .is_some_and(|name| !name.is_empty())
}

pub(crate) fn role_badge(role: TeamRole) -> Markup {
    let class = match role {
        TeamRole::Owner => "badge badge--brand",
        TeamRole::Admin => "badge badge--info",
        TeamRole::Member => "badge",
    };
    html! { span class=(class) { (role.as_str()) } }
}

pub(crate) fn card_header(id: &str, title: &str, subtitle: &str) -> Markup {
    html! {
        header class="card__header" {
            div class="card__heading" {
                h2 class="card__title" id=(id) { (title) }
                p class="card__subtitle" { (subtitle) }
            }
        }
    }
}

/// What the person may do to a member: nothing to themselves, nor to
/// someone ranked above them.
fn manages(team: &TeamPage, member: &MemberRow) -> bool {
    member.user.id != team.current_user_id && team.team.role.may_manage(member.role)
}

/// The menu of a member: the roles the person may give them, and the way
/// out of the team. Each entry is a form of its own.
fn member_menu(context: &PageContext, team: &TeamPage, member: &MemberRow, id: &str) -> Markup {
    let name = member.user.display_name();
    let action = format!("/members/{}", member.user.id);
    html! {
        button type="button" class="icon-button" popovertarget=(id)
            aria-label=(format!("Actions for {name}")) { (icon("ellipsis")) }
        div class="menu" id=(id) popover role="menu" {
            p class="menu__label" { "Role" }
            @for role in TeamRole::ALL.iter().rev().filter(|role| team.team.role.may_grant(**role)) {
                form method="post" action=(format!("{action}?_method=PATCH")) {
                    (context.csrf_field())
                    input type="hidden" name="role" value=(role.as_str());
                    button type="submit" class="menu__item" role="menuitemradio"
                        aria-checked=(if *role == member.role { "true" } else { "false" }) {
                        (match role {
                            TeamRole::Owner => "Owner",
                            TeamRole::Admin => "Admin",
                            TeamRole::Member => "Member",
                        })
                    }
                }
            }
            div class="menu__separator" {}
            form method="post" action=(format!("{action}?_method=DELETE"))
                data-confirm=(format!("{name} loses access to {}. The access tokens they made in this team are revoked. Their account and their other teams are not affected.", team.team.team.name))
                data-confirm-title=(format!("Remove {name}?")) data-confirm-label="Remove" data-confirm-tone="critical" {
                (context.csrf_field())
                button type="submit" class="menu__item menu__item--critical" role="menuitem" { (icon("trash")) "Remove from team" }
            }
        }
    }
}

/// Members: a table on desktop, a list under 768px.
fn members(context: &PageContext, team: &TeamPage) -> Markup {
    let subtitle = match team.members.len() {
        1 => "1 person works in this team".to_string(),
        count => format!("{} people work in this team", grouped(count)),
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
                                        span class="avatar avatar--md" { (member.user.initials()) }
                                        div class="cell-stack" {
                                            span class="cell-title" { (member.user.display_name()) }
                                            @if has_name(&member.user) { span class="cell-sub cell-sub--secondary" { (member.user.email) } }
                                        }
                                    }
                                }
                                td { (role_badge(member.role)) }
                                td class="cell-secondary" { (time_minute(member.joined_at)) }
                                @if member.user.id == team.current_user_id {
                                    td class="cell-end cell-tertiary" { "You" }
                                } @else if manages(team, member) {
                                    td class="cell-end" {
                                        span class="row-actions" {
                                            (member_menu(context, team, member, &format!("member-{}-menu", member.user.id)))
                                        }
                                    }
                                } @else {
                                    td class="cell-end" {}
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
                        span class="avatar avatar--md" { (member.user.initials()) }
                        span class="list-row__text" {
                            span class="list-row__title" { (member.user.display_name()) }
                            @if has_name(&member.user) { span class="list-row__subtitle" { (member.user.email) } }
                        }
                        (role_badge(member.role))
                        @if member.user.id == team.current_user_id {
                            span class="list-row__meta list-row__meta--slot" { "You" }
                        } @else if manages(team, member) {
                            (member_menu(context, team, member, &format!("member-{}-list-menu", member.user.id)))
                        }
                    }
                }
            }
        }
    }
}

/// A text field of a card form, with its error under it.
fn text_field(
    form: &FormState,
    id: &str,
    name: &str,
    label: &str,
    value: &str,
    optional: bool,
) -> Markup {
    let error = form.error(name);
    let error_id = format!("{id}-error");
    html! {
        div class="field" {
            label class="field__label" for=(id) {
                (label) @if optional { " " span class="field__optional" { "Optional" } }
            }
            input class="input" id=(id) name=(name) type="text" autocomplete="off" value=(value)
                aria-invalid=[error.map(|_| "true")] aria-describedby=[error.map(|_| error_id.as_str())];
            @if let Some(message) = error { p class="field__error" id=(error_id) { (message) } }
        }
    }
}

/// What the form shows in a field: what was refused, or what is stored.
fn shown(form: &FormState, name: &str, stored: &str) -> String {
    if form.has_errors() {
        form.old(name)
    } else {
        stored.to_string()
    }
}

/// The name of the team and what it is for: a form for those who may
/// change them, plain rows for a member.
fn general(context: &PageContext, team: &TeamPage) -> Markup {
    let current = &team.team.team;
    let description = current.description.as_deref().unwrap_or("");
    html! {
        section class="section" aria-labelledby="team-general-title" {
            div class="section__intro" {
                h2 class="section__title" id="team-general-title" { "General" }
                p class="section__description" {
                    "MCPs, access tokens, approvals and call logs belong to a team. Only its members see them."
                }
            }
            @if team.team.role.is_admin() {
                form class="card" method="post" action="/team?_method=PATCH" {
                    (context.csrf_field())
                    div class="card__body" {
                        div class="stack" {
                            (text_field(team.team_form, "team-name", "name", "Name", &shown(team.team_form, "name", &current.name), false))
                            (text_field(team.team_form, "team-description", "description", "Description", &shown(team.team_form, "description", description), true))
                        }
                    }
                    footer class="card__footer" {
                        button type="submit" class="button button--secondary" { "Save team" }
                    }
                }
            } @else {
                div class="card" {
                    div class="detail-row" {
                        div class="detail-row__text" {
                            span class="detail-row__label" { "Name" }
                            span class="detail-row__value" { (current.name) }
                        }
                    }
                    @if !description.is_empty() {
                        div class="detail-row" {
                            div class="detail-row__text" {
                                span class="detail-row__label" { "Description" }
                                span class="detail-row__value" { (description) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Deleting the team, for an owner. The Root Team and a personal team stay.
fn danger_zone(context: &PageContext, team: &TeamPage) -> Markup {
    let current = &team.team.team;
    let reason = if current.is_root() {
        Some("The Root Team is the first team of this instance and cannot be deleted.")
    } else if current.personal_team {
        Some("This is your personal team. It stays as long as your account does.")
    } else if team.mcps > 0 {
        Some("Delete the MCPs of this team before you delete it.")
    } else {
        None
    };
    html! {
        section class="section" aria-labelledby="team-danger-title" {
            div class="section__intro" {
                h2 class="section__title" id="team-danger-title" { "Danger zone" }
                p class="section__description" { "Deleting a team removes its access tokens, call logs and invites for good." }
            }
            div class="card" {
                div class="detail-row" {
                    div class="detail-row__text" {
                        span class="detail-row__label" { "Delete this team" }
                        span class="detail-row__value" {
                            @match reason {
                                Some(reason) => { (reason) }
                                None => { "Its members keep their accounts and their other teams." }
                            }
                        }
                    }
                    @if reason.is_some() {
                        button type="button" class="button button--secondary" disabled { "Delete team" }
                    } @else {
                        form method="post" action="/team?_method=DELETE"
                            data-confirm=(format!("{} is deleted with its access tokens, call logs and invites. This cannot be undone.", current.name))
                            data-confirm-title=(format!("Delete {}?", current.name)) data-confirm-label="Delete team" data-confirm-tone="critical" {
                            (context.csrf_field())
                            button type="submit" class="button button--secondary" { "Delete team" }
                        }
                    }
                }
            }
        }
    }
}

/// The form of the "Create team" dialog: what its `[data-fragment]` holds,
/// and what a refused submission of the page's script gets back.
pub fn create_team_form(context: &PageContext, form: &FormState) -> Markup {
    let name_error = form.error("name");
    let description_error = form.error("description");
    html! {
        form method="post" action="/teams" data-async {
            (context.csrf_field())
            div class="dialog__header" {
                div class="dialog__heading" {
                    h2 class="dialog__title" id="create-team-title" { "Create team" }
                    p class="dialog__subtitle" { "A team has its own MCPs, access tokens and members" }
                }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="dialog__body" {
                div class="stack" {
                    div class="field" {
                        label class="field__label" for="create-team-name" { "Name" }
                        // An invalid field takes the focus by itself.
                        input class="input" id="create-team-name" name="name" type="text" autocomplete="off"
                            value=(form.old("name")) autofocus[!form.has_errors()]
                            aria-invalid=[name_error.map(|_| "true")] aria-describedby=[name_error.map(|_| "create-team-name-error")];
                        @if let Some(message) = name_error { p class="field__error" id="create-team-name-error" { (message) } }
                    }
                    div class="field" {
                        label class="field__label" for="create-team-description" {
                            "Description " span class="field__optional" { "Optional" }
                        }
                        input class="input" id="create-team-description" name="description" type="text" autocomplete="off"
                            value=(form.old("description"))
                            aria-invalid=[description_error.map(|_| "true")] aria-describedby=[description_error.map(|_| "create-team-description-error")];
                        @if let Some(message) = description_error { p class="field__error" id="create-team-description-error" { (message) } }
                    }
                }
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                button type="submit" class="button button--primary" { "Create team" }
            }
        }
    }
}

/// The "Create team" dialog, which the team switcher of every page opens.
/// `open` on the page behind `/teams/new`, where it also returns to `/team`.
pub fn create_team_dialog(context: &PageContext, form: &FormState, open: bool) -> Markup {
    html! {
        dialog class="dialog dialog--sm" id="create-team" aria-labelledby="create-team-title" data-dialog-reset
            data-open[open] data-dialog-return=[open.then_some("/team")] {
            div data-fragment { (create_team_form(context, form)) }
        }
    }
}

pub fn team_page(context: &PageContext, team: &TeamPage) -> Markup {
    let current = &team.team.team;
    let admin = team.team.role.is_admin();
    // Without APP_URL there is no link to show: the toast says the invite exists.
    let created = team
        .created
        .filter(|invite| invite.is_usable())
        .and_then(|invite| Some((invite, invite_url(context, invite)?)));
    let invite_refused = team.invite_form.has_errors();

    // What a dialog of the page already says is not said again in a toast.
    let mut context = context.clone();
    if created.is_some() {
        context.flash_success = None;
    }
    for form in [team.invite_form, team.create_form, team.team_form] {
        if form.has_errors() && context.flash_error.as_deref() == form.first_error() {
            context.flash_error = None;
        }
    }
    let context = &context;

    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { (current.name) }
                p class="page-header__subtitle" {
                    @match team.team.role {
                        TeamRole::Owner => "You own this team: you manage its MCPs, its members and the team itself.",
                        TeamRole::Admin => "You are an admin of this team: you manage its MCPs and its members.",
                        TeamRole::Member => "You are a member of this team: you use its MCPs with access tokens of your own.",
                    }
                }
            }
            @if admin {
                div class="page-header__actions" {
                    button type="button" class="button button--primary" data-dialog-open="#create-invite" { (icon("plus")) "Create invite" }
                }
            }
        }
        (general(context, team))
        (members(context, team))
        @if admin { (invites(context, team.invites)) }
        @if team.team.role.is_owner() { (danger_zone(context, team)) }
    };
    let overlays = html! {
        @if admin { (create_invite_dialog(context, team.invite_form, team.team.role, invite_refused)) }
        @if let Some((invite, url)) = &created { (invite_created_dialog(invite, url)) }
        @if team.open_create { (create_team_dialog(context, team.create_form, true)) }
    };
    app_page(context, "Team", content, overlays)
}

/// An account of the instance, with the teams it is in.
pub struct UserRow {
    pub user: User,
    /// The name of each team, and what the account is in it.
    pub teams: Vec<(String, TeamRole)>,
}

/// A dialog of the Users page. It acts on one account, and asks the person
/// for their own password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserDialog {
    /// Make the account a super admin, or an account like any other.
    SuperAdmin,
    Delete,
}

impl UserDialog {
    pub const ALL: [Self; 2] = [Self::SuperAdmin, Self::Delete];

    /// The id of the `<dialog>`, which is also the fragment the page script
    /// asks for.
    pub const fn id(self) -> &'static str {
        match self {
            Self::SuperAdmin => "super-admin",
            Self::Delete => "delete-user",
        }
    }

    /// Where the dialog of an account is drawn open: the address a row
    /// links to.
    pub fn href(self, account: &User) -> String {
        match self {
            Self::SuperAdmin => format!("/users/{}/super-admin", account.id),
            Self::Delete => format!("/users/{}/delete", account.id),
        }
    }

    /// The form of the dialog: what its `[data-fragment]` holds, and what a
    /// refused submission of the page's script gets back.
    pub fn form(self, context: &PageContext, account: &User, form: &FormState) -> Markup {
        match self {
            Self::SuperAdmin => super_admin_form(context, account, form),
            Self::Delete => delete_user_form(context, account, form),
        }
    }
}

/// The dialog the Users page shows open, on which account, with what was
/// refused when its form was posted without the page's script.
#[derive(Clone, Copy)]
pub struct OpenUserDialog<'a> {
    pub dialog: UserDialog,
    pub account: &'a User,
    pub form: &'a FormState,
}

/// What the Users page shows.
pub struct UsersPage<'a> {
    /// Oldest first.
    pub users: &'a [UserRow],
    /// The signed-in person, who changes nothing of their own account here.
    pub current_user_id: i64,
    pub open: Option<OpenUserDialog<'a>>,
}

fn team_names(row: &UserRow) -> String {
    row.teams
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the account is for the instance: one of those who run it, or not.
fn instance_badge(row: &UserRow) -> Markup {
    html! {
        @if row.user.is_super_admin {
            span class="badge badge--info" { "Super admin" }
        } @else {
            span class="badge" { "User" }
        }
    }
}

/// The menu of an account: whether it runs the instance, and the way out of
/// it. Each entry opens a dialog, which the page script loads for the row
/// that was chosen.
fn user_menu(row: &UserRow, id: &str) -> Markup {
    let account = &row.user;
    html! {
        button type="button" class="icon-button" popovertarget=(id)
            aria-label=(format!("Actions for {}", account.display_name())) { (icon("ellipsis")) }
        div class="menu" id=(id) popover role="menu" {
            a class="menu__item" role="menuitem" href=(UserDialog::SuperAdmin.href(account))
                data-dialog-open="#super-admin" data-dialog-fetch {
                @if account.is_super_admin {
                    (icon("user")) "Remove super admin"
                } @else {
                    (icon("shield-check")) "Make super admin"
                }
            }
            div class="menu__separator" {}
            a class="menu__item menu__item--critical" role="menuitem" href=(UserDialog::Delete.href(account))
                data-dialog-open="#delete-user" data-dialog-fetch { (icon("trash")) "Delete user" }
        }
    }
}

/// The form of the "Super admin" dialog. It says what the account becomes,
/// so that it does the same whatever the account has become meanwhile. It
/// hands over the instance, so the person types their own password.
pub fn super_admin_form(context: &PageContext, account: &User, form: &FormState) -> Markup {
    let name = account.display_name();
    let grant = !account.is_super_admin;
    html! {
        form method="post" action=(format!("/users/{}/super-admin?_method=PATCH", account.id)) data-async {
            (context.csrf_field())
            input type="hidden" name="superAdmin" value=(if grant { "true" } else { "false" });
            div class="dialog__header" {
                div class="dialog__heading" {
                    h2 class="dialog__title" id="super-admin-title" {
                        @if grant { "Make " (name) " a super admin?" } @else { "Remove " (name) " from the super admins?" }
                    }
                    p class="dialog__subtitle" { (account.email) }
                }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="dialog__body" {
                div class="stack" {
                    p class="text-secondary" {
                        @if grant {
                            (name) " will run this instance as you do: change its settings, export backups of what every team stores, delete accounts, and decide who else is a super admin. Their teams stay as they are."
                        } @else {
                            (name) " keeps their account and their teams, and no longer changes the instance settings, exports backups, or manages accounts."
                        }
                    }
                    (password_field_with_help(
                        form,
                        "super-admin-password",
                        "currentPassword",
                        "Your password",
                        "current-password",
                        !form.has_errors(),
                        Some("Your account password, to confirm it is you."),
                    ))
                }
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                @if grant {
                    button type="submit" class="button button--primary" { "Make super admin" }
                } @else {
                    button type="submit" class="button button--critical" { "Remove super admin" }
                }
            }
        }
    }
}

/// The form of the "Delete user" dialog. Deleting an account cannot be
/// undone, so the person types their own password.
pub fn delete_user_form(context: &PageContext, account: &User, form: &FormState) -> Markup {
    let name = account.display_name();
    html! {
        form method="post" action=(format!("/users/{}?_method=DELETE", account.id)) data-async {
            (context.csrf_field())
            div class="dialog__header" {
                div class="dialog__heading" {
                    h2 class="dialog__title" id="delete-user-title" { "Delete " (name) "?" }
                    p class="dialog__subtitle" { (account.email) }
                }
                button type="button" class="icon-button" data-dialog-close aria-label="Close" { (icon("x")) }
            }
            div class="dialog__body" {
                div class="stack" {
                    p class="text-secondary" {
                        (name) " can no longer sign in. Their access tokens are revoked, and the teams they were alone in are deleted with their MCPs. This cannot be undone."
                    }
                    (password_field_with_help(
                        form,
                        "delete-user-password",
                        "currentPassword",
                        "Your password",
                        "current-password",
                        !form.has_errors(),
                        Some("Your account password, to confirm it is you."),
                    ))
                }
            }
            div class="dialog__footer" {
                button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                button type="submit" class="button button--critical" { "Delete user" }
            }
        }
    }
}

/// `GET /users`: the accounts of the instance, for those who run it.
pub fn users_page(context: &PageContext, page: &UsersPage) -> Markup {
    let subtitle = match page.users.len() {
        1 => "1 account can sign in to this instance".to_string(),
        count => format!("{} accounts can sign in to this instance", grouped(count)),
    };
    let content = html! {
        header class="page-header" {
            div class="page-header__text" {
                h1 class="page-header__title" { "Users" }
                p class="page-header__subtitle" {
                    "Every account of this instance, whichever team it works in. Super admins run the instance: its settings, its backups, and its accounts."
                }
            }
            div class="page-header__actions" {
                a class="button button--secondary" href="/settings" { "Registration settings" }
            }
        }

        section class="card hide-mobile" aria-labelledby="users-title" {
            (card_header("users-title", "Accounts", &subtitle))
            div class="table-scroll" {
                table class="table" {
                    thead {
                        tr { th { "Name" } th { "Teams" } th class="col-160" { "Instance" } th class="col-180" { "Joined" } th class="col-120 cell-end" { "Actions" } }
                    }
                    tbody {
                        @for row in page.users {
                            tr {
                                td {
                                    div class="cell-media" {
                                        span class="avatar avatar--md" { (row.user.initials()) }
                                        div class="cell-stack" {
                                            span class="cell-title" { (row.user.display_name()) }
                                            @if has_name(&row.user) { span class="cell-sub cell-sub--secondary" { (row.user.email) } }
                                        }
                                    }
                                }
                                td class="cell-secondary" { (team_names(row)) }
                                td { (instance_badge(row)) }
                                td class="cell-secondary" { (time_minute(row.user.created_at)) }
                                @if row.user.id == page.current_user_id {
                                    td class="cell-end cell-tertiary" { "You" }
                                } @else {
                                    td class="cell-end" {
                                        span class="row-actions" {
                                            (user_menu(row, &format!("user-{}-menu", row.user.id)))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        section class="card hide-desktop" aria-labelledby="users-list-title" {
            (card_header("users-list-title", "Accounts", &subtitle))
            ul {
                @for row in page.users {
                    li class="list-row" {
                        span class="avatar avatar--md" { (row.user.initials()) }
                        span class="list-row__text" {
                            span class="list-row__title" { (row.user.display_name()) }
                            span class="list-row__subtitle" { (team_names(row)) }
                        }
                        (instance_badge(row))
                        @if row.user.id == page.current_user_id {
                            span class="list-row__meta list-row__meta--slot" { "You" }
                        } @else {
                            (user_menu(row, &format!("user-{}-list-menu", row.user.id)))
                        }
                    }
                }
            }
        }
    };
    // What the open dialog already says under its field is not said again in a toast.
    let mut context = context.clone();
    if let Some(open) = page.open
        && open.form.has_errors()
        && context.flash_error.as_deref() == open.form.first_error()
    {
        context.flash_error = None;
    }
    let context = &context;
    let overlays = html! {
        @for dialog in UserDialog::ALL {
            @let open = page.open.filter(|open| open.dialog == dialog);
            dialog class="dialog dialog--sm" id=(dialog.id()) aria-labelledby=(format!("{}-title", dialog.id())) data-dialog-reset
                data-dialog-return="/users" data-open[open.is_some()] {
                div data-fragment {
                    @if let Some(open) = open { (dialog.form(context, open.account, open.form)) }
                }
            }
        }
    };
    app_page(context, "Users", content, overlays)
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
