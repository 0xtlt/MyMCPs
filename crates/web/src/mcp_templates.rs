//! The MCPs the "Add an MCP" gallery offers to set up, with the values each
//! one prefills in the create form. (`inertia/components/mcp_template_gallery.tsx`)

use mymcps_core::models::{Mcp, McpAuthType, McpTransport};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateCategory {
    Productivity,
    Development,
    Commerce,
    Marketing,
    Infrastructure,
    Health,
}

impl TemplateCategory {
    /// In the order the gallery lists them.
    pub const ALL: [TemplateCategory; 6] = [
        Self::Productivity,
        Self::Development,
        Self::Commerce,
        Self::Marketing,
        Self::Infrastructure,
        Self::Health,
    ];

    /// The value of the category in the gallery's filter.
    pub fn key(self) -> &'static str {
        match self {
            Self::Productivity => "productivity",
            Self::Development => "development",
            Self::Commerce => "commerce",
            Self::Marketing => "marketing",
            Self::Infrastructure => "infrastructure",
            Self::Health => "health",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Productivity => "Productivity",
            Self::Development => "Development",
            Self::Commerce => "Commerce",
            Self::Marketing => "Marketing",
            Self::Infrastructure => "Infrastructure",
            Self::Health => "Health & fitness",
        }
    }
}

/// What a template puts in the create form. Everything it leaves out is the
/// form's default.
#[derive(Debug, Clone, Copy)]
pub struct TemplateValues {
    pub name: &'static str,
    pub description: &'static str,
    pub transport: McpTransport,
    pub http_url: Option<&'static str>,
    pub npm_package: Option<&'static str>,
    pub npm_version: Option<&'static str>,
    pub npm_args: Option<&'static str>,
    /// Names of the environment variables the package needs. Their values
    /// are the admin's to enter.
    pub npm_env: &'static [&'static str],
    pub builtin_key: Option<&'static str>,
    pub builtin_permissions: &'static [&'static str],
    pub auth_type: McpAuthType,
}

const fn http(name: &'static str, description: &'static str, url: &'static str) -> TemplateValues {
    TemplateValues {
        name,
        description,
        transport: McpTransport::Http,
        http_url: Some(url),
        npm_package: None,
        npm_version: None,
        npm_args: None,
        npm_env: &[],
        builtin_key: None,
        builtin_permissions: &[],
        auth_type: McpAuthType::Auto,
    }
}

const fn npm(
    name: &'static str,
    description: &'static str,
    package: &'static str,
    version: &'static str,
) -> TemplateValues {
    TemplateValues {
        name,
        description,
        transport: McpTransport::Npm,
        http_url: None,
        npm_package: Some(package),
        npm_version: Some(version),
        npm_args: None,
        npm_env: &[],
        builtin_key: None,
        builtin_permissions: &[],
        auth_type: McpAuthType::Auto,
    }
}

const fn builtin(
    name: &'static str,
    description: &'static str,
    key: &'static str,
) -> TemplateValues {
    TemplateValues {
        name,
        description,
        transport: McpTransport::Builtin,
        http_url: None,
        npm_package: None,
        npm_version: None,
        npm_args: None,
        npm_env: &[],
        builtin_key: Some(key),
        builtin_permissions: &[],
        auth_type: McpAuthType::Auto,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct McpTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: TemplateCategory,
    pub popular: bool,
    /// The icon of the template, and of the MCPs made from it.
    pub icon: &'static str,
    pub keywords: &'static [&'static str],
    pub values: TemplateValues,
}

impl McpTemplate {
    pub fn is_builtin(&self) -> bool {
        self.values.transport == McpTransport::Builtin
    }

    /// What the gallery's search looks through, in lower case.
    pub fn searchable_text(&self) -> String {
        let mut parts = vec![self.name, self.description, self.category.label()];
        parts.extend(self.keywords);
        parts.join(" ").to_lowercase()
    }
}

/// The gallery, in the order it is shown.
pub const MCP_TEMPLATES: &[McpTemplate] = &[
    McpTemplate {
        id: "notion",
        name: "Notion",
        description: "Search, read, and update pages and databases in your Notion workspace.",
        category: TemplateCategory::Productivity,
        popular: true,
        icon: "notebook-text",
        keywords: &["notes", "docs", "wiki", "database"],
        values: http(
            "Notion",
            "Search, read, and update pages and databases in Notion.",
            "https://mcp.notion.com/mcp",
        ),
    },
    McpTemplate {
        id: "shopify-dev",
        name: "Shopify Dev",
        description: "Build Shopify apps with current API, Liquid, and Polaris development guidance.",
        category: TemplateCategory::Commerce,
        popular: true,
        icon: "shopping-bag",
        keywords: &["store", "ecommerce", "polaris", "liquid"],
        values: npm(
            "Shopify Dev",
            "Shopify development tools and documentation for apps, APIs, Liquid, and Polaris.",
            "@shopify/dev-mcp",
            "latest",
        ),
    },
    McpTemplate {
        id: "github",
        name: "GitHub",
        description: "Work with repositories, issues, pull requests, code, and GitHub Actions.",
        category: TemplateCategory::Development,
        popular: true,
        icon: "git-branch",
        keywords: &["git", "repository", "pull request", "actions"],
        values: http(
            "GitHub",
            "Work with GitHub repositories, issues, pull requests, code, and Actions.",
            "https://api.githubcopilot.com/mcp/",
        ),
    },
    McpTemplate {
        id: "linear",
        name: "Linear",
        description: "Find, create, and update issues, projects, and comments in Linear.",
        category: TemplateCategory::Productivity,
        popular: true,
        icon: "layers",
        keywords: &["issues", "projects", "roadmap", "tasks"],
        values: http(
            "Linear",
            "Find, create, and update issues, projects, and comments in Linear.",
            "https://mcp.linear.app/mcp",
        ),
    },
    McpTemplate {
        id: "stripe",
        name: "Stripe",
        description: "Interact with Stripe payments, customers, products, and developer documentation.",
        category: TemplateCategory::Commerce,
        popular: false,
        icon: "credit-card",
        keywords: &["payments", "billing", "customers", "finance"],
        values: http(
            "Stripe",
            "Interact with Stripe payments, customers, products, and documentation.",
            "https://mcp.stripe.com",
        ),
    },
    McpTemplate {
        id: "supabase",
        name: "Supabase",
        description: "Manage databases, projects, functions, debugging, and Supabase documentation.",
        category: TemplateCategory::Development,
        popular: false,
        icon: "zap",
        keywords: &["postgres", "database", "functions", "backend"],
        values: http(
            "Supabase",
            "Manage Supabase databases, projects, functions, debugging, and documentation.",
            "https://mcp.supabase.com/mcp",
        ),
    },
    McpTemplate {
        id: "cloudflare-docs",
        name: "Cloudflare Docs",
        description: "Search current Cloudflare developer documentation for Workers and the platform.",
        category: TemplateCategory::Infrastructure,
        popular: false,
        icon: "cloud",
        keywords: &["workers", "platform", "documentation", "edge"],
        values: http(
            "Cloudflare Docs",
            "Search current Cloudflare developer documentation for Workers and the platform.",
            "https://docs.mcp.cloudflare.com/mcp",
        ),
    },
    McpTemplate {
        id: "atlassian-rovo",
        name: "Atlassian Rovo",
        description: "Search and manage Jira issues, Confluence pages, and Atlassian work.",
        category: TemplateCategory::Productivity,
        popular: true,
        icon: "ticket",
        keywords: &["jira", "confluence", "issues", "wiki", "rovo"],
        values: http(
            "Atlassian Rovo",
            "Search and manage Jira issues, Confluence pages, and Atlassian work.",
            "https://mcp.atlassian.com/v1/mcp/authv2",
        ),
    },
    McpTemplate {
        id: "postman",
        name: "Postman",
        description: "Manage API collections, workspaces, specifications, mocks, and monitors.",
        category: TemplateCategory::Development,
        popular: true,
        icon: "send",
        keywords: &["api", "collections", "workspaces", "mocks", "monitors"],
        values: http(
            "Postman",
            "Manage Postman API collections, workspaces, specifications, mocks, and monitors.",
            "https://mcp.postman.com/minimal",
        ),
    },
    McpTemplate {
        id: "sentry",
        name: "Sentry",
        description: "Investigate application errors, issues, traces, and performance data.",
        category: TemplateCategory::Development,
        popular: true,
        icon: "bug",
        keywords: &[
            "errors",
            "monitoring",
            "traces",
            "performance",
            "observability",
        ],
        values: http(
            "Sentry",
            "Investigate Sentry application errors, issues, traces, and performance data.",
            "https://mcp.sentry.dev/mcp",
        ),
    },
    McpTemplate {
        id: "microsoft-learn",
        name: "Microsoft Learn",
        description: "Search current Microsoft documentation and retrieve official code samples.",
        category: TemplateCategory::Development,
        popular: true,
        icon: "book-open",
        keywords: &[
            "documentation",
            "code samples",
            "azure",
            "dotnet",
            "windows",
        ],
        values: http(
            "Microsoft Learn",
            "Search current Microsoft documentation and retrieve official code samples.",
            "https://learn.microsoft.com/api/mcp",
        ),
    },
    McpTemplate {
        id: "firebase",
        name: "Firebase",
        description: "Explore, configure, debug, and deploy authenticated Firebase projects.",
        category: TemplateCategory::Infrastructure,
        popular: false,
        icon: "flame",
        keywords: &["google", "backend", "hosting", "functions", "firestore"],
        values: TemplateValues {
            npm_args: Some("mcp"),
            ..npm(
                "Firebase",
                "Explore, configure, debug, and deploy authenticated Firebase projects.",
                "firebase-tools",
                "15.26.0",
            )
        },
    },
    McpTemplate {
        id: "mongodb",
        name: "MongoDB",
        description: "Explore schemas and query MongoDB or Atlas safely in read-only mode.",
        category: TemplateCategory::Infrastructure,
        popular: false,
        icon: "database",
        keywords: &["database", "atlas", "query", "schema", "nosql"],
        values: TemplateValues {
            npm_args: Some("--readOnly"),
            npm_env: &["MDB_MCP_CONNECTION_STRING"],
            ..npm(
                "MongoDB",
                "Explore schemas and query MongoDB or Atlas safely in read-only mode.",
                "mongodb-mcp-server",
                "2.0.0",
            )
        },
    },
    McpTemplate {
        id: "neon",
        name: "Neon",
        description: "Manage Neon Postgres projects, branches, databases, queries, and migrations.",
        category: TemplateCategory::Infrastructure,
        popular: false,
        icon: "server",
        keywords: &["postgres", "database", "branches", "sql", "migrations"],
        values: http(
            "Neon",
            "Manage Neon Postgres projects, branches, databases, queries, and migrations.",
            "https://mcp.neon.tech/mcp",
        ),
    },
    McpTemplate {
        id: "hugging-face",
        name: "Hugging Face",
        description: "Search models, datasets, Spaces, papers, documentation, and jobs.",
        category: TemplateCategory::Development,
        popular: false,
        icon: "bot",
        keywords: &["ai", "models", "datasets", "spaces", "machine learning"],
        values: http(
            "Hugging Face",
            "Search Hugging Face models, datasets, Spaces, papers, documentation, and jobs.",
            "https://huggingface.co/mcp",
        ),
    },
    McpTemplate {
        id: "context7",
        name: "Context7",
        description: "Retrieve current, version-specific library documentation and code examples.",
        category: TemplateCategory::Development,
        popular: false,
        icon: "braces",
        keywords: &[
            "documentation",
            "libraries",
            "code examples",
            "upstash",
            "context",
        ],
        values: TemplateValues {
            auth_type: McpAuthType::Bearer,
            ..http(
                "Context7",
                "Retrieve current, version-specific library documentation and code examples.",
                "https://mcp.context7.com/mcp",
            )
        },
    },
    McpTemplate {
        id: "strava",
        name: "Strava",
        description: "Read your activities, training totals, zones, segments, and routes, with optional write access. Runs inside MyMCPs with your own Strava API application.",
        category: TemplateCategory::Health,
        popular: true,
        icon: "bike",
        keywords: &[
            "running", "cycling", "fitness", "training", "workout", "built-in",
        ],
        values: builtin(
            "Strava",
            "Read Strava activities, training totals, zones, segments, and routes.",
            "strava",
        ),
    },
    McpTemplate {
        id: "icloud-mail",
        name: "iCloud Mail",
        description: "Read and search your iCloud mailboxes, with permissions you choose for drafts, sending, and filing. Runs inside MyMCPs with an app-specific password.",
        category: TemplateCategory::Productivity,
        popular: true,
        icon: "mail",
        keywords: &[
            "email", "mail", "apple", "inbox", "imap", "smtp", "built-in",
        ],
        values: TemplateValues {
            // Read-only until the admin allows more in the setup dialog.
            builtin_permissions: &["read"],
            ..builtin(
                "iCloud Mail",
                "Read, search, draft, send, and file iCloud Mail, within the allowed permissions.",
                "icloud-mail",
            )
        },
    },
    McpTemplate {
        id: "google-ads",
        name: "Google Ads",
        description: "Monitor accounts, campaigns, ads, keywords, and search terms, and let agents build and run Search and Display campaigns with images. Budget and go-live changes wait for your approval. Runs inside MyMCPs with your own Google Cloud OAuth client.",
        category: TemplateCategory::Marketing,
        popular: true,
        icon: "megaphone",
        keywords: &[
            "ads",
            "adwords",
            "sea",
            "ppc",
            "campaigns",
            "keywords",
            "advertising",
            "built-in",
        ],
        values: builtin(
            "Google Ads",
            "Monitor and manage Google Ads campaigns, ad groups, keywords, ads, and image assets.",
            "google-ads",
        ),
    },
];

/// The template with this id.
pub fn find_template(id: &str) -> Option<&'static McpTemplate> {
    MCP_TEMPLATES.iter().find(|template| template.id == id)
}

/// The template that sets up a built-in MCP.
pub fn builtin_template(builtin_key: &str) -> Option<&'static McpTemplate> {
    MCP_TEMPLATES
        .iter()
        .find(|template| template.values.builtin_key == Some(builtin_key))
}

fn host_of(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
}

/// The template an MCP looks made from. Nothing records it: a built-in MCP
/// is matched by its key, an HTTP one by the host it calls and an npm one by
/// its package.
pub fn template_of(mcp: &Mcp) -> Option<&'static McpTemplate> {
    match mcp.transport {
        McpTransport::Builtin => builtin_template(mcp.builtin_key.as_deref()?),
        McpTransport::Http => {
            let host = host_of(mcp.http_url.as_deref()?)?;
            MCP_TEMPLATES.iter().find(|template| {
                template.values.http_url.and_then(host_of).as_deref() == Some(host.as_str())
            })
        }
        McpTransport::Npm => {
            let package = mcp.npm_package.as_deref()?;
            MCP_TEMPLATES
                .iter()
                .find(|template| template.values.npm_package == Some(package))
        }
    }
}

/// The icon of an MCP: the one of its template, else one for its transport.
pub fn icon_of(mcp: &Mcp) -> &'static str {
    match (template_of(mcp), mcp.transport) {
        (Some(template), _) => template.icon,
        (None, McpTransport::Http) => "plug",
        (None, McpTransport::Npm) => "package",
        (None, McpTransport::Builtin) => "blocks",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::views::icon::has_icon;

    #[test]
    fn lists_the_gallery_of_the_app() {
        assert_eq!(MCP_TEMPLATES.len(), 19);
        let ids: HashSet<&str> = MCP_TEMPLATES.iter().map(|template| template.id).collect();
        assert_eq!(ids.len(), MCP_TEMPLATES.len(), "ids are unique");
        assert_eq!(
            MCP_TEMPLATES
                .iter()
                .filter(|template| template.popular)
                .count(),
            11
        );
        for template in MCP_TEMPLATES {
            assert!(has_icon(template.icon), "{}", template.icon);
            assert_ne!(template.id, "custom", "reserved for the blank form");
            match template.values.transport {
                McpTransport::Http => {
                    let url = template.values.http_url.unwrap();
                    assert!(mymcps_net::parse_http_url(url, "MCP URL").is_ok(), "{url}");
                }
                McpTransport::Npm => assert!(template.values.npm_package.is_some()),
                McpTransport::Builtin => {
                    let key = template.values.builtin_key.unwrap();
                    assert!(mymcps_builtin::keys::is_builtin_mcp_key(key), "{key}");
                }
            }
        }
        // Every category of the filter has a template.
        for category in TemplateCategory::ALL {
            assert!(
                MCP_TEMPLATES
                    .iter()
                    .any(|template| template.category == category),
                "{}",
                category.label()
            );
        }
    }

    #[test]
    fn searches_name_description_category_and_keywords() {
        let notion = find_template("notion").unwrap();
        assert_eq!(
            notion.searchable_text(),
            "notion search, read, and update pages and databases in your notion workspace. productivity notes docs wiki database"
        );
        assert!(
            find_template("strava")
                .unwrap()
                .searchable_text()
                .contains("health & fitness")
        );
        assert!(find_template("custom").is_none());
    }

    #[test]
    fn finds_the_template_an_mcp_was_made_from() {
        let mcp = |transport, url: Option<&str>, package: Option<&str>, key: Option<&str>| Mcp {
            transport,
            http_url: url.map(str::to_string),
            npm_package: package.map(str::to_string),
            builtin_key: key.map(str::to_string),
            ..Default::default()
        };
        let http = |url| mcp(McpTransport::Http, Some(url), None, None);
        assert_eq!(
            icon_of(&http("https://mcp.notion.com/mcp")),
            "notebook-text"
        );
        // Another path of the same server is still that server.
        assert_eq!(icon_of(&http("https://MCP.Stripe.com/v2")), "credit-card");
        assert_eq!(icon_of(&http("https://example.com/mcp")), "plug");
        assert_eq!(icon_of(&http("not a url")), "plug");

        let npm = |package| mcp(McpTransport::Npm, None, Some(package), None);
        assert_eq!(icon_of(&npm("@shopify/dev-mcp")), "shopping-bag");
        assert_eq!(icon_of(&npm("@example/mcp")), "package");

        let builtin = |key| mcp(McpTransport::Builtin, None, None, Some(key));
        assert_eq!(icon_of(&builtin("strava")), "bike");
        assert_eq!(icon_of(&builtin("icloud-mail")), "mail");
        assert_eq!(icon_of(&builtin("google-ads")), "megaphone");
        assert_eq!(icon_of(&builtin("garmin")), "blocks");
        // A package name is not a built-in key, and the other way round.
        assert_eq!(
            icon_of(&mcp(
                McpTransport::Npm,
                None,
                Some("strava"),
                Some("strava")
            )),
            "package"
        );
    }
}
