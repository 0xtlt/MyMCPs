export type McpClient = 'codex' | 'claude' | 'cursor'
export type McpInstallAuthMode = 'oauth' | 'token'

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
  enableLazyToolMode = false,
  authMode: McpInstallAuthMode = 'token'
): McpInstallConfig {
  const authorization = authMode === 'token' ? authorizationHeader(token) : null

  if (client === 'codex') {
    return {
      language: 'toml',
      title: '~/.codex/config.toml',
      code: [
        '[mcp_servers.mymcps]',
        `url = ${tomlString(gatewayUrl)}`,
        ...(authorization || enableLazyToolMode
          ? [
              `http_headers = { ${authorization ? `Authorization = ${tomlString(authorization)}` : ''}${authorization && enableLazyToolMode ? ', ' : ''}${enableLazyToolMode ? '"X-MyMCPs-Tool-Mode" = "lazy"' : ''} }`,
            ]
          : []),
      ].join('\n'),
      restartInstruction: 'Save the file, then restart Codex or restart the IDE extension.',
      verifyInstruction: 'Open /mcp in Codex and confirm that mymcps is connected.',
    }
  }

  if (client === 'claude') {
    const headers = [
      ...(authorization ? [`Authorization: ${authorization}`] : []),
      ...(enableLazyToolMode ? ['X-MyMCPs-Tool-Mode: lazy'] : []),
    ]
    const addCommand = `claude mcp add --transport http --scope user mymcps ${shellArgument(gatewayUrl)}`

    return {
      language: 'bash',
      title: 'Terminal',
      code: [
        headers.length > 0 ? `${addCommand} \\` : addCommand,
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
            ...(authorization || enableLazyToolMode
              ? {
                  headers: {
                    ...(authorization ? { Authorization: authorization } : {}),
                    ...(enableLazyToolMode ? { 'X-MyMCPs-Tool-Mode': 'lazy' } : {}),
                  },
                }
              : {}),
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
