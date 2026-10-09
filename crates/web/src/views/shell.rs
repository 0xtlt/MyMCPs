//! The frame every page is drawn in: the app shell with its navigation for
//! signed-in people, and the centred card of the screens before sign-in.
//! Markup and class names follow `docs/design-system.md`.

use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts};
use http::request::Parts;
use maud::{DOCTYPE, Markup, html};
use mymcps_core::Core;
use mymcps_core::models::User;

use crate::assets::asset_url;
use crate::auth::Auth;
use crate::cookies::Cookies;
use crate::csrf::{CSRF_FIELD, csrf_token};
use crate::error::AppError;
use crate::error::is_fetch;
use crate::security::CspNonce;
use crate::session::Session;
use crate::state::AppState;
use crate::views::icon::{icon, logo, logo_on_frame};

/// Cookie the page script writes when the sidebar is collapsed, so the next
/// page is drawn collapsed from the start.
const SIDEBAR_COOKIE: &str = "mm_sidebar";

/// What every page needs to draw itself.
#[derive(Debug, Clone)]
pub struct PageContext {
    pub user: Option<User>,
    /// The path of the request, to mark the current navigation entry.
    pub path: String,
    /// The token of this response's forms.
    pub csrf: String,
    pub nonce: String,
    /// Flashed by the previous request.
    pub flash_success: Option<String>,
    pub flash_error: Option<String>,
    /// Shown beside Approvals in the navigation, on every page.
    pub pending_approvals: i64,
    /// `APP_URL` without its trailing slash, for the links shown to people.
    pub app_url: Option<String>,
    /// Whether `APP_URL` can be used for OAuth endpoints and signed links.
    pub app_url_configured: bool,
    pub sidebar_collapsed: bool,
    /// The page's script sent the request and expects a fragment back.
    pub is_fetch: bool,
}

impl PageContext {
    /// The hidden field every state-changing form carries.
    pub fn csrf_field(&self) -> Markup {
        html! { input type="hidden" name=(CSRF_FIELD) value=(self.csrf); }
    }

    /// The address MCP clients connect to, when `APP_URL` is set.
    pub fn gateway_url(&self) -> Option<String> {
        self.app_url
            .as_ref()
            .map(|app_url| format!("{app_url}/mcp"))
    }
}

impl<S> FromRequestParts<S> for PageContext
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    /// Counting the approvals that wait reads the database.
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let core: &Arc<Core> = &state.core;
        let session = parts
            .extensions
            .get::<Session>()
            .cloned()
            .unwrap_or_default();
        let user = parts
            .extensions
            .get::<Auth>()
            .and_then(|auth| auth.user.clone());
        let cookies = parts
            .extensions
            .get::<Cookies>()
            .cloned()
            .unwrap_or_default();
        let pending_approvals = match &user {
            Some(user) => state.pending_approvals(user).await?,
            None => 0,
        };

        Ok(Self {
            user,
            path: parts.uri.path().to_string(),
            csrf: csrf_token(&session),
            nonce: parts
                .extensions
                .get::<CspNonce>()
                .map(|nonce| nonce.0.clone())
                .unwrap_or_default(),
            flash_success: session.flashed_text("success"),
            flash_error: session.flashed_text("error"),
            pending_approvals,
            app_url: core.config.public_app_url(),
            app_url_configured: core.config.public_oauth_app_url().is_some(),
            sidebar_collapsed: cookies.get(SIDEBAR_COOKIE).as_deref() == Some("collapsed"),
            is_fetch: is_fetch(&parts.headers),
        })
    }
}

fn head(context: &PageContext, title: &str) -> Markup {
    html! {
        head {
            meta charset="utf-8";
            meta name="viewport" content="width=device-width, initial-scale=1";
            // The instance is private: nothing of it belongs in a search engine.
            meta name="robots" content="noindex, nofollow, noarchive, nosnippet";
            meta name="googlebot" content="noindex, nofollow, noarchive, nosnippet";
            title { (title) " · MyMCPs" }
            meta name="csrf-token" content=(context.csrf);
            link rel="icon" href=(asset_url("brand/favicon.svg")) type="image/svg+xml";
            link rel="alternate icon" href="/favicon.png" type="image/png";
            link rel="apple-touch-icon" href=(asset_url("brand/apple-touch-icon.png"));
            link rel="stylesheet" href=(asset_url("css/app.css"));
            script type="module" src=(asset_url("js/app.js")) nonce=(context.nonce) {}
        }
    }
}

/// A message of the toast region: a confirmation, or an error that stays.
pub(crate) fn toast(message: &str, error: bool) -> Markup {
    html! {
        @if error {
            div class="toast toast--error" data-toast role="alert" {
                (icon("circle-x")) p class="toast__message" { (message) }
                button type="button" class="toast__dismiss" data-toast-dismiss aria-label="Dismiss" { (icon("x")) }
            }
        } @else {
            div class="toast" data-toast role="status" {
                (icon("circle-check")) p class="toast__message" { (message) }
                button type="button" class="toast__dismiss" data-toast-dismiss aria-label="Dismiss" { (icon("x")) }
            }
        }
    }
}

/// The flash toasts of this page, and the templates the page script clones
/// for the toasts and the confirm prompt it shows itself.
fn overlays(context: &PageContext) -> Markup {
    html! {
        div class="toast-region" data-toasts {
            @if let Some(message) = &context.flash_error { (toast(message, true)) }
            @if let Some(message) = &context.flash_success { (toast(message, false)) }
        }
        template id="toast-template" { (toast("", false)) }
        template id="toast-error-template" { (toast("", true)) }
        template id="confirm-template" {
            dialog class="dialog dialog--sm" role="alertdialog" {
                div class="dialog__header" {
                    div class="dialog__heading" { h2 class="dialog__title" data-confirm-title-slot { "Are you sure?" } }
                }
                div class="dialog__body" { p class="text-secondary" data-confirm-message-slot {} }
                div class="dialog__footer" {
                    button type="button" class="button button--secondary" data-dialog-close { "Cancel" }
                    button type="button" class="button button--primary" data-confirm-accept { "Confirm" }
                }
            }
        }
    }
}

fn nav_item(context: &PageContext, href: &str, icon_name: &str, label: &str) -> Markup {
    let current = if href == "/" {
        context.path == "/"
    } else {
        context.path.starts_with(href)
    };
    html! {
        a class="nav-item" href=(href) aria-current=[current.then_some("page")] {
            (icon(icon_name)) span class="nav-item__label" { (label) }
        }
    }
}

fn sidebar(context: &PageContext, user: &User) -> Markup {
    let name = user
        .full_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&user.email);
    let approvals_current = context.path.starts_with("/approvals");
    html! {
        aside class="sidebar" id="sidebar" popover {
            div class="sidebar__header" {
                (logo_on_frame())
                button type="button" class="icon-button icon-button--on-frame sidebar__collapse" data-sidebar-toggle
                    aria-pressed=(if context.sidebar_collapsed { "true" } else { "false" }) aria-label="Collapse sidebar" { (icon("panel-left")) }
                button type="button" class="icon-button icon-button--on-frame sidebar__close" popovertarget="sidebar"
                    popovertargetaction="hide" aria-label="Close navigation" { (icon("x")) }
            }
            nav class="sidebar__nav" aria-label="Main navigation" {
                (nav_item(context, "/", "house", "Home"))
                p class="nav-section" { "Gateway" }
                (nav_item(context, "/mcps", "plug", "MCPs"))
                (nav_item(context, "/tokens", "key-round", "Access tokens"))
                @if context.pending_approvals > 0 {
                    a class="nav-item" href="/approvals" aria-current=[approvals_current.then_some("page")]
                        aria-label=(format!("Approvals, {} waiting", context.pending_approvals)) {
                        (icon("shield-check")) span class="nav-item__label" { "Approvals" }
                        span class="nav-item__count" aria-hidden="true" { (context.pending_approvals) }
                    }
                } @else {
                    (nav_item(context, "/approvals", "shield-check", "Approvals"))
                }
                @if user.is_admin() {
                    p class="nav-section" { "Observability" }
                    (nav_item(context, "/logs", "scroll-text", "Logs"))
                    (nav_item(context, "/analytics", "chart-column", "Analytics"))
                }
                p class="nav-section" { "Instance" }
                @if user.is_admin() { (nav_item(context, "/invites", "users", "Team")) }
                (nav_item(context, "/settings", "settings", "Settings"))
            }
            @if let (Some(gateway_url), true) = (context.gateway_url(), context.app_url_configured) {
                div class="gateway-card" {
                    p class="gateway-card__status" { span class="status-dot status-dot--success" {} "Gateway online" }
                    p class="gateway-card__endpoint" {
                        span { (gateway_url.split_once("://").map_or(gateway_url.as_str(), |(_, rest)| rest)) }
                        button type="button" class="gateway-card__copy" data-copy=(gateway_url) aria-label="Copy gateway URL" {
                            (icon("copy")) (icon("check"))
                        }
                    }
                }
            }
            div class="nav-user" {
                span class="avatar avatar--on-frame" { (user.initials()) }
                span class="nav-user__text" {
                    span class="nav-user__name" { (name) }
                    span class="nav-user__role" { (user.role.as_str()) }
                }
                form method="post" action="/logout" {
                    (context.csrf_field())
                    button type="submit" class="icon-button icon-button--on-frame" aria-label="Log out" { (icon("log-out")) }
                }
            }
        }
    }
}

/// A page of the app: the sidebar, and `content` inside the panel. `content`
/// is what goes in `<div class="page">`: the page header, then the sections.
/// Dialogs and side panels of the page go in `overlays`, next to the panel.
pub fn app_page(
    context: &PageContext,
    title: &str,
    content: Markup,
    page_overlays: Markup,
) -> Markup {
    let Some(user) = &context.user else {
        return auth_page(context, title, content);
    };
    html! {
        (DOCTYPE)
        html lang="en" data-sidebar=[context.sidebar_collapsed.then_some("collapsed")] {
            (head(context, title))
            body class="app" {
                a class="skip-link" href="#main" { "Skip to content" }
                header class="topbar" {
                    button type="button" class="icon-button icon-button--on-frame icon-button--lg" popovertarget="sidebar"
                        aria-label="Open navigation" { (icon("menu")) }
                    (logo_on_frame())
                    a class="topbar__account" href="/settings" aria-label="Settings" {
                        span class="avatar avatar--on-frame" { (user.initials()) }
                    }
                }
                (sidebar(context, user))
                main class="panel" id="main" {
                    div class="page" {
                        @if !context.app_url_configured {
                            div class="banner banner--warning" role="status" {
                                (icon("triangle-alert"))
                                div class="banner__content" {
                                    p class="banner__title" { "Set APP_URL to enable public links" }
                                    p { "Define APP_URL as this instance’s public HTTPS origin, then redeploy." }
                                }
                            }
                        }
                        (content)
                    }
                }
                (page_overlays)
                (overlays(context))
            }
        }
    }
}

/// A screen before sign-in (sign in, onboarding, invite, authorize, errors):
/// `content` is one or more `<section class="auth-card">`.
pub fn auth_page(context: &PageContext, title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            (head(context, title))
            body class="auth" {
                main class="auth__main" id="main" {
                    (content)
                    p class="auth__footnote" {
                        "Self-hosted · invite-only" span class="hide-mobile" { " · no public registration" }
                    }
                }
                (overlays(context))
            }
        }
    }
}

/// The card of an error page, for [`auth_page`].
pub fn error_card(icon_name: &str, critical: bool, title: &str, description: &str) -> Markup {
    html! {
        section class="auth-card control-lg" aria-labelledby="error-title" {
            (logo())
            span class=(if critical { "tile tile--round tile--critical" } else { "tile tile--round" }) {
                (crate::views::icon::icon_with(icon_name, "icon--lg"))
            }
            div class="auth-card__heading" {
                h1 class="auth-card__title" id="error-title" { (title) }
                p class="auth-card__subtitle" { (description) }
            }
            a class="button button--primary button--block" href="/" { "Go home" }
        }
    }
}
