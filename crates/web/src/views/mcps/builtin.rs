//! Setup walkthrough and credentials for an MCP that MyMCPs runs itself. The
//! admin gets the credentials from the provider, so the steps track how far
//! that setup has come. (`inertia/components/builtin_mcp_fields.tsx`)

use maud::{Markup, html};

use crate::assets::asset_url;
use crate::views::icon::icon;
use crate::views::mcps::PublicApp;
use crate::views::mcps::form::{Field, McpForm, banner, choice, optional_mark};

/// Something the provider needs beyond the sign-in, such as the account to act through.
pub struct SettingGuide {
    pub key: &'static str,
    pub label: &'static str,
    pub placeholder: Option<&'static str>,
    pub description: &'static str,
    pub optional: bool,
}

/// What the admin can allow. The provider cannot restrict the password itself.
pub struct PermissionGuide {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

/// The admin registers an API application, then approves access on the provider's site.
pub struct OauthGuide {
    /// Names the first step when the provider does not call it an API application.
    pub create_application_label: Option<&'static str>,
    /// What the admin does on the provider's site before coming back here.
    pub create_application: fn(&PublicApp) -> Markup,
    /// Names the second step when it asks for more than the two credentials.
    pub credentials_label: Option<&'static str>,
    pub client_id_placeholder: &'static str,
    pub credentials_hint: &'static str,
    /// Asked for with the credentials, in this order.
    pub settings: &'static [SettingGuide],
    /// What every authorization can read.
    pub read_access: &'static str,
    /// What agents can change once write access is allowed.
    pub write_access: &'static str,
    /// The provider has one permission for reading and writing, so allowing
    /// write access needs no new authorization.
    pub write_applies_on_save: bool,
}

/// The admin creates a password for apps on the provider's site and pastes it here.
pub struct PasswordGuide {
    /// What the admin does on the provider's site before coming back here.
    pub create_password_label: &'static str,
    pub create_password: fn() -> Markup,
    pub password_label: &'static str,
    pub username_label: &'static str,
    pub username_placeholder: &'static str,
    pub username_hint: &'static str,
    pub password_placeholder: &'static str,
    /// Other addresses of the same account that agents may act as.
    pub aliases_label: &'static str,
    pub aliases_placeholder: &'static str,
    pub aliases_hint: &'static str,
    /// Why the permissions are chosen here and not on the provider's site.
    pub permissions_hint: &'static str,
    pub permissions: &'static [PermissionGuide],
}

pub enum SignIn {
    Oauth(OauthGuide),
    Password(PasswordGuide),
}

impl SignIn {
    /// How the registry names the sign-in.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Oauth(_) => "oauth",
            Self::Password(_) => "password",
        }
    }
}

pub struct SetupGuide {
    pub provider: &'static str,
    /// How the MCP reaches the provider, shown in the registry.
    pub endpoint: &'static str,
    /// What the admin brings to the setup, shown under the dialog title.
    pub requirement: &'static str,
    pub sign_in: SignIn,
}

/// A link in running text that opens the provider's site in another tab.
fn external_link(href: &str, label: &str) -> Markup {
    html! {
        a class="link" href=(href) target="_blank" rel="noopener noreferrer" { (label) (icon("external-link")) }
    }
}

/// A value to enter on the provider's site, with a button that copies it.
fn copy_field(label: &str, value: &str, copy_label: &str) -> Markup {
    html! {
        div class="field" {
            span class="field__label" { (label) }
            div class="copy-field copy-field--wrap" {
                span class="copy-field__value" { (value) }
                button type="button" class="icon-button" data-copy=(value) aria-label=(copy_label) {
                    (icon("copy")) (icon("check"))
                }
            }
        }
    }
}

fn strava_application(public_app: &PublicApp) -> Markup {
    let app_icon = asset_url("brand/mymcps-app-icon.png");
    html! {
        p {
            "Open " (external_link("https://www.strava.com/settings/api", "Strava API settings"))
            " and create an application. Strava states that this requires a Strava subscription. Any name and category work. Enter these two values:"
        }
        (copy_field("Website", &public_app.url, "Copy website"))
        (copy_field("Authorization Callback Domain", &public_app.hostname, "Copy authorization callback domain"))
        p class="text-body-sm" {
            "The callback domain has no https:// and no path. Strava then asks for an application icon (JPG or PNG) before it shows the credentials. This one is ready to upload:"
        }
        // A file of the instance, on an opaque white square: the logo itself
        // is transparent, which a provider that converts uploads to JPG shows on black.
        div class="cluster gap-300" {
            span class="tile tile--lg" { img src=(app_icon) alt="" width="36" height="36"; }
            a class="button button--secondary" href=(app_icon) download="mymcps-app-icon.png" { (icon("download")) "Download app icon" }
        }
    }
}

fn google_ads_application(public_app: &PublicApp) -> Markup {
    html! {
        p {
            "In a " (external_link("https://console.cloud.google.com/apis/library/googleads.googleapis.com", "Google Cloud project"))
            ", enable the Google Ads API and apply for Explorer access on its Google Ads API Overview page: until then the project only reaches test accounts. Set up the OAuth consent screen, then create an OAuth client of the type Web application with this redirect URI:"
        }
        (copy_field("Authorized redirect URI", &format!("{}/mcps/oauth/callback", public_app.url), "Copy authorized redirect URI"))
        p class="text-body-sm" {
            "Publish the consent screen to production: while it is in testing, Google ends every authorization after 7 days. Google has retired developer tokens, so there is none to enter."
        }
    }
}

fn icloud_password() -> Markup {
    html! {
        p {
            "Open " (external_link("https://account.apple.com/account/manage", "your Apple Account"))
            ", select Sign-In and Security, then App-Specific Passwords, and create one named MyMCPs. Apple offers this once two-factor authentication is turned on."
        }
        p class="text-body-sm" {
            "Apple shows the password once, as four groups of letters. You can revoke it from the same page at any time without changing your Apple Account password."
        }
    }
}

static STRAVA: SetupGuide = SetupGuide {
    provider: "Strava",
    endpoint: "Strava API",
    requirement: "Runs inside MyMCPs with your own API application",
    sign_in: SignIn::Oauth(OauthGuide {
        create_application_label: None,
        create_application: strava_application,
        credentials_label: None,
        client_id_placeholder: "123456",
        credentials_hint: "Both are shown on the My API Application page once the application exists.",
        settings: &[],
        read_access: "MyMCPs reads your profile, activities, routes, and segments, including private ones. Permissions you uncheck on Strava hide the matching tools.",
        write_access: "Lets agents create manual activities, edit activity details, star segments, and update your weight.",
        write_applies_on_save: false,
    }),
};

static GOOGLE_ADS: SetupGuide = SetupGuide {
    provider: "Google Ads",
    endpoint: "Google Ads API",
    requirement: "Runs inside MyMCPs with an OAuth client from your own Google Cloud project",
    sign_in: SignIn::Oauth(OauthGuide {
        create_application_label: Some("Create a Google Cloud OAuth client"),
        create_application: google_ads_application,
        credentials_label: Some("Paste its Client ID and Client Secret, and choose the accounts"),
        client_id_placeholder: "1234567890-abc123.apps.googleusercontent.com",
        credentials_hint: "Google shows both when the OAuth client is created. The Client Secret can only be copied then.",
        settings: &[
            SettingGuide {
                key: "loginCustomerId",
                label: "Manager account ID",
                placeholder: Some("123-456-7890"),
                description: "Only when your sign-in reaches the advertiser accounts through a manager account: the ID of that manager.",
                optional: true,
            },
            SettingGuide {
                key: "customerIds",
                label: "Accounts agents may use",
                placeholder: Some("123-456-7890, 234-567-8901"),
                description: "Limits every tool to these Google Ads accounts. Left blank, agents reach every account your sign-in does.",
                optional: true,
            },
        ],
        read_access: "MyMCPs reads the accounts, campaigns, ads, keywords, and statistics that your Google sign-in can open.",
        write_access: "Lets agents create and change campaigns, budgets, ad groups, keywords, ads, and image assets. The tools that commit money or put a campaign live ask for your approval first, which you can change under Tool approvals.",
        write_applies_on_save: true,
    }),
};

static ICLOUD_MAIL: SetupGuide = SetupGuide {
    provider: "iCloud Mail",
    endpoint: "iCloud Mail over IMAP and SMTP",
    requirement: "Runs inside MyMCPs with an app-specific password",
    sign_in: SignIn::Password(PasswordGuide {
        create_password_label: "Create an app-specific password",
        create_password: icloud_password,
        password_label: "App-specific password",
        username_label: "iCloud Mail address",
        username_placeholder: "name@icloud.com",
        username_hint: "The address ending in @icloud.com, @me.com, or @mac.com, even if you sign in to Apple with another one.",
        password_placeholder: "abcd-efgh-ijkl-mnop",
        aliases_label: "Other sender addresses",
        aliases_placeholder: "alias@icloud.com, me@example.com",
        aliases_hint: "Aliases, custom domain addresses, or Hide My Email addresses of this account that agents may also send from, separated by commas. They must already be set up in iCloud Mail.",
        permissions_hint: "Apple cannot limit what an app-specific password reaches, so MyMCPs enforces these permissions itself. Agents only get the tools of the permissions you allow.",
        permissions: &[
            PermissionGuide {
                key: "read",
                label: "Read mail",
                description: "List mailboxes, search, read messages, and get temporary links to download their attachments. Reading does not mark a message as read.",
            },
            PermissionGuide {
                key: "draft",
                label: "Save drafts",
                description: "Write messages, with the files agents attach, to your Drafts mailbox for you to review and send yourself.",
            },
            PermissionGuide {
                key: "send",
                label: "Send mail",
                description: "Send messages from your address, with the files agents attach. A sent message cannot be recalled.",
            },
            PermissionGuide {
                key: "organize",
                label: "Organize mail",
                description: "Mark messages as read or flagged and move them between mailboxes, including to the Trash.",
            },
        ],
    }),
};

/// How to set up a built-in MCP, or `None` for a key this version does not know.
pub fn builtin_setup_guide(builtin_key: Option<&str>) -> Option<&'static SetupGuide> {
    match builtin_key? {
        "strava" => Some(&STRAVA),
        "google-ads" => Some(&GOOGLE_ADS),
        "icloud-mail" => Some(&ICLOUD_MAIL),
        _ => None,
    }
}

pub fn builtin_provider_name(builtin_key: Option<&str>) -> &'static str {
    builtin_setup_guide(builtin_key).map_or("Built-in", |guide| guide.provider)
}

/// What the built-in fields of a dialog are drawn from.
pub(super) struct BuiltinFields<'a> {
    /// What the ids of the dialog start with.
    pub prefix: &'a str,
    pub form: &'a McpForm,
    pub has_saved_client_secret: bool,
    pub has_saved_password: bool,
    pub is_connected: bool,
    pub public_app: Option<&'a PublicApp>,
}

/// One step of the walkthrough. `active` is the step to do now: the ones
/// before it are done.
fn step(index: usize, active: usize, title: &str, title_id: Option<&str>, body: Markup) -> Markup {
    let done = index < active;
    html! {
        li class="step" data-complete[done] aria-current=[(index == active).then_some("step")] {
            div class="step__body" {
                h3 class="step__title" id=[title_id] {
                    @if done { span class="visually-hidden" { "Done: " } }
                    (title)
                }
                (body)
            }
        }
    }
}

/// `loginCustomerId` as `login-customer-id`, for an id.
fn kebab(key: &str) -> String {
    let mut text = String::with_capacity(key.len() + 4);
    for character in key.chars() {
        if character.is_ascii_uppercase() {
            text.push('-');
            text.push(character.to_ascii_lowercase());
        } else {
            text.push(character);
        }
    }
    text
}

fn oauth_steps(fields: &BuiltinFields, guide: &SetupGuide, oauth: &OauthGuide) -> Markup {
    let BuiltinFields { prefix, form, .. } = *fields;
    let values = &form.values;
    // Saved credentials still wait for Connect.
    let active = if fields.is_connected {
        3
    } else if fields.has_saved_client_secret {
        2
    } else {
        0
    };
    let provider = guide.provider;

    let application = html! {
        @match fields.public_app {
            Some(public_app) => { ((oauth.create_application)(public_app)) }
            None => {
                (banner("warning", "Set APP_URL first", html! {
                    p { (provider) " sends you back to this instance after you approve access. Set APP_URL to its public HTTPS origin and redeploy to see the values to enter." }
                }, html! {}))
            }
        }
    };
    let secret_label = html! {
        @if fields.has_saved_client_secret { "Client Secret (leave blank to keep) " (optional_mark()) } @else { "Client Secret" }
    };
    let credentials = html! {
        p class="text-body-sm" { (oauth.credentials_hint) }
        (Field::new(format!("{prefix}-client-id"), "oauthClientId", html! { "Client ID" })
            .value(&values.oauth_client_id)
            .placeholder(oauth.client_id_placeholder)
            .verbatim()
            .error(form.error("oauthClientId"))
            .render())
        (Field::new(format!("{prefix}-client-secret"), "oauthClientSecret", secret_label)
            .password()
            .value(&values.oauth_client_secret)
            .help(html! { "Encrypted at rest and only sent to the provider." })
            .error(form.error("oauthClientSecret"))
            .render())
        @for setting in oauth.settings {
            @let label = html! { (setting.label) @if setting.optional { " " (optional_mark()) } };
            (Field::new(format!("{prefix}-{}", kebab(setting.key)), &format!("builtinSettings[{}]", setting.key), label)
                .value(values.builtin_settings.get(setting.key).map_or("", String::as_str))
                .placeholder_opt(setting.placeholder)
                .help(html! { (setting.description) })
                .error(form.error(&format!("builtinSettings.{}", setting.key)))
                .render())
        }
    };
    let applies = if oauth.write_applies_on_save {
        "It applies as soon as you save."
    } else {
        "Turning it on applies the next time you connect or re-authorize."
    };
    let connect = html! {
        p class="text-body-sm" {
            @if fields.is_connected {
                "Connected. " (oauth.read_access)
            } @else {
                "Once this MCP is saved, select Connect and approve access on " (provider) ". " (oauth.read_access)
            }
        }
        (choice("builtinWriteEnabled", None, values.builtin_write_enabled, "Allow write access",
            Some(&format!("{} {applies}", oauth.write_access))))
    };

    html! {
        ol class="steps steps--detailed" aria-label=(format!("{provider} setup")) {
            (step(0, active, &oauth.create_application_label.map_or_else(|| format!("Create a {provider} API application"), str::to_string), None, application))
            (step(1, active, oauth.credentials_label.unwrap_or("Paste its Client ID and Client Secret"), None, credentials))
            (step(2, active, &format!("Connect your {provider} account"), None, connect))
        }
    }
}

fn password_steps(fields: &BuiltinFields, guide: &SetupGuide, password: &PasswordGuide) -> Markup {
    let BuiltinFields { prefix, form, .. } = *fields;
    let values = &form.values;
    // A saved password that does not work has to be entered again.
    let active = if fields.is_connected {
        3
    } else if fields.has_saved_password {
        1
    } else {
        0
    };
    let provider = guide.provider;

    let password_label = html! {
        @if fields.has_saved_password {
            (password.password_label) " (leave blank to keep) " (optional_mark())
        } @else {
            (password.password_label)
        }
    };
    let sign_in = html! {
        (Field::new(format!("{prefix}-username"), "builtinUsername", html! { (password.username_label) })
            .value(&values.builtin_username)
            .placeholder(password.username_placeholder)
            .inputmode("email")
            .verbatim()
            .help(html! { (password.username_hint) })
            .error(form.error("builtinUsername"))
            .render())
        (Field::new(format!("{prefix}-password"), "builtinPassword", password_label)
            .password()
            .value(&values.builtin_password)
            .placeholder_opt((!fields.has_saved_password).then_some(password.password_placeholder))
            .help(html! { "Encrypted at rest and only sent to the provider's mail servers." })
            .error(form.error("builtinPassword"))
            .render())
        (Field::new(format!("{prefix}-aliases"), "builtinAliases", html! { (password.aliases_label) " " (optional_mark()) })
            .value(&values.builtin_aliases)
            .placeholder(password.aliases_placeholder)
            .verbatim()
            .help(html! { (password.aliases_hint) })
            .error(form.error("builtinAliases"))
            .render())
    };

    let title_id = format!("{prefix}-permissions-title");
    let help_id = format!("{prefix}-permissions-help");
    let permissions = html! {
        p class="text-body-sm" id=(help_id) {
            @if fields.is_connected {
                "Connected. " (password.permissions_hint)
            } @else {
                "Saving this MCP signs in to " (provider) " to check the password. " (password.permissions_hint)
            }
        }
        fieldset class="field-group gap-300" aria-labelledby=(title_id) aria-describedby=(help_id) {
            @for permission in password.permissions {
                (choice("builtinPermissions[]", Some(permission.key),
                    values.builtin_permissions.iter().any(|allowed| allowed == permission.key),
                    permission.label, Some(permission.description)))
            }
        }
        @if let Some(message) = form.error("builtinPermissions") {
            (banner("critical", message, html! { p { "Without a permission, agents would get no tool from this MCP." } }, html! {}))
        }
    };

    html! {
        ol class="steps steps--detailed" aria-label=(format!("{provider} setup")) {
            (step(0, active, password.create_password_label, None, (password.create_password)()))
            (step(1, active, "Enter your address and the password", None, sign_in))
            (step(2, active, "Choose what agents can do", Some(&title_id), permissions))
        }
    }
}

/// What stands for the choices a custom MCP makes with its option cards: a
/// built-in MCP has no transport or authentication to choose.
pub(super) fn builtin_choices(builtin_key: &str) -> Markup {
    html! {
        input type="hidden" name="transport" value="builtin";
        input type="hidden" name="builtinKey" value=(builtin_key);
        input type="hidden" name="authType" value="auto";
    }
}

/// The fields of a built-in MCP: they replace Transport, its fields and
/// Authentication.
pub(super) fn builtin_fields(fields: &BuiltinFields) -> Markup {
    let values = &fields.form.values;
    let guide = builtin_setup_guide(Some(&values.builtin_key));
    html! {
        @match guide {
            None => {
                (banner("critical", "Unknown built-in MCP", html! {
                    p { "This version of MyMCPs does not include this built-in MCP." }
                }, html! {}))
            }
            Some(guide) => {
                @match &guide.sign_in {
                    SignIn::Oauth(oauth) => { (oauth_steps(fields, guide, oauth)) }
                    SignIn::Password(password) => { (password_steps(fields, guide, password)) }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_built_in_mcps_of_this_version() {
        for key in mymcps_builtin::keys::BUILTIN_MCP_KEYS {
            assert!(builtin_setup_guide(Some(key)).is_some(), "{key}");
        }
        assert!(builtin_setup_guide(Some("garmin")).is_none());
        assert!(builtin_setup_guide(None).is_none());
        assert_eq!(builtin_provider_name(Some("icloud-mail")), "iCloud Mail");
        assert_eq!(builtin_provider_name(Some("garmin")), "Built-in");
        assert_eq!(
            builtin_setup_guide(Some("strava")).unwrap().sign_in.label(),
            "oauth"
        );
        assert_eq!(
            builtin_setup_guide(Some("icloud-mail"))
                .unwrap()
                .sign_in
                .label(),
            "password"
        );
    }

    #[test]
    fn writes_ids_from_setting_keys() {
        assert_eq!(kebab("loginCustomerId"), "login-customer-id");
        assert_eq!(kebab("customerIds"), "customer-ids");
    }
}
