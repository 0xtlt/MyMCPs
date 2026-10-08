//! The handlers, one module for each part of the app. A module says which
//! routes it has, and who may reach them, in [`FeatureRoutes`]; `app.rs`
//! puts every module's routes behind the right layers.

use axum::Router;

use crate::state::AppState;

pub mod analytics;
pub mod approvals;
pub mod auth;
pub mod builtin_files;
pub mod gateway;
pub mod home;
pub mod invites;
pub mod logs;
pub mod mcp_tools;
pub mod mcps;
pub mod oauth_server;
pub mod settings;
pub mod tokens;

/// The routes of one part of the app, by who may reach them.
#[derive(Default)]
pub struct FeatureRoutes {
    /// Called by programs, not by a person's browser: no session, no CSRF
    /// token, no parsed body. The handler authenticates the request itself
    /// (a bearer token, a signed link). Reachable before onboarding.
    pub machine: Router<AppState>,
    /// Called by programs once the instance is set up, with a parsed body
    /// but no session and no CSRF token: the OAuth protocol endpoints.
    pub protocol: Router<AppState>,
    /// Pages anyone may open once the instance is set up. The handler looks
    /// at who is signed in itself.
    pub open: Router<AppState>,
    /// Pages for visitors who are not signed in. A signed-in person is sent home.
    pub guest: Router<AppState>,
    /// Pages of a signed-in person. A visitor is sent to sign in.
    pub signed_in: Router<AppState>,
    /// Pages of an administrator. Anyone else is sent home with a message.
    pub admin: Router<AppState>,
    /// First-run pages, gone once an administrator exists.
    pub first_run: Router<AppState>,
}

impl FeatureRoutes {
    pub fn merge(mut self, other: FeatureRoutes) -> Self {
        self.machine = self.machine.merge(other.machine);
        self.protocol = self.protocol.merge(other.protocol);
        self.open = self.open.merge(other.open);
        self.guest = self.guest.merge(other.guest);
        self.signed_in = self.signed_in.merge(other.signed_in);
        self.admin = self.admin.merge(other.admin);
        self.first_run = self.first_run.merge(other.first_run);
        self
    }
}

/// Every route of the app.
pub fn all() -> FeatureRoutes {
    [
        analytics::routes(),
        approvals::routes(),
        auth::routes(),
        builtin_files::routes(),
        gateway::routes(),
        home::routes(),
        invites::routes(),
        logs::routes(),
        mcp_tools::routes(),
        mcps::routes(),
        oauth_server::routes(),
        settings::routes(),
        tokens::routes(),
    ]
    .into_iter()
    .fold(FeatureRoutes::default(), FeatureRoutes::merge)
}
