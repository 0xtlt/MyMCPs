//! The configuration that connects an MCP client to this gateway, as the
//! install dialog of the Tokens page shows it for each client.
//! (`inertia/components/mcp_install_config.ts`)

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClient {
    Codex,
    Claude,
    Cursor,
}

impl McpClient {
    pub const ALL: [McpClient; 3] = [McpClient::Codex, McpClient::Claude, McpClient::Cursor];

    /// The name in identifiers of the page.
    pub fn key(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Cursor => "Cursor",
        }
    }

    /// How a token is escaped where this client's configuration holds it.
    pub fn token_format(self) -> TokenFormat {
        match self {
            Self::Codex | Self::Cursor => TokenFormat::Json,
            Self::Claude => TokenFormat::Shell,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpInstallAuthMode {
    Oauth,
    Token,
}

impl McpInstallAuthMode {
    /// The value of the `auth` field of the install dialog.
    pub fn key(self) -> &'static str {
        match self {
            Self::Oauth => "oauth",
            Self::Token => "token",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Bash,
    Json,
    Toml,
}

/// The place a token takes in a configuration: inside a JSON or TOML string,
/// or inside a single-quoted shell word. The page script escapes a token
/// typed in the dialog the same way (`data-bind-format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenFormat {
    Json,
    Shell,
}

impl TokenFormat {
    /// The value of `data-bind-format`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Shell => "shell",
        }
    }

    /// The token as it is written inside its string or its quoted word.
    pub fn escape(self, token: &str) -> String {
        match self {
            Self::Json => {
                let quoted = toml_string(token);
                quoted[1..quoted.len() - 1].to_string()
            }
            Self::Shell => token.replace('\'', SHELL_QUOTE),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpInstallConfig {
    pub code: String,
    pub language: Language,
    pub title: &'static str,
    pub restart_instruction: &'static str,
    pub verify_instruction: &'static str,
}

/// A single quote inside a single-quoted shell word: close the word, add a
/// quoted quote, open it again.
const SHELL_QUOTE: &str = "'\"'\"'";

/// A basic TOML string is written like a JSON string.
fn toml_string(value: &str) -> String {
    Value::String(value.to_string()).to_string()
}

fn shell_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', SHELL_QUOTE))
}

fn authorization_header(token: &str) -> String {
    format!("Bearer {token}")
}

pub fn create_mcp_install_config(
    client: McpClient,
    gateway_url: &str,
    token: &str,
    enable_lazy_tool_mode: bool,
    auth_mode: McpInstallAuthMode,
) -> McpInstallConfig {
    let authorization =
        (auth_mode == McpInstallAuthMode::Token).then(|| authorization_header(token));

    match client {
        McpClient::Codex => {
            let mut lines = vec![
                "[mcp_servers.mymcps]".to_string(),
                format!("url = {}", toml_string(gateway_url)),
            ];
            if authorization.is_some() || enable_lazy_tool_mode {
                let mut headers = Vec::new();
                if let Some(authorization) = &authorization {
                    headers.push(format!("Authorization = {}", toml_string(authorization)));
                }
                if enable_lazy_tool_mode {
                    headers.push("\"X-MyMCPs-Tool-Mode\" = \"lazy\"".to_string());
                }
                lines.push(format!("http_headers = {{ {} }}", headers.join(", ")));
            }
            McpInstallConfig {
                language: Language::Toml,
                title: "~/.codex/config.toml",
                code: lines.join("\n"),
                restart_instruction: "Save the file, then restart Codex or restart the IDE extension.",
                verify_instruction: "Open /mcp in Codex and confirm that mymcps is connected.",
            }
        }
        McpClient::Claude => {
            let mut headers = Vec::new();
            if let Some(authorization) = &authorization {
                headers.push(format!("Authorization: {authorization}"));
            }
            if enable_lazy_tool_mode {
                headers.push("X-MyMCPs-Tool-Mode: lazy".to_string());
            }
            let add_command = format!(
                "claude mcp add --transport http --scope user mymcps {}",
                shell_argument(gateway_url)
            );

            let mut lines = vec![if headers.is_empty() {
                add_command
            } else {
                format!("{add_command} \\")
            }];
            for (index, header) in headers.iter().enumerate() {
                let continued = if index < headers.len() - 1 { " \\" } else { "" };
                lines.push(format!("  --header {}{continued}", shell_argument(header)));
            }
            lines.push("claude mcp get mymcps".to_string());
            McpInstallConfig {
                language: Language::Bash,
                title: "Terminal",
                code: lines.join("\n"),
                restart_instruction: "Restart Claude Code after the command completes.",
                verify_instruction: "Run /mcp in Claude Code and confirm that mymcps is connected.",
            }
        }
        McpClient::Cursor => {
            let mut server = Map::new();
            server.insert("type".into(), json!("http"));
            server.insert("url".into(), json!(gateway_url));
            if authorization.is_some() || enable_lazy_tool_mode {
                let mut headers = Map::new();
                if let Some(authorization) = &authorization {
                    headers.insert("Authorization".into(), json!(authorization));
                }
                if enable_lazy_tool_mode {
                    headers.insert("X-MyMCPs-Tool-Mode".into(), json!("lazy"));
                }
                server.insert("headers".into(), Value::Object(headers));
            }
            let document = json!({ "mcpServers": { "mymcps": server } });
            McpInstallConfig {
                language: Language::Json,
                title: "~/.cursor/mcp.json",
                // Two spaces of indentation, as `JSON.stringify(value, null, 2)`.
                code: serde_json::to_string_pretty(&document).unwrap_or_default(),
                restart_instruction: "Save the file, then restart Cursor.",
                verify_instruction: "Open Cursor MCP settings and confirm that mymcps is enabled.",
            }
        }
    }
}

/// A configuration cut where its token stands, for a page that lets the
/// token be typed: `before`, the token escaped with
/// [`McpClient::token_format`], then `after`, make the configuration.
/// `None` when the configuration holds no token (OAuth).
pub fn split_at_token(
    client: McpClient,
    gateway_url: &str,
    enable_lazy_tool_mode: bool,
    auth_mode: McpInstallAuthMode,
) -> (McpInstallConfig, Option<(String, String)>) {
    // Private-use characters: no escaping touches them, and no gateway URL holds them.
    const MARK: &str = "\u{e000}token\u{e000}";
    let config =
        create_mcp_install_config(client, gateway_url, MARK, enable_lazy_tool_mode, auth_mode);
    let parts = config
        .code
        .split_once(MARK)
        .map(|(before, after)| (before.to_string(), after.to_string()));
    (config, parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `createMcpInstallConfig` of the Node app returns for the same
    /// arguments (Node 24).
    #[test]
    fn writes_the_configurations_of_the_node_app() {
        let cases = [
            (
                McpClient::Codex,
                "https://mcp.example.com/mcp",
                "mcp_Zk8rT2mQ9vLx4HdW7cNpY1sB6fGjK3uA0eRtCw5MhXo",
                false,
                McpInstallAuthMode::Oauth,
                "[mcp_servers.mymcps]\nurl = \"https://mcp.example.com/mcp\"",
            ),
            (
                McpClient::Codex,
                "https://mcp.example.com/mcp",
                "mcp_Zk8rT2mQ9vLx4HdW7cNpY1sB6fGjK3uA0eRtCw5MhXo",
                true,
                McpInstallAuthMode::Oauth,
                "[mcp_servers.mymcps]\nurl = \"https://mcp.example.com/mcp\"\nhttp_headers = { \"X-MyMCPs-Tool-Mode\" = \"lazy\" }",
            ),
            (
                McpClient::Codex,
                "https://mcp.example.com/mcp",
                "token'quoted",
                false,
                McpInstallAuthMode::Token,
                "[mcp_servers.mymcps]\nurl = \"https://mcp.example.com/mcp\"\nhttp_headers = { Authorization = \"Bearer token'quoted\" }",
            ),
            (
                McpClient::Codex,
                "https://it's.example/m\"cp",
                "a\"b\\c\té\u{1}",
                true,
                McpInstallAuthMode::Token,
                "[mcp_servers.mymcps]\nurl = \"https://it's.example/m\\\"cp\"\nhttp_headers = { Authorization = \"Bearer a\\\"b\\\\c\\té\\u0001\", \"X-MyMCPs-Tool-Mode\" = \"lazy\" }",
            ),
            (
                McpClient::Claude,
                "https://mcp.example.com/mcp",
                "unused",
                false,
                McpInstallAuthMode::Oauth,
                "claude mcp add --transport http --scope user mymcps 'https://mcp.example.com/mcp'\nclaude mcp get mymcps",
            ),
            (
                McpClient::Claude,
                "https://mcp.example.com/mcp",
                "unused",
                true,
                McpInstallAuthMode::Oauth,
                "claude mcp add --transport http --scope user mymcps 'https://mcp.example.com/mcp' \\\n  --header 'X-MyMCPs-Tool-Mode: lazy'\nclaude mcp get mymcps",
            ),
            (
                McpClient::Claude,
                "https://mcp.example.com/mcp",
                "token'quoted",
                false,
                McpInstallAuthMode::Token,
                "claude mcp add --transport http --scope user mymcps 'https://mcp.example.com/mcp' \\\n  --header 'Authorization: Bearer token'\"'\"'quoted'\nclaude mcp get mymcps",
            ),
            (
                McpClient::Claude,
                "https://it's.example/m\"cp",
                "a\"b\\c\té\u{1}",
                true,
                McpInstallAuthMode::Token,
                "claude mcp add --transport http --scope user mymcps 'https://it'\"'\"'s.example/m\"cp' \\\n  --header 'Authorization: Bearer a\"b\\c\té\u{1}' \\\n  --header 'X-MyMCPs-Tool-Mode: lazy'\nclaude mcp get mymcps",
            ),
            (
                McpClient::Cursor,
                "https://mcp.example.com/mcp",
                "unused",
                false,
                McpInstallAuthMode::Oauth,
                "{\n  \"mcpServers\": {\n    \"mymcps\": {\n      \"type\": \"http\",\n      \"url\": \"https://mcp.example.com/mcp\"\n    }\n  }\n}",
            ),
            (
                McpClient::Cursor,
                "https://mcp.example.com/mcp",
                "unused",
                true,
                McpInstallAuthMode::Oauth,
                "{\n  \"mcpServers\": {\n    \"mymcps\": {\n      \"type\": \"http\",\n      \"url\": \"https://mcp.example.com/mcp\",\n      \"headers\": {\n        \"X-MyMCPs-Tool-Mode\": \"lazy\"\n      }\n    }\n  }\n}",
            ),
            (
                McpClient::Cursor,
                "<YOUR_GATEWAY_URL>",
                "<YOUR_ACCESS_TOKEN>",
                false,
                McpInstallAuthMode::Token,
                "{\n  \"mcpServers\": {\n    \"mymcps\": {\n      \"type\": \"http\",\n      \"url\": \"<YOUR_GATEWAY_URL>\",\n      \"headers\": {\n        \"Authorization\": \"Bearer <YOUR_ACCESS_TOKEN>\"\n      }\n    }\n  }\n}",
            ),
            (
                McpClient::Cursor,
                "https://it's.example/m\"cp",
                "a\"b\\c\té\u{1}",
                true,
                McpInstallAuthMode::Token,
                "{\n  \"mcpServers\": {\n    \"mymcps\": {\n      \"type\": \"http\",\n      \"url\": \"https://it's.example/m\\\"cp\",\n      \"headers\": {\n        \"Authorization\": \"Bearer a\\\"b\\\\c\\té\\u0001\",\n        \"X-MyMCPs-Tool-Mode\": \"lazy\"\n      }\n    }\n  }\n}",
            ),
        ];
        for (client, gateway_url, token, lazy, auth_mode, code) in cases {
            let config = create_mcp_install_config(client, gateway_url, token, lazy, auth_mode);
            assert_eq!(config.code, code, "{client:?} {auth_mode:?} lazy={lazy}");
        }
    }

    #[test]
    fn names_the_file_and_the_steps_of_each_client() {
        let config =
            |client| create_mcp_install_config(client, "u", "t", false, McpInstallAuthMode::Token);
        let codex = config(McpClient::Codex);
        assert_eq!(codex.language, Language::Toml);
        assert_eq!(codex.title, "~/.codex/config.toml");
        assert_eq!(
            codex.restart_instruction,
            "Save the file, then restart Codex or restart the IDE extension."
        );
        assert_eq!(
            codex.verify_instruction,
            "Open /mcp in Codex and confirm that mymcps is connected."
        );

        let claude = config(McpClient::Claude);
        assert_eq!(claude.language, Language::Bash);
        assert_eq!(claude.title, "Terminal");
        assert_eq!(
            claude.restart_instruction,
            "Restart Claude Code after the command completes."
        );
        assert_eq!(
            claude.verify_instruction,
            "Run /mcp in Claude Code and confirm that mymcps is connected."
        );

        let cursor = config(McpClient::Cursor);
        assert_eq!(cursor.language, Language::Json);
        assert_eq!(cursor.title, "~/.cursor/mcp.json");
        assert_eq!(
            cursor.restart_instruction,
            "Save the file, then restart Cursor."
        );
        assert_eq!(
            cursor.verify_instruction,
            "Open Cursor MCP settings and confirm that mymcps is enabled."
        );
    }

    /// The page shows a configuration in three parts so that the script can
    /// rewrite the token: put back together, they are the configuration.
    #[test]
    fn a_configuration_cut_at_its_token_is_the_same_configuration() {
        let tokens = [
            "mcp_Zk8rT2mQ9vLx4HdW7cNpY1sB6fGjK3uA0eRtCw5MhXo",
            "token'quoted",
            "a\"b\\c\té\u{1}",
            "<YOUR_ACCESS_TOKEN>",
        ];
        for client in McpClient::ALL {
            for lazy in [false, true] {
                for token in tokens {
                    let whole = create_mcp_install_config(
                        client,
                        "https://it's.example/m\"cp",
                        token,
                        lazy,
                        McpInstallAuthMode::Token,
                    );
                    let (config, parts) = split_at_token(
                        client,
                        "https://it's.example/m\"cp",
                        lazy,
                        McpInstallAuthMode::Token,
                    );
                    let (before, after) = parts.expect("a token configuration holds a token");
                    assert_eq!(
                        format!("{before}{}{after}", client.token_format().escape(token)),
                        whole.code,
                        "{client:?} lazy={lazy} {token:?}"
                    );
                    assert_eq!(config.title, whole.title);
                }

                let (config, parts) = split_at_token(
                    client,
                    "https://mcp.example.com/mcp",
                    lazy,
                    McpInstallAuthMode::Oauth,
                );
                assert_eq!(parts, None);
                assert_eq!(
                    config,
                    create_mcp_install_config(
                        client,
                        "https://mcp.example.com/mcp",
                        "ignored",
                        lazy,
                        McpInstallAuthMode::Oauth
                    )
                );
            }
        }
    }
}
