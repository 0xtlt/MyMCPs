# Backups

A backup is one file that holds everything an instance stores, encrypted with a password you choose. You export it from a running instance and import it on the setup screen of a new one. Use it to move MyMCPs to another server, or to keep a copy you can return to.

## What a backup holds

- Users, with their password hashes, and pending invites.
- MCPs, with their credentials: bearer tokens, header values, OAuth clients and connections, built-in MCP sign-ins and settings, and the environment of npm MCPs.
- Access tokens and the OAuth connections of AI clients. Agents keep working with the tokens they already have.
- Tool approval choices and requests, call logs, and the instance settings.

It does not hold the files MyMCPs can make again: the Deno cache and the sandboxes of npm MCPs, and files waiting in `builtin-uploads`. It does not hold your environment variables either: set `APP_URL`, `TRUST_PROXY` and the others on the new instance yourself.

## Export a backup

Only an administrator can export.

1. Open **Settings**. Under **Backup**, select **Export backup**.
2. Choose a **Backup password** and type it again. Use one you do not use elsewhere, and store it in a password manager: the file cannot be opened without it, and nobody can recover it.
3. Type your **Current password**, the one of your account. A backup holds every credential of the instance, so MyMCPs asks you to confirm that it is you.
4. Select **Export backup**. The browser downloads `mymcps-backup-YYYYMMDD-HHMMSS.mymcps`. The name carries the date and time of the export, in UTC, so several backups are told apart and sort in order. The same time is stored inside the file.

The backup is a snapshot of the moment you select the button. The instance keeps running while it is made.

## Import a backup

Importing is only offered on an instance nobody has set up yet. It replaces nothing you could lose.

1. Start the new instance and open it. It shows **Set up MyMCPs**.
2. Select **Import a backup**.
3. Choose the backup file and type its password.
4. Select **Import backup**. When it is done, you are on the sign-in page.
5. Sign in with an account of the instance you exported from, with the password it had then.

On the new instance, also check:

- **`APP_URL`**. If the address of the instance changed, the redirect URIs of OAuth clients you registered with providers (Strava, Google Ads, upstream OAuth MCPs) must change too, and AI clients must be given the new MCP URL.
- **npm MCPs** are downloaded again the first time they run.

To return an instance that is already set up to a backup, start from an empty data directory (a new volume), then import.

## What happens to secrets and sessions

Credentials are encrypted in the database with the `APP_KEY` of the instance. A backup carries the key it was made with, so the import can read them: when the new instance has another `APP_KEY`, every credential is encrypted again with the new one. You do not have to copy `APP_KEY` from the old instance.

Browser sessions are tied to `APP_KEY`. With another key, everyone signs in again. Access tokens of agents do not depend on it and keep working.

## Versions

- A backup from an older version imports into a newer one: its data is migrated first.
- A backup from a newer version is refused. Update the instance, then import again.
- The TypeScript server and the Rust server write and read the same file.

## Limits

- A backup file can be up to 4 GB. A reverse proxy in front of the instance must accept a request body of the size of your file on `POST /onboarding/import`.
- The file has 30 minutes to arrive.
- An address can try 10 imports per 15 minutes, and one import runs at a time.
- An export or an import keeps a copy of the database in `backup-tmp`, in the data directory, while it runs: the disk needs room for it. The copy is deleted when the work ends, and when the server starts.

## Keep the file safe

Anyone with the file and its password has every credential of the instance: the MCP secrets, the password hashes, and the `APP_KEY`. Treat it like the server's disk. The password is the only protection of a file that leaves the server, so make it long.

## File format

For tools that need to read or write a backup. All integers are big-endian.

**Header**, 36 bytes, not encrypted:

| Offset | Size | Field                                                   |
| ------ | ---- | ------------------------------------------------------- |
| 0      | 8    | The ASCII text `MYMCPSBK`                               |
| 8      | 1    | Format version: `1`                                     |
| 9      | 1    | Key derivation: `1` for scrypt                          |
| 10     | 1    | scrypt `log2(N)`: written as 17, accepted from 14 to 18 |
| 11     | 1    | scrypt `r`: 8                                           |
| 12     | 1    | scrypt `p`: 1                                           |
| 13     | 16   | Salt                                                    |
| 29     | 7    | Nonce prefix                                            |

**Key**: scrypt of the password (UTF-8, surrounding whitespace removed) with the salt and parameters of the header, 32 bytes.

**Body**: the content is cut into chunks of 65,536 bytes, the last one shorter and possibly empty. Each is encrypted with AES-256-GCM and written as its ciphertext followed by its 16-byte tag. The nonce of a chunk is the nonce prefix, the number of the chunk on 4 bytes counting from 0, and one byte that is `1` for the last chunk and `0` for the others. The 36 header bytes are the additional authenticated data of every chunk.

**Content**, once decrypted:

| Size        | Field                                                                                                                                                                       |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 4           | Length of the metadata                                                                                                                                                      |
| that length | Metadata: a JSON object with `createdAt` (the time of the export), `appKey` (the `APP_KEY` of the instance that made it), and `app` (which server made it, and its version) |
| the rest    | The SQLite database, as written by `VACUUM INTO`                                                                                                                            |
