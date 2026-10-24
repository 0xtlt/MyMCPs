# Tailnets

A team can connect [Tailscale](https://tailscale.com/) tailnets, and choose one on an HTTP MCP. The MCP is then reached through the tailnet: its URL can name a device of your tailnet (`https://grafana.tail1234.ts.net/mcp`) or an address behind one of its subnet routers, which no public network answers.

MyMCPs joins a tailnet as one device of its own. It does not change the network of the server: the device is a `tailscaled` process that MyMCPs starts and keeps, in userspace networking, with no TUN interface, no root and no change to the routes or the DNS of the machine.

## Turn it on

Tailnets need `tailscaled`, the Tailscale daemon. The Docker image has it, and `TAILSCALED_BINARY` points to it. On a host install, `TAILSCALED_BINARY` names the binary; unset, the `tailscaled` of the `PATH` is used. Without one, with `TAILSCALED_BINARY=off`, or with a path that is not an executable file, tailnets are off: the **Tailnets** page says so and nothing can be connected. Restart MyMCPs after a change.

The device of each tailnet keeps its key in `tailnets/<id>/` of the data directory (`/app/tmp` in the image), readable by the server alone. Persist that directory as you persist the database: without it, every tailnet asks for a new sign-in after a restart. It is not part of a [backup](backup.md): the tailnets of an imported backup are listed, and each needs a sign-in on the new instance.

## Connect a tailnet

Only an owner or an admin of the team sees **Tailnets** in the navigation.

1. Open **Tailnets** and select **Connect a tailnet**.
2. Give it a **Name**, such as `pro`: letters, digits, spaces, dots, dashes and underscores, 40 characters at most, unique in the team. The device is called `mymcps-pro` in the tailnet, and Tailscale adds a number when another device already has that name.
3. Choose how the device signs in:
   - **Sign in with Tailscale**. Select **Connect**; the row says **Needs sign-in**. Select **Sign in** on the row: the page of your Tailscale account opens in a new tab. Sign in and approve the device. The row turns to **Connected** by itself.
   - **Auth key**. Paste a key from the Tailscale admin console (**Settings**, **Keys**, **Generate auth key**). Prefer a one-off key, with a tag (below): it is used once, to join. MyMCPs never stores it, never logs it and never shows it again: the form comes back empty when something is wrong, and the key is typed again.
4. **Control server** is only there when the operator allows other control servers than Tailscale's (see [Control servers](#control-servers)). Leave it empty for Tailscale. For a Headscale, see below.

A team connects up to 5 tailnets. The page refreshes every 5 seconds while a tailnet is starting, needs a sign-in or needs an approval; **Connect a tailnet** then opens the form on its own page, which does not refresh, so that nothing typed is lost.

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

When the team has a tailnet, the dialog of an HTTP MCP has a **Network** field: **Internet**, or **Tailnet: pro**. Save the MCP; it is tested through the tailnet. On a tailnet the URL may name a device by its MagicDNS short name (`http://nas:8080/mcp`) as well as by its full name or its Tailscale address; on the internet a host needs a domain with a top-level domain. The list of MCPs names the tailnet under the URL. Changing the network of an MCP, from the internet to a tailnet, back, or from one tailnet to another, is like moving it to another server: the same URL may name another machine there, so the MCP forgets its saved bearer token, header value and OAuth sign-in, and they are entered or connected again. A sign-in or a token renewal that is still waiting for its provider at that moment is dropped too: what the provider issues afterwards is not saved, and the MCP is connected again. Only HTTP MCPs have a network: an npm MCP runs on the server, and a built-in one talks to its provider.

An agent that has the permission to [manage MCPs](mcp-management-tools.md) reads and sets the network by tailnet name, with `tailnet` on `mcp_registry_create` and `mcp_registry_update`.

## Delete a tailnet

Select **Delete** on its row. A tailnet that MCPs use is not deleted: the message names them, and each goes back to **Internet** or to another tailnet first. Deleting asks the device to log out of the tailnet, stops its `tailscaled` and deletes its state, which holds its key. The device stays in your admin console, to be removed there, in three cases:

- The control server could not be reached for the logout. The deletion goes on: the state is deleted all the same.
- The control server of the tailnet is not allowed any more (see [Control servers](#control-servers)). The logout would need a daemon that server steers, so none is started and the state is only deleted.
- Tailscale is off on this server (see [Turn it on](#turn-it-on)), for a tailnet that joined before it was turned off. There is no daemon to log out with, so the state is only deleted, and the message that follows the deletion says that the device could not be logged out. A tailnet that has no state on this server, such as one that came with a backup and never signed in here, is deleted without that message.

Deleting a team, or a user whose personal team goes with them, does the same for each of its tailnets, with one message for all of them when Tailscale is off.

## What an MCP can reach

An MCP that has a tailnet reaches the devices of that tailnet and the subnet routes its devices advertise through it, and its own server is reached nowhere else. Every request to the origin of the MCP (scheme, host and port) goes through the tailnet, or fails: a host that the tailnet does not carry is refused, rather than sent to the internet. That holds for every request, not only the first: if the tailnet stops carrying the server of an MCP in the middle of a session (a subnet route is withdrawn, the device leaves the tailnet), the next request fails with an error that names the tailnet and is never sent over the internet, so the bearer token or header of the MCP cannot leave through the public network.

The other servers of an OAuth sign-in (the authorization, token and registration endpoints, and the provider's metadata, when they are not on the origin of the MCP) are reached by the tailnet when it carries them, and otherwise over the public network, under two rules:

- **Whatever belongs to the tailnet is the tailnet's or nobody's.** A host that is an IP address (a public one, behind a subnet route that advertises it, included), a name of a single label (`nas`), or a name under `.ts.net` or under the MagicDNS suffix of the tailnet is never sent over the public network. When the tailnet carries it, the request goes through the tailnet. When it does not, for instance because its route was withdrawn between the start of the sign-in and the exchange of the code or the refresh of the token, the request fails, and the authorization code, its PKCE verifier, the refresh token and the client secret go nowhere. A public OAuth provider must therefore be named by a DNS name (`auth.example.com`), not by an IP address.
- **The public network is only public addresses.** A request that is not carried by the tailnet is refused when its host is an IP address in a loopback, private, link-local, carrier-grade NAT or otherwise special-purpose range, whatever way it is written, and when its name resolves to one at the moment of the connection. So a provider cannot point the sign-in at the network of the server, such as `http://127.0.0.1:8080/` or the metadata address `http://169.254.169.254/`, unless the tailnet itself routes that address, which it never does for loopback, link-local and its own addresses (below).

The device is set to refuse connections from the tailnet (`ShieldsUp`), so it only dials out. It does not use an exit node. The addresses of MyMCPs' own device are never reachable either, even when a subnet route of the tailnet covers them: an MCP cannot loop back to the server through its own tailnet address. That check reads a fresh status of the device right before every connection, not the status kept for two seconds. What stays is the time between that read and the connection being made, which is the round trip of the request to the local daemon: an address the control server gives to the device in that very moment is still dialled. Giving the device an address is the control server's to do, so the control servers a team may use are the operator's to choose (below).

Two things to keep in mind:

- The device has the access of what it signs in as. With a sign-in link it is a device of the person who signs in, with that person's access in your policy. Give it less: join with an auth key that carries a tag, and let the policy of your tailnet say what that tag reaches.
- An agent that can manage MCPs can add an MCP on your tailnet by name, with any path on any device the device reaches. The address check that keeps agents off the private network of the server resolves names with the DNS of the server, which does not know the names of your devices. Allow the permission only to agents you trust with the tailnet, and keep the tag of MyMCPs from reaching more than the MCP servers. An agent cannot take such an MCP off its tailnet to reach the private network of the server: whenever the network of an MCP changes, its address goes through the same check as a new one.

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

Set **Control server** to the address of your [Headscale](https://headscale.net/), such as `https://headscale.example.com`, once the operator has allowed it (below). It must be an `http` or `https` address with no user name, password or query. The server connecting to it is `tailscaled`, which also reaches private addresses, so a Headscale on your LAN works.

- With **Auth key**, create a pre-authorised key on the Headscale (`headscale preauthkeys create --user <user>`) and paste it.
- With **Sign in with Tailscale**, the link of the row is the registration page of your Headscale: it shows a `headscale nodes register` command to run there.

The access rules are the ones of the policy file of your Headscale. The device of a Headscale is named like any other: `mymcps-pro`.

## Control servers

Unless the operator allows more, a tailnet uses Tailscale's own control server. The reason: a control server steers the `tailscaled` of its tailnet, and can make it send HTTP requests of its own (redirects, ping requests, calls to the node) to any address the server can reach, loopback, the metadata address of a cloud and the LAN included. Those requests come from the daemon, not from an MCP, so the routing of the tailnet, which only guards what MCPs send, does not see them. A team that could name any control server could aim them at the network of the instance.

`TAILNET_CONTROL_URLS` in the environment of MyMCPs (restart after a change) says which other servers a team may use:

| Value | Effect |
| --- | --- |
| unset | Tailscale's only. The Connect dialog has no **Control server** field. |
| `https://hs.example.com,http://10.0.0.2:8080` | Tailscale's, and those origins (scheme, host and port: a path is ignored). The dialog has a **Control server** list: **Tailscale**, then each origin. |
| `any` | Tailscale's, and any `http` or `https` server. The dialog has a **Control server** field to type an address in. |

The server refuses any other value, whatever the form sends. A tailnet whose saved control server is not allowed any more (the variable changed, or the tailnet came with an imported backup) is not started and cannot sign in again: its row says **Error**, with the reason, and **Sign in again** gives the same message. Allow the server again, or delete the tailnet and connect it again.

Allow only control servers you trust, or `any` on an instance whose teams you trust. One risk stays, even for an allowed server: the DERP map of a tailnet comes from its own control server, and `tailscaled` connects to the relays in it. A control server can name a relay at a private address, and the daemon dials it. Treat an allowed control server as able to make the daemon connect to the network of the instance, and run the instance where that is no risk (a network of its own, no access to cloud metadata) when you allow servers that are not yours.

## Limits

- 5 tailnets a team, each one a `tailscaled` process of its own.
- HTTP MCPs only.
- Linux and macOS: the tailnet support is not compiled for Windows.
- A tailnet belongs to one team. Another team connects the same tailnet again if it needs it, as a second device.
- The auth key is read once. A key that is refused shows up as **Needs sign-in** or **Error** on the row, not on the form.

## Contributing: the end-to-end test

`crates/tailnet/tests/headscale-e2e.sh` proves with real software that MyMCPs reaches an HTTP MCP through a tailnet. Nothing in it is a fake and nothing touches a Tailscale account. It starts, in Docker and on a private network of its own:

- a Headscale, the control server (pinned by tag),
- a device: the real `tailscaled`, joined with a pre-auth key, that serves a small streamable-HTTP MCP (`crates/tailnet/tests/headscale-e2e/mcp-device.js`),
- the MyMCPs image, built from your checkout, which joins the same tailnet with the `tailscaled` of the image. The script sets `TAILNET_CONTROL_URLS` to the address of its Headscale for that container, since MyMCPs only connects to Tailscale otherwise.

It then drives MyMCPs over HTTP like a person: onboarding and sign-in, the **Tailnets** page, the MCP form and an access token. It checks that the tailnet reaches **Connected**; that MCPs on it, by short name, full name and Tailscale address, save as ready and list the tools of the device; that a tool call through the gateway reaches the device, which sees it come from a tailnet address; that an unknown name, a public host, the Docker address of the device and a tailnet address that no device has are refused with the tailnet message, and that the short name and the tailnet address of the device, entered on **Internet** instead, are refused or not reachable; and that the sign-in link leads to the register page of the Headscale and, once registered there, turns the row **Connected**.

```sh
crates/tailnet/tests/headscale-e2e.sh
```

It needs Docker, `curl`, `jq` and the Internet, and takes about a minute when the layers of the image are cached, longer when the image has to compile. It prints one `PASS` per check and `ALL GREEN` at the end, and, unless `E2E_KEEP=1`, removes its containers, volumes and network when it ends, whether it passed or failed. Environment: `MYMCPS_IMAGE` uses an image you built instead of building one, `HEADSCALE_TAG` and `TAILSCALE_TAG` choose the versions, and `E2E_KEEP=1` leaves the containers running to look around (remove them with `docker rm -f -v $(docker ps -aq --filter name=mymcps-e2e-)`).

Peers connect directly on the Docker network: the DERP relay that a Headscale embeds needs TLS, which the test does not set up, so the script lets the device and MyMCPs find their direct path before the first request. A real tailnet needs no such step.
