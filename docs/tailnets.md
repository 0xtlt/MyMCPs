# Tailnets

A team can connect [Tailscale](https://tailscale.com/) tailnets, and choose one on an HTTP MCP. The MCP is then reached through the tailnet: its URL can name a device of your tailnet (`https://grafana.tail1234.ts.net/mcp`) or an address behind one of its subnet routers, which no public network answers.

MyMCPs joins a tailnet as one device of its own. It does not change the network of the server: the device lives inside MyMCPs, in userspace, with no TUN interface, no root and no change to the routes or the DNS of the machine.

## Turn it on

Tailnets need `tailscaled`, the Tailscale daemon. The Docker image has it, and `TAILSCALED_BINARY` points to it. On a host install, `TAILSCALED_BINARY` names the binary; unset, the `tailscaled` of the `PATH` is used. Without one, or with `TAILSCALED_BINARY=off`, tailnets are off: the **Tailnets** page says so and nothing can be connected. Restart MyMCPs after a change.

The device of each tailnet keeps its key in `tailnets/<id>/` of the data directory (`/app/tmp` in the image), readable by the server alone. Persist that directory as you persist the database: without it, every tailnet asks for a new sign-in after a restart. It is not part of a [backup](backup.md): the tailnets of an imported backup are listed, and each needs a sign-in on the new instance.

## Connect a tailnet

Only an owner or an admin of the team sees **Tailnets** in the navigation.

1. Open **Tailnets** and select **Connect a tailnet**.
2. Give it a **Name**, such as `pro`: letters, digits, spaces, dots, dashes and underscores, 40 characters at most, unique in the team. The device is called `mymcps-pro` in the tailnet, and Tailscale adds a number when another device already has that name.
3. Choose how the device signs in:
   - **Sign in with Tailscale**. Select **Connect**; the row says **Needs sign-in**. Select **Sign in** on the row: the page of your Tailscale account opens in a new tab. Sign in and approve the device. The row turns to **Connected** by itself.
   - **Auth key**. Paste a key from the Tailscale admin console (**Settings**, **Keys**, **Generate auth key**). Prefer a one-off key, with a tag (below): it is used once, to join. MyMCPs never stores it, never logs it and never shows it again: the form comes back empty when something is wrong, and the key is typed again.
4. Leave **Control server URL** empty for Tailscale. For a Headscale, see below.

A team connects up to 5 tailnets. The page refreshes every 5 seconds while a tailnet starts or waits for its sign-in.

| Status | Meaning |
| --- | --- |
| Starting | `tailscaled` is starting, or has not answered yet. |
| Needs sign-in | The device must sign in: select **Sign in**. The link of the control server is only good for a while; if it stops working, select **Sign in again** for a new one. |
| Needs approval | The device signed in and waits for an admin to approve it in the admin console of your tailnet. |
| Connected | The device is in the tailnet. Its name is shown in **Device**. |
| Stopped | The device is logged in and switched off. Select **Sign in again** to bring it back. |
| Error | `tailscaled` cannot start or keeps exiting. The row says why. **Sign in again** tries once more, and asks for a new sign-in. |
| Unavailable | Tailscale is off on this server. |

## Sign in again

A tailnet that is not **Connected** has a way back that keeps its name and the MCPs that use it: **Sign in again** on its row, for an owner or an admin. Use it when the sign-in link expired or does not work, when the key of the device expired, when the tailnet came with an imported backup, or when it is **Stopped** or in **Error**. It is not offered while the device is **Connected** or waits for an approval.

Select it, then **Sign in** on the row once the new link shows up (a few seconds), and approve the device as for a new tailnet. The control server, the name of the device and the MCPs of the tailnet do not change. The connections the MCPs kept through the tailnet end, and are opened again when they are next used. A device that signed in with an auth key signs in with a link this time: the key was used once and is not kept.

## Choose it on an MCP

When the team has a tailnet, the dialog of an HTTP MCP has a **Network** field: **Internet**, or **Tailnet: pro**. Save the MCP; it is tested through the tailnet. The list of MCPs names the tailnet under the URL. Changing the network of an MCP, from the internet to a tailnet, back, or from one tailnet to another, is like moving it to another server: the same URL may name another machine there, so the MCP forgets its saved bearer token, header value and OAuth sign-in, and they are entered or connected again. Only HTTP MCPs have a network: an npm MCP runs on the server, and a built-in one talks to its provider.

An agent that has the permission to [manage MCPs](mcp-management-tools.md) reads and sets the network by tailnet name, with `tailnet` on `mcp_registry_create` and `mcp_registry_update`.

## Delete a tailnet

Select **Delete** on its row. A tailnet that MCPs use is not deleted: the message names them, and each goes back to **Internet** or to another tailnet first. Deleting logs the device out of the tailnet, stops its `tailscaled` and deletes its key. If the control server could not be reached for the logout, the device stays in your admin console: remove it there.

Deleting a team, or a user whose personal team goes with them, does the same for its tailnets.

## What an MCP can reach

An MCP that has a tailnet reaches the devices of that tailnet and the subnet routes its devices advertise, and nothing else through it: a host that is neither is refused, and so is a name that is not one of the tailnet's, rather than sent to the internet. The device only dials out; it accepts no connection from the tailnet. It does not use an exit node. The addresses of MyMCPs' own device are never reachable either, even when a subnet route of the tailnet covers them: an MCP cannot loop back to the server through its own tailnet address.

Two things to keep in mind:

- The device has the access of what it signs in as. With a sign-in link it is a device of the person who signs in, with that person's access in your policy. Give it less: join with an auth key that carries a tag, and let the policy of your tailnet say what that tag reaches.
- An agent that can manage MCPs can add an MCP on your tailnet by name, with any path on any device the device reaches. The address check that keeps agents off the private network of the server resolves names with the DNS of the server, which does not know the names of your devices. Allow the permission only to agents you trust with the tailnet, and keep the tag of MyMCPs from reaching more than the MCP servers.

### Tailscale policy

Give the device a tag, such as `tag:mymcps`, and allow it the servers and ports of your MCPs only. In the policy file of the admin console:

```json
{
  "tagOwners": { "tag:mymcps": ["autogroup:admin"] },
  "acls": [
    { "action": "accept", "src": ["tag:mymcps"], "dst": ["tag:mcp-servers:443"] }
  ]
}
```

Then generate the auth key with the tag `tag:mymcps`. A tagged device has no key expiry. A device signed in as a person expires like the others (180 days by default), and the row goes back to **Needs sign-in**: select **Sign in again** for a new link, then disable the key expiry of the device in the admin console, or use a tag.

For a subnet route, `dst` is the address and port behind the router (`192.168.1.0/24:8080`), and the router must advertise the route and have it approved in the admin console.

## Headscale

Set **Control server URL** to the address of your [Headscale](https://headscale.net/), such as `https://headscale.example.com`. It must be an `http` or `https` address with no user name, password or query. The server connecting to it is `tailscaled`, which also reaches private addresses, so a Headscale on your LAN works.

- With **Auth key**, create a pre-authorised key on the Headscale (`headscale preauthkeys create --user <user>`) and paste it.
- With **Sign in with Tailscale**, the link of the row is the registration page of your Headscale: it shows a `headscale nodes register` command to run there.

The access rules are the ones of the policy file of your Headscale. The device of a Headscale is named like any other: `mymcps-pro`.

## Limits

- 5 tailnets a team, each one a `tailscaled` process of its own.
- HTTP MCPs only.
- Linux and macOS: the tailnet support is not compiled for Windows.
- A tailnet belongs to one team. Another team connects the same tailnet again if it needs it, as a second device.
- The auth key is read once. A key that is refused shows up as **Needs sign-in** or **Error** on the row, not on the form.
