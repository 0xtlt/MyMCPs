//! The screens of the gateway's OAuth server: the consent a signed-in person
//! gives an MCP client, and the page that sends the browser back to it.

use maud::{DOCTYPE, Markup, html};

use crate::assets::asset_url;
use crate::views::icon::{icon, logo};
use crate::views::shell::{PageContext, auth_page};

/// What the consent screen shows and sends back.
pub struct Consent<'a> {
    pub client_name: &'a str,
    /// Host and port of the redirect URI.
    pub redirect_host: &'a str,
    pub is_loopback_redirect: bool,
    pub scope: &'a str,
    pub user_email: &'a str,
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub state: Option<&'a str>,
    pub code_challenge: &'a str,
    pub resource: &'a str,
}

/// `GET /authorize` for a signed-in person.
pub fn authorize_page(context: &PageContext, consent: &Consent<'_>) -> Markup {
    let content = html! {
        // The form is sent by the page's script, which then takes the browser
        // to the client: the fragment is only there for that.
        section class="auth-card auth-card--wide control-lg" aria-labelledby="authorize-title" data-fragment {
            (logo())
            div class="auth-card__heading" {
                h1 class="auth-card__title" id="authorize-title" { "Authorize " (consent.client_name) }
                p class="auth-card__subtitle" { "Signed in as " (consent.user_email) }
            }

            div class="well" {
                h2 { "Allow access to MyMCPs?" }
                p class="text-secondary" {
                    "This client will be able to call every enabled MCP through your gateway. You can revoke the connection at any time from Access tokens."
                }
                div class="cluster cluster--between" {
                    p class="cluster" {
                        span class="text-body-md-medium" { "Permission" }
                        code class="code code--info" { (consent.scope) }
                    }
                    p class="text-body-sm text-tertiary" { "Callback: " (consent.redirect_host) }
                }
            }

            @if consent.is_loopback_redirect {
                div class="banner banner--warning" role="status" {
                    (icon("triangle-alert"))
                    div class="banner__content" {
                        p class="banner__title" { "Local callback" }
                        p {
                            "After approval, the authorization code will be sent to " (consent.redirect_host)
                            ". Only continue if you started this connection from an MCP client on this device."
                        }
                    }
                }
            }

            // One form, two submits: the button pressed sends decision=deny or decision=approve.
            form class="auth-card__actions" method="post" action="/authorize" data-async {
                (context.csrf_field())
                input type="hidden" name="client_id" value=(consent.client_id);
                input type="hidden" name="redirect_uri" value=(consent.redirect_uri);
                input type="hidden" name="response_type" value="code";
                input type="hidden" name="code_challenge" value=(consent.code_challenge);
                input type="hidden" name="code_challenge_method" value="S256";
                input type="hidden" name="scope" value=(consent.scope);
                input type="hidden" name="resource" value=(consent.resource);
                @if let Some(state) = consent.state.filter(|state| !state.is_empty()) {
                    input type="hidden" name="state" value=(state);
                }
                button type="submit" class="button button--secondary" name="decision" value="deny" { "Cancel" }
                button type="submit" class="button button--primary" name="decision" value="approve" { "Authorize client" }
            }
        }
    };
    auth_page(context, "Authorize MCP client", content)
}

/// What a consent form posted without the page's script is answered with: a
/// page that sends the browser on to the client, by itself and through a
/// link. The policy of the pages (`form-action 'self'`) stops a form from
/// being answered with a redirect to another site, and it stays that way.
///
/// The page is a whole document of its own: the instruction to move on
/// belongs in its head, and it runs no script.
pub fn leaving_page(client_name: &str, redirect_host: &str, location: &str) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="robots" content="noindex, nofollow, noarchive, nosnippet";
                meta name="googlebot" content="noindex, nofollow, noarchive, nosnippet";
                meta http-equiv="refresh" content=(format!("0;url={location}"));
                title { "Returning to " (client_name) " · MyMCPs" }
                link rel="icon" href=(asset_url("brand/favicon.svg")) type="image/svg+xml";
                link rel="alternate icon" href="/favicon.png" type="image/png";
                link rel="stylesheet" href=(asset_url("css/app.css"));
            }
            body class="auth" {
                main class="auth__main" id="main" {
                    section class="auth-card control-lg" aria-labelledby="leaving-title" {
                        (logo())
                        div class="auth-card__heading" {
                            h1 class="auth-card__title" id="leaving-title" { "Returning to " (client_name) }
                            p class="auth-card__subtitle" {
                                "Your answer is on its way to " (redirect_host)
                                ". Continue there if this page does not move on by itself."
                            }
                        }
                        a class="button button--primary button--block" id="oauth-continue" href=(location) rel="noreferrer" {
                            "Continue"
                        }
                    }
                    p class="auth__footnote" {
                        "Self-hosted · invite-only" span class="hide-mobile" { " · no public registration" }
                    }
                }
            }
        }
    }
}
