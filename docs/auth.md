# Signing in

People sign in to the pages of MyMCPs with their email and password, with a passkey, or with their password followed by a second step. This page explains how passkeys, the authenticator app, and recovery codes work, and what an operator needs to know about them. Agents are not concerned: they reach `/mcp` with access tokens and OAuth, which do not change.

Each person sets this up for their own account in **Settings → Sign-in security**.

## Passkeys

A passkey signs in on its own, without the email or the password. Choose **Sign in with a passkey** on the sign-in page, and the browser or the phone asks for a fingerprint, a face, a PIN, or a security key.

A passkey is enough because MyMCPs requires the device to verify the person (the WebAuthn "user verification"): signing in with one proves something the person has (the device that holds the key) and something they are or know (the fingerprint or the PIN). It also cannot be phished: the browser only uses it on the address it was created for. So a passkey is a sign-in of its own, not a second step after the password. It also serves as the second step after a password, for whoever prefers to type the password first.

- **Add a passkey**: give it a name ("MacBook", "YubiKey") and type the current password. The browser then creates the passkey. An account can have as many as it needs, one per device or security key.
- **Rename** a passkey at any time. **Remove** one with the current password.
- The list shows when each passkey was added and last used.

Passkeys are tied to the host name of `APP_URL`, which must be set to the public HTTPS origin of the instance (`http://localhost` is accepted for development). Without a valid `APP_URL`, the Settings page says so and passkeys are not offered. Changing the host name of `APP_URL` later makes every registered passkey useless: the browsers will not offer them to the new address. Users then sign in with their password and second step, and add their passkeys again.

## Authenticator app

An authenticator app (1Password, Bitwarden, Google Authenticator, Aegis…) shows a new 6-digit code every 30 seconds.

1. Choose **Set up** next to **Authenticator app** and type the current password.
2. Scan the QR code with the app, or type the key it shows.
3. Type the code the app shows. The app is only turned on once a code proves it holds the key.

A code is accepted during its 30 seconds and the ones just before and after, for clocks that are a little off. Each code signs in once: the same code, or an older one, is refused afterwards, even within its 30 seconds. **Turn off** asks for the current password.

## Recovery codes

When the first passkey or the authenticator app is added, MyMCPs shows 10 recovery codes, once. Each one replaces the second step of one sign-in, for when the passkeys and the app are out of reach. Keep them in a password manager or on paper.

**New codes** (with the current password) replaces all of them and shows the new ones. Settings shows how many are left. The codes are deleted when the last passkey and the app are removed: there is nothing left for them to replace.

## How a sign-in goes

- **A passkey** signs in at once.
- **A password**, for an account without a passkey or an authenticator app, signs in at once, as before.
- **A password**, for an account with a passkey or the authenticator app, opens no session. The next page asks for a passkey, a code of the app, or a recovery code, and offers to switch between the ones the account has. The page waits 10 minutes; after that, or once the password changes or the account is signed out everywhere, the password must be typed again. No session cookie and no remember-me token exist until the second step is done.

Where a sign-in was going (an OAuth authorization, a tool call to approve) is kept through the second step.

Adding the first passkey or turning on the app signs out every other browser of the account, which had signed in with the password alone. The browser that turned it on stays signed in.

## Limits

- The password keeps its limits: 5 wrong passwords per account and 30 attempts per address (or IPv6 /64 network) per 15 minutes.
- A sign-in with a passkey counts against the same limit per address.
- The second step allows 5 attempts per account per 15 minutes, all methods and all browsers together, so that whoever has the password cannot guess a 6-digit code. When they are used up, the right code is refused too until the period ends. Confirming the setup of the app has its own 5 attempts.
- Every action of the section that needs the current password shares the limit of the password checks of Settings.

## When someone is locked out

- With a recovery code: sign in with the password, choose **Use a recovery code**, then generate new codes in Settings.
- Without one: an operator with access to the server removes the passkeys, the authenticator app, and the recovery codes of the account, and signs it out everywhere. The password alone then signs in again, and the person sets up their factors again.

  ```sh
  mymcps user:reset-2fa user@example.com
  ```

  `mymcps user:reset-password` changes the password only: it leaves passkeys and the app in place.

## Storage, backups, and keys

Three tables hold the data, each row tied to a user and deleted with them:

| Table | Holds |
| --- | --- |
| `user_passkeys` | The name, the credential ID, and the public key with its signature counter (what `webauthn-rs` stores as a `Passkey`, in JSON). Public data: it cannot sign anything. |
| `user_totp_secrets` | The key of the authenticator app, encrypted with `APP_KEY` like the other secrets, and the last 30-second step used, which is how a code is refused a second time. |
| `user_recovery_codes` | The SHA-256 hash of each code (80 random bits), and when it was used. |

Backups hold the three tables. Importing a backup on an instance with another `APP_KEY` encrypts the keys of the authenticator apps again with the new one, as it does every other secret. Passkeys keep working after an import as long as the new instance has the same `APP_URL` host name.

The challenge of a passkey request stays in the memory of the server, never in the browser, for 5 minutes. A restart forgets the requests under way: the person tries again. The 10-minute wait of a second step lives in the encrypted session cookie, and survives a restart.

## The Node.js app and the shared database

The Rust server and the Node.js app of the `main` branch read the same SQLite database. This feature adds one migration, which only creates the three tables. The Node.js app ignores them, so switching between the two keeps working, with these consequences:

- **The Node.js app does not ask for the second step.** On an instance that goes back to the Node.js app, a password alone signs in to an account protected by a passkey or an authenticator app, and passkeys are not offered. The data stays in place and is used again when the Rust server returns. Rolling back therefore removes this protection: tell your users, or do not roll back accounts that rely on it.
- The Node.js app refuses to import a backup made by a Rust server that has this migration: it reads it as a backup from a newer version. Backups of the Node.js app still import into the Rust server.
- `node ace migration:rollback` does not know this migration and stops at it. Drop the three tables and delete its row from `adonis_schema` first if you must roll the Node.js schema back.

## Libraries

- [`webauthn-rs`](https://crates.io/crates/webauthn-rs) (0.5) verifies passkeys. It is maintained by the Kanidm project, which uses it in production, follows the WebAuthn specification, and exposes a passkey API that makes the safe choices (user verification required, no attestation, signature counters checked). It verifies signatures with OpenSSL, which is compiled from source into the binary (`openssl` with its `vendored` feature): building needs `perl` and `make`, and the image installs them in its build stage. Version 0.6 is not used yet: it is a development release and pulls the `rsa` crate, which has an open advisory (RUSTSEC-2023-0071).
- [`totp-rs`](https://crates.io/crates/totp-rs) computes the codes of RFC 6238, and writes the `otpauth://` URL apps understand.
- [`qrcodegen`](https://crates.io/crates/qrcodegen) (Project Nayuki, no dependency) encodes the QR code, which the server draws as SVG: no image is fetched and no script runs.
