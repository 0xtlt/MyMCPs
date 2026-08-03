export type McpClient = 'codex' | 'claude' | 'cursor'

export type McpInstallConfig = {
  code: string
  language: 'bash' | 'json' | 'toml'
  title: string
  restartInstruction: string
  verifyInstruction: string
}

function tomlString(value: string) {
  return JSON.stringify(value)
}

function shellArgument(value: string) {
  return `'${value.replaceAll("'", `'"'"'`)}'`
}

function authorizationHeader(token: string) {
  return `Bearer ${token}`
}

export function createMcpInstallConfig(
  client: McpClient,
  gatewayUrl: string,
  token: string,
  enableLazyToolMode = false
): McpInstallConfig {
  const authorization = authorizationHeader(token)

  if (client === 'codex') {
    return {
      language: 'toml',
      title: '~/.codex/config.toml',
      code: [
        '[mcp_servers.mymcps]',
        `url = ${tomlString(gatewayUrl)}`,
        `http_headers = { Authorization = ${tomlString(authorization)}${enableLazyToolMode ? ', "X-MyMCPs-Tool-Mode" = "lazy"' : ''} }`,
      ].join('\n'),
      restartInstruction: 'Save the file, then restart Codex or restart the IDE extension.',
      verifyInstruction: 'Open /mcp in Codex and confirm that mymcps is connected.',
    }
  }

  if (client === 'claude') {
    const headers = [
      `Authorization: ${authorization}`,
      ...(enableLazyToolMode ? ['X-MyMCPs-Tool-Mode: lazy'] : []),
    ]

    return {
      language: 'bash',
      title: 'Terminal',
      code: [
        `claude mcp add --transport http --scope user mymcps ${shellArgument(gatewayUrl)} \\`,
        ...headers.map(
          (header, index) =>
            `  --header ${shellArgument(header)}${index < headers.length - 1 ? ' \\' : ''}`
        ),
        'claude mcp get mymcps',
      ].join('\n'),
      restartInstruction: 'Restart Claude Code after the command completes.',
      verifyInstruction: 'Run /mcp in Claude Code and confirm that mymcps is connected.',
    }
  }

  return {
    language: 'json',
    title: '~/.cursor/mcp.json',
    code: JSON.stringify(
      {
        mcpServers: {
          mymcps: {
            type: 'http',
            url: gatewayUrl,
            headers: {
              Authorization: authorization,
              ...(enableLazyToolMode ? { 'X-MyMCPs-Tool-Mode': 'lazy' } : {}),
            },
          },
        },
      },
      null,
      2
    ),
    restartInstruction: 'Save the file, then restart Cursor.',
    verifyInstruction: 'Open Cursor MCP settings and confirm that mymcps is enabled.',
  }
}
