# Let an agent manage your MCPs

An agent connected to MyMCPs can do more than call the tools of your MCPs: with your permission, it can also add, change, delete and test the MCPs of a team, and give you the link that connects one that signs in with OAuth. It does with tools what you do on the **MCPs** page.

These tools are off unless you turn them on for one connection.

## Give the permission

1. Connect the agent to `https://your-domain.example/mcp` with OAuth, as usual.
2. On the consent screen, choose the **Team**, then tick **Allow this agent to manage your MCPs**. The box is unchecked to begin with, whatever the client asks for.
3. Select **Authorize client**.

Only an admin or an owner of the team can give the permission: the box is only shown to someone who is an admin or an owner of at least one of their teams, and the consent is refused when it is ticked for a team where they are only a member.

The permission belongs to that connection alone. Access tokens made by hand on **Access tokens** never have it, and neither do other connections of the same client.

## Take it back

- On **Access tokens**, a connection that has the permission shows **Manages MCPs**. Select **Remove MCP management** on its row: the connection keeps calling the tools of your MCPs and can no longer manage them.
- Revoking the connection takes everything back.
- To give the permission again, connect the client again and tick the box.

The gateway checks the permission at every request, not when the connection was made: once it is removed, or once the person who gave it is no longer an admin or an owner of the team, the tools leave the list of tools at the next request, and a call to one is refused.

## The tools

| Tool | What it does |
| --- | --- |
| `mcp_registry_list` | Lists every MCP of the team, the disabled ones included, with its slug, status and sign-in. |
| `mcp_registry_create` | Adds an HTTP MCP: name, description, URL, sign-in (`auto`, `bearer`, `header`), and whether it is enabled. It is tested once saved, as on the page. |
| `mcp_registry_update` | Changes an MCP, named by its slug. What is left out keeps its saved value, secrets included. For an npm MCP: its name, description and whether it is enabled. |
| `mcp_registry_delete` | Deletes an MCP, named by its slug. |
| `mcp_registry_test` | Tests the connection, as **Test connection** does, and answers the status, the error, and whether the MCP waits for OAuth. |
| `mcp_registry_auth_link` | Gives the link that connects an MCP that signs in with OAuth. |

They are listed in both tool modes, eager and lazy.

What the tools accept is what the form of the **MCPs** page accepts: the same checks run on both, with the exceptions below. Built-in MCPs (Strava, iCloud Mail, Google Ads) are listed, tested, deleted and connected through these tools, but created and changed on the page only.

One access token may call these tools 30 times a minute.

### Secrets

The tools take secrets and never give them back. A bearer token or a header value is answered as set or not, never with its value, and so is each environment variable of an npm MCP, and neither are the tokens of an OAuth connection. The URL of an MCP is shown as its origin (`https://mcp.example/…`): its user name, password, path and query can all hold a key. The arguments of an npm MCP are only counted, since people put keys there too. The arguments of these tools are kept out of the call log.

As on the page, an MCP forgets its saved secrets when its URL moves to another origin. The answer of `mcp_registry_update` names the secrets it cleared, so the agent can send them again.

### Connect an MCP that signs in with OAuth

The agent cannot sign in to an MCP's provider for you. `mcp_registry_auth_link` (and `mcp_registry_test`, when the MCP waits for OAuth) answers a link to the MCP in MyMCPs, `https://your-domain.example/mcps/<id>`:

1. Open the link while signed in to MyMCPs, with the team of the connection selected.
2. The MCP's dialog opens. Select **Connect** and approve the access at the provider.
3. Ask the agent to test the MCP again.

### npm MCPs are the user's to add

An npm MCP runs the code of a package on your server. These tools wait for nobody to approve what they do, so they never run code: `mcp_registry_create` refuses `transport: "npm"` and every `npm_*` argument, and `mcp_registry_update` refuses to switch an MCP to npm and to change the package, the version, the arguments or the environment variables of an npm MCP. The agent is told to ask you to do it on the **MCPs** page. The sandbox is not what stops it: on macOS and Windows it exists only with `NPM_SANDBOX=off`, which is no sandbox at all.

The environment is held back with the package: an environment variable can name the address a package sends its key to, so changing one is changing where the keys the package holds go. What the agent can still do with an npm MCP is list it, rename it, change its description, enable or disable it, test it, and delete it. The tool approvals of **MCPs** are not used for this: they hold the call of a tool of an MCP, and show the arguments of the call to the person who decides, which here would be the secrets.

### Addresses

The agent may only set the URL of an MCP to a public address. A URL whose host is a loopback, private, link-local or carrier-grade NAT address (`127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16` with the metadata service of the clouds, `::1`, `fc00::/7`, `fe80::/10`, and the IPv4-mapped forms of those), or a name that resolves to one, is refused: what that address answers, or the error it causes, comes back to the agent, so it would be a way to read the network of the server. An MCP at such an address is added on the **MCPs** page, where you are the one who decides. An agent can still test, rename, enable, disable and delete it, but it cannot move it to another address of that network.

The check is made when the address is set. A name that points to a public address then and to a private one later (DNS rebinding) is not caught. Redirects are only followed inside the origin of the MCP.

## What the agent reads

The names, descriptions and errors these tools answer come from the MCPs and their servers, and an MCP you add can say anything in them, an instruction to the agent included. The tools tell the agent to take them as data, which a model may or may not do: add MCPs of servers you trust, and tick the box below only for an agent whose other tools you trust as much.

## Before you tick the box

An agent with this permission can do what a team admin does to HTTP MCPs. In particular, it can add an MCP of any public address and give it the credentials it is handed, and whatever MCP it adds is enabled and its tools reach the other agents of the team. These tools do not wait for an approval: tick the box only for agents you trust with that.
