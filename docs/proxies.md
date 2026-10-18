# Proxies

Some MCPs do not listen on the internet. They listen on a private network, a [Tailscale](https://tailscale.com/) tailnet for one, and only a machine on that network can reach them. A MyMCPs instance in a container on a server is not on your tailnet, so it cannot.

A proxy fixes that. You add one under **Proxies**, MyMCPs joins your tailnet as a device of its own, and you choose that proxy on any HTTP MCP that lives there. The MCP's requests then go through the tailnet. Nothing is installed on your server, and the container needs no privilege.

Setup takes about five minutes: create an auth key at Tailscale, paste it into MyMCPs, and pick the proxy on the MCP.

## What MyMCPs runs

Tailscale is the only kind of proxy for now.

- **One Tailscale node per proxy.** Each proxy is a device of your tailnet, named after its **Node name** (`mymcps` to begin with). MyMCPs starts a `tailscaled` process for it, in the image or at `TAILSCALED_BINARY`.
- **Userspace, no privilege.** The node creates no network interface and needs no capability, no `/dev/net/tun`, and no change to the container. `docker-compose.yml` is the same with or without proxies.
- **Its state is under `DATA_DIR/tailscale`**, one directory per proxy, private to the user the server runs as. In the image that is `/app/tmp/tailscale`, in the volume you already keep.
- **Nothing can connect into MyMCPs.** The node runs with Tailscale's `ShieldsUp`, which refuses every incoming connection from the tailnet. MyMCPs only ever reaches out.
- **It accepts subnet routes.** A subnet that another device of your tailnet advertises, such as a home network, is reachable through the proxy.

## Before you start

- **A Tailscale account**, with a tailnet where the MCP you want to reach is.
- **An auth key**, from [login.tailscale.com/admin/settings/keys](https://login.tailscale.com/admin/settings/keys). It is optional, but without one the proxy waits for you to follow a login link. See [Add the proxy](#2-add-the-proxy).
- **A role of owner or admin** in the team. Members do not see the **Proxies** page.
- **Outbound access** from your instance to the internet, for Tailscale to reach its servers. It needs no inbound port.

## 1. Create an auth key

1. Open [Keys](https://login.tailscale.com/admin/settings/keys) in the Tailscale admin console and select **Generate auth key**.
2. Check **Reusable**.
3. Under **Tags**, add a tag such as `tag:mymcps`. You may have to create the tag first, see [Limit what the proxy reaches](#limit-what-the-proxy-reaches).
4. Generate the key and copy it. Tailscale shows it once, as `tskey-auth-…`.

Why this combination:

- **Tagged.** The devices you log in yourself belong to you, and their key **expires after 180 days** unless you turn expiry off. A tagged device belongs to the tag instead, and its key does not expire. A proxy that stops working every six months would be a poor one.
- **Reusable.** A key that works once cannot log the node in again. That matters when its state is lost, for example after you [import a backup](#backups) or recreate the volume. With a reusable key, MyMCPs logs in again by itself.

MyMCPs keeps the key, encrypted, only to log the node in again when it needs to. OAuth client secrets (`tskey-client-…`) are not accepted yet: create an auth key.

## 2. Add the proxy

1. Open **Proxies** and select **Add proxy**.
2. Enter a **Name** for yourself, such as `Home`.
3. Keep the **Node name** or choose another: lowercase letters, digits, and hyphens. It is the name of the device in your tailnet.
4. Paste the **Auth key**, or leave it empty.
5. Select **Add proxy**.

The proxy starts at once, and its **Status** goes from **Starting** to one of the following. The page does not refresh by itself: turn on **Live** above the list to follow it, or reload.

| Status                    | Meaning                                                                                                                                                  |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Connected**             | The node is in your tailnet. Its name and addresses are shown on the page. MCPs can use it.                                                             |
| **Needs login**           | There is no auth key, or it did not work. Select **Log in to Tailscale** in the banner above the list, or in the **⋯** menu, to sign in in a new tab. Or edit the proxy and paste a key. While Tailscale has not given a link, the banner says so and offers **Edit**. |
| **Waiting for approval**  | The node is logged in, but your tailnet asks an admin to approve new devices. Approve it under **Machines** in the Tailscale admin console.             |
| **Starting**              | The process is starting. This lasts a few seconds.                                                                                                       |
| **Error**                 | The process keeps stopping, or Tailscale refused the key. A banner above the list says why, with a **Reconnect** button.                                 |

**Reconnect**, in the **⋯** menu, restarts the process and logs in again, with the key or with a new login link. Editing a proxy and saving it applies a changed **Node name**; leaving **Auth key** empty keeps the saved one.

## Limit what the proxy reaches

By default a device of a tailnet may reach every other one. A key that sits in a MyMCPs database should reach as little as it can. The tag from the key is what the [policy file](https://login.tailscale.com/admin/acls/file) is written against. This example lets `tag:mymcps` reach port 443 of the machines tagged `tag:t3`, and nothing else:

```json
{
  "tagOwners": {
    "tag:mymcps": ["autogroup:admin"],
    "tag:t3": ["autogroup:admin"]
  },
  "grants": [
    {
      "src": ["tag:mymcps"],
      "dst": ["tag:t3"],
      "ip": ["443"]
    }
  ]
}
```

Tag the machines the MCPs live on (`tag:t3` here), and check that no other rule of the file, such as the default one that allows everything, still lets `tag:mymcps` through. The proxy refuses nothing itself: the policy of your tailnet decides.

## 3. Use it on an MCP

Proxies work for **HTTP** MCPs. Open the MCP's dialog under **MCPs**: when the team has a proxy, a **Network** select follows the URL. It says **Direct** to begin with. Choose the proxy and save.

- **Everything of the MCP goes through the proxy**: the calls to its endpoint, the checks of its status, and its OAuth sign-in (discovery, registration, the exchange of the code, and the refresh of tokens).
- **Use the full MagicDNS name**, such as `https://box.tailnet.ts.net/mcp`. The form refuses a name without a dot, so `https://box/mcp` does not save.
- **`https://` names need HTTPS certificates** enabled for the tailnet (**DNS → HTTPS Certificates** in the Tailscale admin console), and a machine that serves one, for example with `tailscale serve`. Otherwise use `http://100.x.y.z:port`, the Tailscale address of the machine. Both are encrypted by WireGuard between your instance and the machine, whatever the scheme says.
- **If the proxy is not connected, the call fails**, with a sentence such as `Proxy "Home" is not connected: it needs a Tailscale login`. It never goes out directly instead.
- An MCP that uses a proxy keeps its OAuth connection when you choose a proxy or go back to **Direct**: it is the same server.

Which way a request goes:

| Where it goes                                 | What happens                                               |
| --------------------------------------------- | ---------------------------------------------------------- |
| A machine or subnet of your tailnet           | Through the proxy                                          |
| A public address that is not on your tailnet  | Leaves MyMCPs directly                                     |
| A private address that is not on your tailnet | Refused, so the container's own network stays out of reach |

## Example: T3 Code

[T3 Code](https://github.com/pingdotgg/t3code) runs coding agents on a machine and has an MCP of its own. This reaches it from MyMCPs, and from there from any AI client.

1. **Put T3 on your tailnet.** On the machine that runs it, either:
   - `t3 serve --tailscale-serve`, which serves it at `https://<machine>.<tailnet>.ts.net` (HTTPS certificates must be enabled for the tailnet), or
   - `t3 serve --host <the machine's tailnet IP> --port 3773`, which listens on the Tailscale address only, at `http://<tailnet IP>:3773`.
2. **Add the proxy** as above, with a key whose policy lets it reach that machine.
3. **Add the MCP.** Under **MCPs**, **Add MCP**, choose the **HTTP** transport, and enter the URL `<address>/mcp`, for example `https://box.tailnet.ts.net/mcp`. Choose **OAuth** as the authentication, and the proxy under **Network**. Save.
4. **Select Connect.** T3's sign-in page opens in your browser. Your device has to be on the tailnet to see it, because it is a tailnet address. Approve the connection with a **pairing code**: make one in T3 under **Settings → Connections**, or with `t3 auth pairing create`.
5. **Choose what the agent may do**, from **Read only** up to **Full access**. That is T3's own setting: MyMCPs can still ask for your approval on single tools with [tool approvals](tool-approvals.md).

The MCP then lists the tools of T3, such as `t3_project_list`, `t3_thread_launch`, `t3_thread_send`, `t3_thread_read`, `t3_thread_wait`, `t3_pending_request_respond`, and `orchestrator_capabilities`. An agent can start a thread in a project, send it a message, wait for it, and answer what it asks.

Two limits to know:

- **A tool call through MyMCPs is cut at 60 seconds.** A thread can take longer, so ask your agents to call `t3_thread_wait` with a `timeoutMs` of at most `50000`, and to call it again until the thread is done.
- **A T3 sign-in lasts 30 days**, and T3 gives no refresh token. After that the MCP says it needs to be authorized again: select **Re-authorize** on the MCP and approve with a new pairing code.

## Limits

- At most **5 proxies per team**, and at most `TAILSCALE_MAX_PROXIES` on the instance (20 to begin with). `TAILSCALE_MAX_PROXIES=0` turns Tailscale proxies off.
- Owners and admins of the team manage proxies. A member does not see them, and uses MCPs that use one like any other.
- A proxy belongs to a team and only serves MCPs of that team.
- **HTTP MCPs only.** npm MCPs and built-in MCPs ignore proxies. An npm MCP cannot use one even if it wanted to, see [Security](#security).
- A proxy that MCPs use cannot be deleted: the page lists them. Set those MCPs to another network first. Deleting a proxy logs its node out of the tailnet and removes its state; remove the device in the Tailscale admin console if it still shows there.

## Security

- **The node's private key is on disk**, in `DATA_DIR/tailscale`, readable only by the user the server runs as. Treat the volume like the rest of the data directory. Anyone who has the key can be that device, as far as the policy of your tailnet allows.
- **Nothing connects in.** `ShieldsUp` refuses incoming connections from the tailnet, and the node opens no port on the container's network.
- **The proxy is in the server's process, behind a secret.** MyMCPs sends its requests to a small proxy that listens on the loopback interface, and that proxy asks for a random secret, held in memory only: not in a file, not in an environment variable, not in a log. A sandboxed npm MCP does not know it, so it cannot use your tailnet through the server.
- **An MCP that uses a proxy is treated like an MCP on a private address.** You chose that network, so the OAuth endpoints it announces are accepted whatever they resolve to, as for an MCP you entered at a private address. Only choose a proxy for MCPs you trust.
- **A request that leaves the tailnet** goes out directly only to a public address. A private address that is not on your tailnet is refused, so a proxy cannot be used to reach the container's own network.
- **Limit the tag** in your tailnet's policy, as above. It is the strongest control you have.

## Backups

A [backup](backup.md) holds the proxies and their encrypted auth keys. It does not hold the state of the nodes, which is under `DATA_DIR/tailscale` and is not exported. After an import, each node logs in again as a new device with its saved key if the key is reusable, and shows **Needs login** with a login link if there is none or it cannot be used again. Remove the old devices in the Tailscale admin console.

## Troubleshooting

| What you see                                                     | What to do                                                                                                                                                                                              |
| ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| "tailscaled was not found"                                       | The server cannot start the Tailscale daemon. The image holds it. Running from source, install Tailscale or set `TAILSCALED_BINARY` to the path of `tailscaled`, and restart.                         |
| "Tailscale proxies are off"                                      | `TAILSCALE_MAX_PROXIES` is `0` on this instance. Set it higher and restart.                                                                                                                             |
| **Add proxy** is disabled                                        | Tailscale is unavailable or off (the banner says why), the team has 5 proxies, or the instance has reached `TAILSCALE_MAX_PROXIES`.                                                                          |
| **Needs login** with a login link                                | The proxy has no key, or Tailscale refused it. Select **Log in to Tailscale**, sign in, and approve the device. Or edit the proxy and paste an auth key.                                                |
| **Needs login** stays after you pasted a key                     | The key has expired, was revoked, or was a single-use key already used. Create a new reusable key, paste it, and select **Reconnect**.                                                                  |
| **Waiting for approval**                                         | Device approval is on in your tailnet. Approve the device in the admin console, under **Machines**. Generate the key with **Pre-approved** checked and new nodes skip it.                       |
| **Error**                                                        | Read the sentence in the banner above the list. If the process keeps stopping, check the server's log, then select **Reconnect**.                                                                       |
| **Stopped**                                                      | Proxies are off, `tailscaled` is missing, or the proxy was just removed.                                                                                                                                |
| `Proxy "Home" is not connected: …` on a tool call                | The proxy is not **Connected**. The sentence says what it needs. Fix the status on **Proxies** and try again.                                                                                           |
| A call to the MCP times out or fails after the proxy is **Connected** | The policy of your tailnet does not let the tag reach that machine or port, the machine is off, or nothing listens on that port. Check the grants and try `tailscale ping` from another device.     |
| "Choose a proxy of this team"                                    | The proxy belongs to another team. Add one to this team.                                                                                                                                                |
| "Proxy is used by 2 MCPs: …" when deleting                       | Set **Network** to **Direct**, or another proxy, on the MCPs it names, then delete.                                                                                                                     |
| The form refuses the MCP URL                                     | A name without a dot is refused. Use the full name (`box.tailnet.ts.net`) or the `100.x.y.z` address.                                                                                                   |
| "OAuth client secrets are not supported yet"                     | You pasted a `tskey-client-…` secret. Create an auth key instead.                                                                                                                                      |

To disconnect completely, delete the proxy in MyMCPs, then remove its device and revoke its key in the Tailscale admin console.
