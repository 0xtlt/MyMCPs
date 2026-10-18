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
| `mcp_registry_create` | Adds an HTTP or npm MCP: name, description, URL or npm package (version, arguments, environment variables), sign-in (`auto`, `bearer`, `header`), and whether it is enabled. It is tested once saved, as on the page. |
| `mcp_registry_update` | Changes an MCP, named by its slug. What is left out keeps its saved value, secrets included. |
| `mcp_registry_delete` | Deletes an MCP, named by its slug. |
| `mcp_registry_test` | Tests the connection, as **Test connection** does, and answers the status, the error, and whether the MCP waits for OAuth. |
| `mcp_registry_auth_link` | Gives the link that connects an MCP that signs in with OAuth. |

They are listed in both tool modes, eager and lazy.

What the tools accept is what the form of the **MCPs** page accepts: the same checks run on both. Built-in MCPs (Strava, iCloud Mail, Google Ads) are listed, tested, deleted and connected through these tools, but created and changed on the page only.

### Secrets

The tools take secrets and never give them back. A bearer token, a header value or the value of an environment variable is answered as set or not, never with its value, and neither are the tokens of an OAuth connection. In the URL of an MCP, the user name, the password and the values of the query are replaced. The arguments of these tools are kept out of the call log.

As on the page, an MCP forgets its saved secrets when its URL moves to another origin or it runs another npm package. The answer of `mcp_registry_update` names the secrets it cleared, so the agent can send them again.

### Connect an MCP that signs in with OAuth

The agent cannot sign in to an MCP's provider for you. `mcp_registry_auth_link` (and `mcp_registry_test`, when the MCP waits for OAuth) answers a link to the MCP in MyMCPs, `https://your-domain.example/mcps/<id>`:

1. Open the link while signed in to MyMCPs, with the team of the connection selected.
2. The MCP's dialog opens. Select **Connect** and approve the access at the provider.
3. Ask the agent to test the MCP again.

## Before you tick the box

An agent with this permission can do what a team admin does to MCPs. In particular, it can add an npm MCP, which runs the code of an npm package on your server (in its sandbox on Linux), or point an MCP at an address of its choice and send it the credentials it is given. These tools do not wait for an approval: tick the box only for agents you trust with that.
