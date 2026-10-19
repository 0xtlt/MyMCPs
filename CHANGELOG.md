# Changelog

Notable project changes are recorded here in English. Sections are organized by UTC merge date, newest first.

## 2026-10-19

### Added

- Added `mise.toml`: `mise install` sets up the Rust, Node.js, and cargo-audit versions the releases are built and checked with, and `mise run dev`, `test`, `lint`, `audit`, `build`, and `ci` run the commands of the Quality and Security workflows.

### Changed

- The image runs npm MCPs with Node.js 24.22.0, up from 24.21.0, and the server is built with Rust 1.99.1, up from 1.98.1, in the image and in CI.
- Updated the Rust dependencies to their latest compatible releases, among them tokio 1.53.3, serde_json 1.0.154, aws-lc-rs 1.18.2, and uuid 1.28.0, and the GitHub Actions that install CI tools and run CodeQL.

### Fixed

- The table of **Access tokens** no longer spills out of its card. The actions of each row (**Edit**, **Remove MCP management**, **Revoke**, **Delete**) are in a **…** menu at the right edge of the row, as in the list shown on phones, and the table scrolls inside its card when the window is too narrow instead of shrinking the **Name** column to nothing. **Manages MCPs** is a line under the connection's name, no longer a second badge cut short in the **Type** column.

## 2026-10-18

### Added

- Added tools that let an agent manage the MCPs of a team through the gateway: `mcp_registry_list`, `mcp_registry_create`, `mcp_registry_update`, `mcp_registry_delete`, `mcp_registry_test`, and `mcp_registry_auth_link`. They accept what the form of the **MCPs** page accepts for HTTP MCPs, test an MCP as **Test connection** does, and answer the link to the MCP's dialog where a person selects **Connect** for an MCP that signs in with OAuth. They never answer a secret: tokens, header and environment values are only said to be set, and a URL is shown as its origin. Their arguments stay out of the call log, and one access token may call them 30 times a minute. See [docs/mcp-management-tools.md](docs/mcp-management-tools.md).
- Added the `mcps:manage` OAuth scope and a checkbox for it on the consent screen, **Allow this agent to manage your MCPs**, unchecked by default whatever the client asks for. Only an admin or an owner of the chosen team can tick it. The tools above are only listed and only run for a connection that has it, while the person who gave it is still an admin or an owner of the team; the gateway checks this at every request.
- Added **Remove MCP management** to the OAuth connections of **Access tokens** that have the permission, marked **Manages MCPs**: the connection keeps calling tools and can no longer manage MCPs.

### Security

- Updated rustls to 0.23.46 for RUSTSEC-2026-0346, RUSTSEC-2026-0347, RUSTSEC-2026-0348, and RUSTSEC-2026-0349, which affect the TLS connections the server makes to MCPs and OAuth providers.
- The tools that manage MCPs never add an npm MCP, nor change what one runs: its package, version, arguments or environment variables. Running a package is running code on the server, and these tools wait for no approval, so the agent is told to ask the user to do it on the **MCPs** page. An agent can still list, rename, enable, disable, test, and delete an npm MCP.
- The tools refuse an MCP address on a loopback, private or link-local network (such as `169.254.169.254`, the metadata service of the clouds), or a name that resolves to one: the answer of that address would come back to the agent. An MCP already on such an address, set on the **MCPs** page, can still be tested and renamed by the agent. A person adds one on the page.
- The tools show the URL of an MCP as its origin only, since the path of an URL can hold the key of an account, and the number of arguments of an npm MCP instead of its arguments.

### Changed

- The OAuth server publishes `mcp:tools` and `mcps:manage` in `scopes_supported` and accepts either in the `scope` of a registration or an authorization request; any other scope is still refused. A refresh keeps the scopes of its grant: it may name fewer, never more, and never changes them.

## 2026-10-12

### Added

- Added `NPM_SANDBOX`. npm MCPs run in a sandbox of the Linux kernel and do not start where it cannot be enforced; `NPM_SANDBOX=off` starts them without one, with every right of the server, to develop on macOS or Windows. The server says at its start what it found: `npm MCPs run in a sandbox`, or `npm MCPs cannot start` and the reason.
- Added `NODE_BINARY`, the path of the Node.js binary that runs npm MCPs when it is not the `node` of the `PATH`. It replaces `DENO_PATH`.
- Added teams, with the roles and rules of Coolify. MCPs, access tokens, approvals, call logs, and analytics belong to a team and are only shown to its members, and an access token reaches the MCPs of its own team only. Every account has a personal team, anyone can create more, and a team switcher at the top of the navigation goes from one to another. On upgrade, everything an instance had goes to the **Root Team**: its first administrator becomes the owner, the other administrators stay admins, members stay members, and each of them also gets a personal team.
- Added the **owner**, **admin**, and **member** roles of a team. A member reads the MCPs of the team and makes access tokens of their own. An admin also manages MCPs, every access token, members, and invites, and reads the logs and the analytics. An owner also deletes the team and decides who else owns it. Nobody changes or removes someone ranked above them, nor gives a role above their own.
- Added **super admins**, who run the instance: they change the instance settings, export backups, and manage accounts. It is said of the account, apart from any team: owning or administering a team, the Root Team included, gives no right over the instance. The first account is a super admin, and on upgrade every administrator an instance had becomes one.
- Added registration, off by default: a super admin turns **Allow registration** on under **Settings → My Instance**, and anyone can then create an account, with a team of its own, on `/signup`.
- Added a **Users** page for super admins: every account with its teams, a way to make another account a super admin or to take that from it, and a way to delete an account. Each asks for your own password, and the instance always keeps a super admin. The teams a deleted account was alone in are deleted with it; the others keep an owner.
- Added **sign-in protection** under **Settings**: an account can ask for more than its password. An **authenticator app** (1Password, Google Authenticator, Authy and the like) is set up by scanning a QR code, and its 6-digit code is then asked after the password. A **passkey** (a fingerprint, a face, a security key) answers that second step too, and also signs in alone from the sign-in page. Passkeys need `APP_URL` to be the address people open.
- Added **recovery codes**: eight codes that come with the first protection of an account, shown once, each of which signs in once in place of the app or the passkey. New ones can be made under **Settings**.
- Added two ways back in for someone who lost their app, their passkeys and their codes: a super admin removes the protection of the account under **Users**, and `mymcps user:reset-protection <email>` does it from the server.
- Added a role to invites, and invites for people who already have an account: they sign in and join the team with one button. An invite works whether or not registration is open. An admin sees and removes the invites up to their own rank: the invite of an owner, with its link, is for owners.
- Added a choice of team to the consent page of the OAuth server, for someone who is in several: the client reaches the MCPs of that team.
- Added OAuth with MCP providers that register no client and read a Client ID Metadata Document instead, such as ElevenLabs (`https://api.us.elevenlabs.io/v1/mcp`): **Connect** no longer answers `This OAuth provider does not support automatic client registration` there. The instance serves its document at `/oauth/client-metadata.json`, and the provider fetches it: `APP_URL` has to be an HTTPS address the provider can reach. A provider that registers clients is connected as before.
- Added an **ElevenLabs** template to the gallery of **Add MCP**.

### Changed

- Members no longer add, change, or delete MCPs, nor set their tool approvals: that is for the owners and admins of the team. A member only sees the access tokens they made themselves.
- An email address is one account whatever its letter case: signing in no longer depends on the case it is typed in, and nobody signs up with, or changes their address to, one that only differs by case from the address of another account.
- Instance settings, backups, and accounts are for super admins. An owner or admin of a team manages that team only.
- Removing a member takes them out of the team and revokes the access tokens they made there. Their account and their other teams stay; deleting the account is done from **Users**.
- The page of the team is at `/team`, where everyone in the team sees its members. `/invites` redirects there. The slug of an MCP is unique in its team rather than in the instance.
- A backup holds the sign-in protection of each account. The secret of an authenticator app is encrypted again for the instance that imports it; passkeys only keep answering when that instance has the same `APP_URL`.
- A backup must hold a super admin to be imported, where it had to hold an administrator. A backup made by an earlier version still imports: its administrators become super admins.
- The database migration that adds teams runs when the server starts and cannot be undone by installing an earlier version, which no longer reads the database afterwards. Export a backup under **Settings → Backup** before you upgrade.
- npm MCPs are run by Node.js instead of Deno. The package of an MCP is installed with `npm` the first time the MCP is used, in a directory of its own under `mcp-packages` in the data directory, then started with `node` for each request. Every package is downloaded again once after the upgrade, and the first request of an MCP waits for its install. **Update MCP** and the scheduled updates install the latest version beside the one in use, which the requests under way finish on. The MCP list and the edit dialog show the version that is installed.
- A package that is used by several MCPs is installed once for each: nothing is shared between two MCPs any more.
- An npm MCP names a package of the npm registry, at a version, a range of versions or a tag. A path, a URL, a git repository or an alias is refused by the form, and when an MCP saved earlier starts.
- The image contains Node.js 24 with its npm instead of Deno. Running from source needs Node.js with npm instead of Deno to run npm MCPs. `DENO_PATH` and `DENO_DIR` are no longer read and can be deleted from a deployment, and the server deletes the `deno-cache` directory of its data directory when it starts.
- The environment of an npm MCP no longer takes `NODE_COMPILE_CACHE`, `NODE_V8_COVERAGE`, `NODE_REPL_EXTERNAL_MODULE`, `OPENSSL_CONF` and `OPENSSL_MODULES`, which configure Node.js rather than the package. Names that start with `DENO_` are ordinary variables again.

### Security

- npm MCPs run in a sandbox the Linux kernel enforces, where the permissions of Deno were enforced by the runtime that ran the package. With Landlock, a package reads the files it was installed with and the system's, and reads and writes one directory of its own: the database, the key, `.env` and the files of other MCPs are refused. With seccomp, it starts no other process, sends a signal to none but itself, opens no Unix socket, changes the mode or the owner of no file by its path, and is refused tracing, mounts and namespaces. The sandbox needs no privilege and works in a container without capabilities; it needs Linux 5.13 or later with Landlock enabled on the host. The network of a package stays open, as it was. A server whose application or data directory lies in a directory of the system, such as `/usr`, which every sandboxed process reads, starts no npm MCP and says why.
- A process of an npm MCP holds at most 2 GiB of memory, 4,096 open files, and cannot push the server's user past 4,096 threads.
- `npm` installs a package inside the same sandbox, where it writes nothing but the directory it installs into, and runs none of the scripts of the package or of its dependencies. A package cannot write its own code.

### Removed

- Removed Deno from the image and from the requirements of a server run from source.

## 2026-10-11

### Added

- Added a dashboard as the home page: the gateway address with a shortcut to install it in a client, the MCPs, access tokens, and approvals that need attention, and for administrators the tool calls of the last 14 days with the most recent ones.
- Added search, filters by status, transport, and authentication, pagination, and an on/off switch on each row to the **MCPs** page, with the number of tools each MCP listed since the server started.
- Added **Active** and **Expired & revoked** views and pagination to **Access tokens**, and a choice of page size to **Logs**. A call opened in **Logs** links to its MCP and copies as JSON.
- Added a dialog that shows the link of an invite right after it is created, on the page now named **Team**.
- Added the `mymcps` command. `mymcps` alone serves the instance, and `mymcps user:reset-password`, `mymcps mcp:update`, `mymcps migration:run`, `mymcps generate:key`, and `mymcps healthcheck` replace the `node ace` commands.
- Added `DATA_DIR` to choose where the database, the generated key, MCP sandboxes, and uploads are kept (`tmp` by default, `/app/tmp` in the image), and `APP_ENV` as the name of the environment. `NODE_ENV` is still read.

### Changed

- MyMCPs is now a single Rust program instead of a Node.js application, with the same features. It reads and writes the same SQLite database, the same `APP_KEY`, encrypted credentials, access tokens, and OAuth connections, the same environment variables, and the same `/app/tmp` volume: upgrading is replacing the image or the binary, browsers stay signed in, and an instance can return to the previous version on the same data.
- The interface follows the new design and is rendered by the server: pages load one stylesheet and one small script instead of a React application. **Tool approvals**, **Update MCP**, and **Re-authorize** moved from the edit dialog of an MCP to the **⋯** menu of its row.
- The image contains the `mymcps` binary and Deno, and no longer Node.js. The server creates its key and applies database migrations itself when it starts, where the entrypoint script did, and the container health check is `mymcps healthcheck`.
- A server started without `APP_ENV` or `NODE_ENV` runs as production.
- Building from source needs Rust and a C compiler instead of Node.js and pnpm: `cargo run --bin mymcps`, `cargo test --workspace`, `cargo build --release --bin mymcps`. The release workflows build, test, and audit with Cargo.
- The iCloud Mail MCP converts to text the HTML messages it used to give up on, such as deeply nested or unclosed tags. The limits on time and memory of a conversion are unchanged.
- The schedule of the npm auto-update is read by MyMCPs itself, with the same five fields. A date written out in place of a schedule is refused, and a schedule naming a day that February lacks, such as `0 3 1,31 * *`, also runs on 1 March.
- The call log writes the records that wait together in one transaction, so a burst of tool calls no longer fills its queue of 1,000 records and loses the rest.
- `/mcp` reads a message from the body of the request only. Parameters in the query string of a `POST` are ignored, where they used to replace the members of the message.
- **Tool approvals** lists a tool whose name is blank or longer than 254 characters without a choice: it always asks, and the page saves the choices of the other tools.
- The consent page of the OAuth server warns that a client chose its own name and shows where the authorization code is sent, also when the callback is not a local address.
- A download link of a built-in MCP has 10 minutes to deliver its file.

### Fixed

- The server starts when the schedule of the npm auto-update cannot be read, and says so in its log.

### Security

- Pages are sent with `Cache-Control: no-store`, so a page with the data of an account, or a new access token, is not shown again from the cache of the browser after sign-out.
- The Content-Security-Policy no longer allows inline styles (`style-src 'self'`).
- A JSON document with many numeric keys, in the answer of a remote MCP or in a request to `/mcp`, no longer keeps a processor busy for minutes.
- An email whose date is the last or the first minute a date can hold no longer makes the iCloud Mail MCP fail to list the mailbox it is in.
- The package of an npm MCP can no longer write the files Deno reads in its sandbox before it starts (`.npmrc`, `deno.json`, `deno.jsonc`, `package.json`), with which it chose the registry its next start and its next update were fetched from. Files of these names are deleted from a sandbox at each start.
- `SSLKEYLOGFILE` can no longer be set on an npm MCP: Deno appended TLS keys to the file it named, anywhere the server can write.
- The OAuth endpoints a remote MCP names are connected to at public addresses only, also when their name resolves to another address between the check and the connection.
- The import of a backup keeps at most 1 MB of its form in memory before the file starts, where a form without a file part was kept whole. It refuses a database with generated columns, expression defaults, or indexes on expressions or on some rows, evaluates no `CHECK` constraint of the file, stops after 20 minutes, and refuses settings or administrators the server could not read afterwards.
- A stored password hash that asks for more than 16 times the work of the hashes MyMCPs writes, or more memory than Node allowed, is never computed.
- Signing in or out gives the session a new CSRF secret.
- The rate limits of the OAuth server count the addresses of one IPv6 /64 network together, as sign-in does.
- `/mcps/oauth/callback` shows the error of a provider only when it answers an authorization the session started.

### Removed

- Removed the Node.js application and its toolchain: `package.json`, pnpm, the `node ace` commands, the Docker entrypoint script, and the browser test suite. `SESSION_DRIVER`, `APP_NAME`, and `VITE_APP_NAME` are no longer read and can be deleted from a deployment.

## 2026-10-08

### Added

- Added encrypted backups. Under **Settings → Backup**, an administrator chooses a password, confirms with the password of their account, and downloads one `.mymcps` file, named after the date and time of the export, that holds everything the instance stores: users, MCPs with their credentials, access tokens, call logs and settings. The setup screen of a new instance offers **Import a backup** beside creating the admin account: choose the file, enter its password, then sign in with an account of the imported instance. The file carries the `APP_KEY` its credentials were encrypted with, so it imports into an instance that has another key, and a backup made by an older version is brought to the current schema on the way in. Files on disk are not part of it: npm sandboxes and the Deno cache are downloaded again, and uploads waiting for a built-in MCP are short-lived. A backup takes up to 4 GB, and a reverse proxy in front of the instance must accept a request body of the size of the file to import; [docs/backup.md](docs/backup.md) has the details.

### Security

- A backup is encrypted with AES-256-GCM, in chunks of 64 KiB, under a key derived from its password with scrypt. A file that was changed, cut short, reordered or continued does not open, and neither does one whose header asks for more than 256 MiB of memory to derive its key.
- Exporting a backup asks for the current password of the account again. It shares the allowance of the email and password changes, 5 attempts per 15 minutes, and each export is recorded in the server log.
- The import form only exists while the instance has no account. Its file is read once the CSRF token, the absence of any account and the rate limit, 10 imports per 15 minutes for each client address, have been checked. It is written to a private directory under `tmp/backup-tmp`, never held in memory, and refused past 4 GB; one import runs at a time, and the directory is deleted whatever the outcome, and when the server starts.
- An imported database is checked before it is trusted. It is opened read-only with an untrusted schema and must pass SQLite's integrity check, be made of plain tables and indexes (no trigger, no view, no virtual table), come from a version this one knows, and have an administrator. It must then have the tables and columns of the instance and no other, and its rows are copied table by table and column by column, in one transaction that fails on any row that refers to a missing one. A refused backup leaves the instance as it was. The counters of the rate limiter are never imported.
- A form posted without a valid CSRF token no longer has its fields copied into the session. The export form is the one form the browser posts itself, and it is made of passwords; a refused export likewise sends back its errors without what was typed.

## 2026-10-07

### Added

- Added tool approvals for every MCP, built-in or connected. Under **Tool approvals** in the menu of an MCP, each tool of that MCP either **Runs** or **Asks**. A call to a tool that asks is not run: the agent gets a link to an approval page, and the call runs once an administrator, or the member who created the access token, approved it and the agent makes it again with the same arguments. The page is written by MyMCPs from the call itself, never by the agent: a built-in tool says what would change beside the current value, and a tool of a connected MCP has its exact arguments listed beside the description its MCP gives. An approval covers one call, is used once, and expires after 24 hours. Open requests are listed on the new **Approvals** page, with a count in the navigation, and agents see which tools ask in their descriptions. Approval links require `APP_URL`; [docs/tool-approvals.md](docs/tool-approvals.md) has the details.
- Added a built-in Google Ads MCP on version 25 of the Google Ads API, signed in through an OAuth client from your own Google Cloud project. It has 13 read tools for accounts, campaigns, ad groups, ads, keywords, search terms, performance by day, device, or network, assets, the change history, locations, keyword ideas, and read-only GAQL queries. **Allow write access** adds 15 tools to create and change Search and Display campaigns, budgets, bidding, targeting, ad groups, keywords, responsive search ads, responsive display ads, and image assets. `create_campaign`, `update_campaign`, `update_campaign_budget`, and `set_campaign_status` ask for approval by default, and their approval page shows the campaign, the current and new amounts, and a warning when a budget is multiplied. Campaigns are created paused, and the MCP can be limited to the accounts you list. Google retired developer tokens in September 2026, so the setup asks for none; [docs/google-ads.md](docs/google-ads.md) has the full guide.
- Added image uploads to the Google Ads MCP. `create_image_upload_link` returns a temporary signed link that takes one JPEG, PNG, or GIF file of up to 5 MB as the body of a `PUT` request, and `create_image_asset` adds the uploaded file to an account after checking its shape and size. A reverse proxy in front of the instance must accept 5 MB request bodies.
- Added file attachments to the iCloud Mail MCP. `create_upload_link` returns a temporary signed link that takes one file of up to 20 MB as the body of a `PUT` request, for example with `curl -T`, and `send_message` and `create_draft` attach up to 10 uploaded files and 20 MB of them with `attachments`. The tool comes with **Save drafts** or **Send mail**. An uploaded file waits in `tmp/builtin-uploads` and can be attached for an hour, then it is deleted, as it is with its MCP. Upload links require `APP_URL`, and a reverse proxy in front of the instance must accept 20 MB request bodies; [docs/icloud-mail.md](docs/icloud-mail.md#sending-files) has the details.

### Changed

- Built-in MCPs can ask for settings beyond their sign-in, such as the manager account Google Ads acts through. They are encrypted like other MCP credentials.
- The dialog of a built-in MCP reports every wrong field at once instead of one group at a time.
- Signing in from an approval link returns to that approval request.
- The call log files a held call under the new categories `approval required` and `approval denied`.

### Fixed

- Refreshing an OAuth connection twice at the same moment no longer answers the second request with a server error. That request is now refused with `invalid_grant` and the connection is revoked, as it is whenever a refresh token that was already rotated comes back. A refresh that arrives while its connection is being revoked is refused too, where it used to receive tokens that did not work.

### Security

- An authorization code exchanged by two requests at the same moment now yields a single OAuth connection: the second request is refused with `invalid_grant`, where both used to receive tokens.
- Upload links check their signature before anything is counted or read, cannot be used to download, take one file each, and stop working when the MCP is disabled or allows neither **Save drafts** nor **Send mail**. Each MCP holds at most 50 uploaded files and 100 MB, takes 60 uploads per 15 minutes from each client address and 3 at a time, and writes 2 messages with attachments at a time. An attachment is always bytes sent to an upload link: no tool can name a path on the instance or a URL to attach.
- An approval is bound to the access token, MCP, tool, and exact arguments of the call it was asked for: other arguments ask again, and another access token cannot use it. Of two identical calls made at once, one gets the approval. The arguments and the summary of a request are encrypted at rest. A request is read and decided by an administrator or by the member who created its access token, through a CSRF-protected form, and the link alone grants nothing. An access token can have 20 requests waiting, also when its calls arrive at once, and the same call made twice at the same moment asks once. If the saved choices of an MCP cannot be read, every tool of that MCP asks.
- A built-in tool that asks checks its arguments, the write access of its MCP, and its sign-in before anyone is asked to approve it, and the Google Ads tools have Google validate the change without making it.
- An upload link of a built-in MCP that signs in with OAuth stops taking files when write access is turned off.

## 2026-10-05

### Changed

- Built-in MCP tools now check all their arguments before anything else: `list_routes` refuses wrong page arguments without calling Strava, and `update_activity` reports a missing `activity_id` before asking for a field to change.

### Fixed

- Report a Strava error response with an unexpected shape as a plain HTTP error instead of failing the tool call.

## 2026-10-04

### Added

- Added built-in MCPs: MCP servers that MyMCPs runs itself for services whose own MCP is closed to self-hosted gateways. They authorize through an API application the admin registers with the service, and their Client Secret and tokens are encrypted like other MCP credentials.
- Added a built-in Strava MCP with 17 read tools for the athlete profile, training totals, zones, activities, activity streams, segments, routes, clubs, and gear. It is read-only by default; **Allow write access** adds 4 tools to create manual activities, edit activity details, update the athlete's weight, and star segments. The **Add MCP** dialog walks through creating the Strava API application, shows the Website and Authorization Callback Domain to enter, and offers a ready-made application icon to upload; [docs/strava.md](docs/strava.md) has the full guide.
- Added a built-in iCloud Mail MCP that reaches the mailbox over IMAP and SMTP with an app-specific password, which is encrypted like other MCP credentials. It has 8 tools to list mailboxes, list, search, and read messages, link to their attachments, save drafts, send plain text mail and replies, mark messages, and move them between mailboxes. The **Add MCP** dialog walks through creating the password at Apple and checks it by signing in when the MCP is saved; [docs/icloud-mail.md](docs/icloud-mail.md) has the full guide. The instance needs outbound access to `imap.mail.me.com:993` and `smtp.mail.me.com:587`.
- Added permissions to built-in MCPs whose provider cannot restrict the sign-in. Apple does not scope an app-specific password, so the admin chooses what agents may do when adding iCloud Mail: **Read mail**, **Save drafts**, **Send mail**, and **Organize mail**, with only reading allowed by default. MyMCPs enforces them for every access token: tools outside the allowed permissions are not listed and are refused when called.
- Added temporary attachment links to the iCloud Mail MCP. `get_message` lists the attachments of a message and `get_attachment_link` returns a signed link to download one from the instance, valid for 15 minutes by default and 60 at most. The link needs no sign-in, cannot be altered, only downloads, and stops working when the MCP is disabled or loses **Read mail**. It requires `APP_URL`, and downloads are limited to 60 per 15 minutes for each client address.
- Added sender addresses to the iCloud Mail MCP. The admin can list the account's aliases, custom domain addresses, and Hide My Email addresses, and agents can then send or draft from them with `from`. Replies are sent from the address the original message was written to.

### Changed

- Updated application and development dependencies and refreshed the lockfile, including AdonisJS core 7.5.2, the MCP SDK 1.32.0, Inertia 3.8.0, Vite 8.3.2, and the Astryx design system 0.6.5.
- Changed the proxy trust default in Docker Compose and Coolify from `TRUST_PROXY=true` to `loopback,uniquelocal`, so a client can no longer choose the address that rate limits and call logs record. `TRUST_PROXY` now accepts a comma-separated list of proxy IPs, CIDR ranges, and the names `loopback`, `linklocal`, and `uniquelocal`. A deployment that already stores `true` keeps it until you edit it; behind a CDN, add the CDN's address ranges.
- All npm MCPs now share one Deno cache that packages can read but not write, so **Update MCP** and scheduled updates change the version that actually runs. Each package is downloaded again once after the upgrade. An MCP's sandbox directory is deleted with the MCP and when its package changes, and a failed start reports Deno's own output.
- The gateway contacts upstream MCPs only to list tools and to call the one in use, instead of connecting to every allowed MCP on each request.
- The server always runs in UTC and ignores `TZ`, so access tokens and authorization codes expire when intended whatever the host's time zone.
- The Logs filter lists the MCPs that exist instead of every name found in the log.
- iCloud Mail `list_messages` and `get_message` cut very long subjects, address lists, and attachment lists, and flag what was cut with `<field>_truncated`.
- Pull requests and pushes to `main` now run the unit, functional, browser, and release-automation tests in the **Quality** workflow, and CodeQL also analyzes the workflows.
- The Docker image pins its Node and Deno base images by digest and Dependabot proposes their updates. The `NODE_VERSION` and `DENO_VERSION` build arguments are gone.
- The dependency audit ignores the `braces` advisory GHSA-vfj7-8cjw-p6xm, which has no fixed release and is only loaded by build tooling.

### Fixed

- Stopped the MCP dialog header from sliding out of view when a validation error was focused in a form taller than the dialog.
- Report a malformed OAuth callback as "Invalid OAuth callback" instead of returning to the app without a message.
- Stopped refreshing an upstream OAuth token on every connection when the provider issues it without `expires_in`.
- Download an attachment whose filename is not well-formed Unicode instead of returning an error.
- Kept `state` intact in gateway OAuth redirects, which had the request's query string appended to them.
- Removed expired rate-limit counters from the database instead of letting them accumulate.
- Kept pending OAuth connections small enough for the session cookie, so starting several of them no longer loses what the session was saving.

### Security

- Updated the MCP SDK to a release that limits request body size and JSON-RPC batch length in its HTTP server transport and follows client redirects only within the endpoint's origin.
- Updated Hono to 4.13.13 and its Node.js adapter to 2.1.3, which fix a `serveStatic` path-decoding bypass of middleware on static paths, and raised the pinned Hono floor to 4.13.11. MyMCPs does not serve files through `serveStatic`.
- Browser sessions are now checked on the server. They end after 2 hours idle or 24 hours, and signing out, changing the password, or running `user:reset-password` revokes the account's sessions, including a copied session cookie. Sessions from before the upgrade are refused once: browsers with a remember-me cookie get a new session automatically, the others sign in again. The upgrade adds a `session_version` column to users.
- Sign-in attempts are counted before the password is checked and limited to 5 per 15 minutes for each account and client address, and 30 failed attempts per 15 minutes for each client address. A successful sign-in to another account no longer resets the count, and IPv6 clients are counted by /64. Current-password confirmations in settings are limited to 5 per 15 minutes.
- Multipart request bodies are ignored. An anonymous upload could previously leave up to 20 MB on disk per request.
- The gateway authorization endpoint records a consent decision only from the CSRF-protected form. A `HEAD` request could previously issue an authorization code without the consent screen. Rejected authorization requests are shown on the instance instead of being redirected, unless the redirect URI is on the user's own device.
- Limited each access token to 600 gateway requests per minute, and dynamically registered OAuth clients to 1000, removing clients unused for 90 days.
- A large error response from an upstream MCP can no longer stall the instance while it is redacted. Upstream responses are limited to 32 MiB, error responses to 64 KiB, and a request stays cancellable after a redirect.
- Refused npm MCP environment variables that configure the Deno sandbox itself, such as `PATH` and loader or Deno runtime variables, which let a member escape the sandbox. Saved ones are ignored when the MCP starts, and Deno is always started from an absolute path.
- A saved bearer token, header value, or environment value is no longer carried over when an MCP is pointed at another origin, transport, or npm package; enter it again.
- Starting OAuth for an MCP with a different provider or a newly registered client drops the saved tokens first, and the start link refuses requests coming from another site. An MCP on a public address can no longer send the instance to OAuth endpoints on loopback, private, or link-local addresses, and its OAuth resource indicator must match the MCP URL.
- iCloud Mail: a crafted message can no longer stall or crash the instance when an agent reads it. HTML messages are converted in a separate short-lived process limited to 5 seconds; when one cannot be converted, `get_message` returns the headers and attachments with a `warning`. A reply is refused when the message being answered would address it to more than 50 recipients.
- Attachment links check their signature before a download is counted, are limited per MCP and client address, and serve 3 downloads at a time per MCP.
- Full page loads no longer go blank when stored text, such as a logged tool name or an MCP description, contains markup, and page data kept in the browser history is encrypted and unreadable after sign-out.
- The production server refuses to start with either `APP_KEY` published in this repository, and the container creates its files in `/app/tmp` with `umask 077`. Run `chmod -R go-rwx /app/tmp` in the container to tighten an existing volume.
- Server errors returned as JSON no longer include internal messages, failed queries no longer quote their values, and call-log slugs and caller addresses are bounded. The gateway and OAuth endpoints answer cross-origin requests without credentials.
- Release workflows install and run dependency code with a read-only token; a separate job that installs nothing tags and publishes the validated commit.

## 2026-10-02

### Changed

- Detect the Figma remote MCP (`https://mcp.figma.com/mcp`) when connecting OAuth. Figma only registers first-party client names with a localhost redirect, so MyMCPs registers as `Codex`, opens Figma in a new tab, and asks the admin to paste the localhost address that tab ends on to finish connecting.
- Register the Strava remote MCP (`https://mcp.strava.com/mcp`) as `Claude Code`. Upstream HTTP calls to Figma and Strava also use those clients' User-Agent and MCP initialize identity. Other MCP hosts still identify as MyMCPs. Strava uses the normal MyMCPs OAuth callback; only Figma still requires the pasted localhost callback.

## 2026-09-24

### Changed

- Released version [0.4.1](https://github.com/0xtlt/MyMCPs/releases/tag/v0.4.1).

### Fixed

- Share one upstream OAuth refresh across concurrent requests to the same MCP and reload the saved credentials in every caller. Parallel tool calls and health checks no longer reuse a rotating refresh token and accidentally revoke the connection.

## 2026-09-22

### Added

- Added `node ace user:reset-password <email>` for server-side account recovery, with hidden password confirmation, existing password validation, and atomic password hashing and remember-me token revocation. Documented local and Docker Compose usage.

### Changed

- Released version [0.4.0](https://github.com/0xtlt/MyMCPs/releases/tag/v0.4.0).

- Updated application and development dependencies, refreshed the lockfile, and upgraded pnpm to 12.5.1, bundled Deno to 2.9.7, and pinned GitHub Actions to current releases. TypeScript remains on 6.0.3 because the Adonis assembler and TypeScript ESLint do not support TypeScript 7 yet.
- Removed pnpm's obsolete module-purge setting for pnpm 12 compatibility and ran the Astryx 0.6 migration checks.

### Fixed

- Preserved accessible names for MCP and access-token edit dialogs after the design-system upgrade.

## 2026-08-18

### Removed

- Dropped Figma, Vercel, Canva, and Slack from the MCP template gallery because they only accept first-party clients such as Claude or Codex, not a custom MyMCPs OAuth configuration.

### Changed

- Upgraded the Astryx design system to 0.4.3, including info-banner painting under the neutral theme, tokenized focus rings, and NumberInput/Selector behavior.

## 2026-08-15

### Changed

- Released version [0.3.0](https://github.com/0xtlt/MyMCPs/releases/tag/v0.3.0).

## 2026-08-14

### Changed

- Upgraded the Inertia adapter to version 5 with Inertia v3, `@adonisjs/vite` 6, and Vite 8.
- Delivered success and error toasts through Inertia's flash bag instead of shared page props.

## 2026-08-13

### Added

- Added an Update MCP action for Deno npm MCPs that already track `latest`, which reloads the Deno package cache and retests the connection without changing pinned versions.
- Added instance settings to enable scheduled auto-updates of latest-tracking Deno npm MCPs, with a 5-field UTC cron expression that defaults to every day at 02:00.
- Added the Deno-cached npm package version in small type on the MCP list and edit form.

### Changed

- Released version [0.2.0](https://github.com/0xtlt/MyMCPs/releases/tag/v0.2.0).

- Hid the MCP edit form behind a centered spinner while Update MCP is running.
- Moved the Deno cached-version hint below the Version field so Extra args stays aligned.

### Fixed

- Rendered flash toasts into the open modal so they stay readable above the dialog backdrop.
- Reloaded Deno npm MCP caches with `--node-modules-dir=none` so Update MCP works inside this Node app, where Deno otherwise treats `package.json` as manual `node_modules` and refuses specifiers such as `@shopify/dev-mcp`.
- Allowed Deno npm MCP sandboxes to read the Deno package cache, so Node packages can load their own packaged files after a cache reload.

## 2026-08-12

### Added

- Added confirmed cleanup for selected or all expired and revoked access tokens, with a compact grouped mobile layout, neutral triggers, and a destructive final confirmation while preserving active tokens and historical activity labels.

## 2026-08-12

### Added

- Added confirmed cleanup for selected or all expired and revoked access tokens, with a compact grouped mobile layout, neutral triggers, and a destructive final confirmation while preserving active tokens and historical activity labels.

## 2026-08-11

### Fixed

- Fixed OAuth discovery for MCP servers that publish a verified same-origin, path-based issuer while relying on legacy root metadata discovery.

## 2026-08-09

### Changed

- Released version [0.1.2](https://github.com/0xtlt/MyMCPs/releases/tag/v0.1.2).

### Added

- Added public `linux/amd64` and `linux/arm64` GHCR images for every successful stable and nightly release, with immutable release tags, channel tags, OCI metadata, pre-publish health validation, and cached builds.

## 2026-08-08

### Added

- Added a searchable, category-filtered gallery with 20 official MCP setups, branded cards, bottom-left-aligned actions, and prefilled HTTP or npm configuration.

- Added nightly prereleases that run only for new commits and a manually triggered stable release workflow that validates the application, increments its semantic version across package metadata and MCP handshakes, updates the changelog, tags the commit, and publishes generated GitHub release notes without an AI service.

### Changed

- Released version [0.1.1](https://github.com/0xtlt/MyMCPs/releases/tag/v0.1.1).

- Extended the typecheck command to validate repository `.mjs` release scripts with TypeScript's strict JavaScript checking.

### Fixed

- Fixed nightly and stable release validation failing when the settings browser test matched both a visible notification and its accessibility live region.

### Security

- Pinned the transitive Nano ID dependency to a patched release that prevents zero-length custom generators from looping indefinitely.

## 2026-08-07

### Added

- Added OAuth login for MCP clients, including discovery, dynamic client registration, PKCE authorization, consent, refresh-token rotation, connection revocation, and OAuth-managed access tokens ([#53](https://github.com/0xtlt/MyMCPs/pull/53)).
- Added an admin setting for the instance-wide default MCP tool discovery mode, with per-request header overrides.

### Changed

- Set the application version to 0.1.0 for the first tagged release.
- Made OAuth the recommended MCP installation method while retaining manual bearer-token configurations ([#53](https://github.com/0xtlt/MyMCPs/pull/53)).

### Fixed

- Allowed Deno npm MCP sandboxes to call `os.homedir()`, so Node-oriented packages such as `@shopify/dev-mcp` can start under the existing filesystem jail.
- Restored Cursor OAuth connections by accepting its exact native-app callback while continuing to reject unapproved custom URI schemes ([#56](https://github.com/0xtlt/MyMCPs/pull/56)).
- Standardized user-visible dates to day-first format while preserving local-time display and stored ISO values ([#54](https://github.com/0xtlt/MyMCPs/pull/54)).

### Security

- Hardened credential handling across MCP and OAuth redirects, callbacks, CORS, diagnostics, and logs; added safe outbound fetch behavior, login rate limiting, nonce-based CSP, persistent remember-me token revocation after password changes, and call-capture size limits ([#51](https://github.com/0xtlt/MyMCPs/pull/51)).
- Enforced HTTPS outside loopback development, constrained OAuth gateway boundaries, and added refresh-token reuse detection and grant-family revocation ([#53](https://github.com/0xtlt/MyMCPs/pull/53)).

## 2026-08-06

### Added

- Added custom, shareable analytics time ranges with exact timezone-aware intervals, start and end times, and a 365-day limit ([#50](https://github.com/0xtlt/MyMCPs/pull/50)).

### Fixed

- Hardened custom-range validation and fallbacks, preserved exact intervals across timezones and daylight-saving transitions, and restored spacing below log navigation ([#50](https://github.com/0xtlt/MyMCPs/pull/50)).

## 2026-08-05

### Changed

- Made the authenticated application responsive across mobile navigation, dashboard metrics, analytics, MCPs, access tokens, invitations, logs, settings, forms, and dialogs while preserving desktop tables ([#49](https://github.com/0xtlt/MyMCPs/pull/49)).

### Fixed

- Stacked email actions correctly on narrow screens and prevented horizontal overflow at supported mobile widths ([#49](https://github.com/0xtlt/MyMCPs/pull/49)).
