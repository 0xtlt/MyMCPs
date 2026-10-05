# Built-in iCloud Mail MCP

Apple has no mail API and no OAuth for third-party apps. MyMCPs therefore ships an iCloud Mail MCP of its own. It runs inside your instance and reaches your mailbox the way a mail app does: over IMAP and SMTP, signed in with an [app-specific password](https://support.apple.com/102654) that you create and can revoke at any time.

Setup takes about three minutes: create the password on Apple's site, paste it into MyMCPs with your iCloud Mail address, and choose what agents may do.

## Before you start

- **An iCloud Mail address**, ending in `@icloud.com`, `@me.com`, or `@mac.com`. If you sign in to Apple with another address, such as a Gmail one, you still need the iCloud Mail address for this.
- **Two-factor authentication** on your Apple Account. Apple only offers app-specific passwords with it.
- **Outbound access** from your instance to `imap.mail.me.com` on port 993 and `smtp.mail.me.com` on port 587. `APP_URL` is only needed for [attachment links](#attachment-links): nothing redirects back to MyMCPs during setup.

## 1. Create an app-specific password

1. Sign in at [account.apple.com](https://account.apple.com/account/manage).
2. In **Sign-In and Security**, select **App-Specific Passwords**.
3. Generate a password and name it, for example `MyMCPs`.
4. Copy it. Apple shows it once, as four groups of letters like `abcd-efgh-ijkl-mnop`.

Two things to know about this password:

- **Apple cannot limit it.** It is not restricted to mail, and it cannot be made read-only. Apple accepts it for the iCloud services open to third-party apps, which are Mail, Calendar, and Contacts. MyMCPs only ever sends it to the two mail servers above, and restricts what agents can do with [permissions](#permissions) of its own.
- **Changing your Apple Account password revokes it**, along with every other app-specific password. Create a new one and save it in MyMCPs afterwards.

## 2. Add the MCP in MyMCPs

1. Open **MCPs**, select **Add MCP**, and choose **iCloud Mail** (under **Popular** or **Productivity**).
2. Enter your **iCloud Mail address** and paste the **app-specific password**.
3. Optionally list your **Other sender addresses**, separated by commas. See [Sender addresses](#sender-addresses).
4. Under **Choose what agents can do**, check the [permissions](#permissions) to allow. Only **Read mail** is checked to begin with.
5. Select **Add MCP**.

MyMCPs signs in once to check the password, and the MCP status becomes **ready**. There is no account to connect afterwards.

The password is encrypted with the instance's `APP_KEY` and is never sent back to the browser. MyMCPs refuses a value that does not look like an app-specific password, so your Apple Account password cannot be saved by mistake.

## Permissions

Strava and other OAuth services let you pick permissions on their own consent screen. Apple offers nothing of the kind for an app-specific password, so the permissions are chosen in MyMCPs when you add the MCP, and MyMCPs enforces them for every agent that uses it.

| Permission        | Tools                                                                   | What it allows                                                                    |
| ----------------- | ----------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| **Read mail**     | `list_mailboxes`, `list_messages`, `get_message`, `get_attachment_link` | List mailboxes, search, read messages, and download their attachments             |
| **Save drafts**   | `create_draft`                                                          | Write messages to your Drafts mailbox, for you to review and send yourself        |
| **Send mail**     | `send_message`                                                          | Send messages from your addresses                                                 |
| **Organize mail** | `mark_messages`, `move_messages`                                        | Mark messages as read or flagged, and move them between mailboxes, Trash included |

- A tool outside the allowed permissions is not listed to agents, and is refused if an agent calls it anyway.
- At least one permission is required. Any combination works, including sending without reading.
- Changes apply as soon as you save the MCP. There is nothing to re-authorize.
- Answering a message with `reply_to_uid` reads the message being answered, so it needs **Read mail** in addition to **Save drafts** or **Send mail**.

**Different permissions for different agents.** Permissions belong to the MCP, not to an access token. To give agents different rights, add iCloud Mail more than once, for example `iCloud Mail` with **Read mail** only and `iCloud Mail assistant` with **Read mail** and **Save drafts**. Each one has its own slug, and each access token can be limited to the MCPs it should use.

**Think twice before allowing Send mail together with Read mail.** Anyone can write to your inbox, and a message can be worded to manipulate the agent that reads it, for example into forwarding other mail to its sender. **Read mail** with **Save drafts** keeps you in the loop: the agent prepares the answer, and you send it from Mail.

## Tools

Through the gateway, tool names are prefixed with the MCP's slug, such as `icloud-mail__list_messages`.

| Tool                  | Permission    | Does                                                                                                            |
| --------------------- | ------------- | --------------------------------------------------------------------------------------------------------------- |
| `list_mailboxes`      | Read mail     | Lists mailboxes with their message and unread counts, and their role: inbox, sent, drafts, trash, junk, archive |
| `list_messages`       | Read mail     | Lists or searches a mailbox, newest first: sender, recipients, subject, date, flags, attachment count           |
| `get_message`         | Read mail     | Returns one message: headers, text, and the name, type, size, and part of each attachment                       |
| `get_attachment_link` | Read mail     | Returns a temporary link to download one attachment                                                             |
| `create_draft`        | Save drafts   | Saves a plain text message to Drafts without sending it                                                         |
| `send_message`        | Send mail     | Sends a plain text message and keeps a copy in Sent                                                             |
| `mark_messages`       | Organize mail | Marks up to 100 messages as read or unread, flagged or not                                                      |
| `move_messages`       | Organize mail | Moves up to 100 messages to another mailbox                                                                     |

How they behave:

- **Mailboxes** are named by their path, as `list_mailboxes` returns it. iCloud calls the sent and trash mailboxes `Sent Messages` and `Deleted Messages`. The tools default to `INBOX`.
- **Messages** are identified by a UID that belongs to one mailbox. A moved message gets a new UID in its destination, which `move_messages` returns.
- **Searching** combines `from`, `to`, `subject`, `text`, `since`, `before`, `unread`, and `flagged`. Apple's server runs the search. Dates are compared by day of receipt.
- **Reading does not mark a message as read.** Use `mark_messages` for that.
- **Message text** is the plain text part when there is one, and the HTML converted to text otherwise, with links kept and images dropped. Only that part is downloaded. `max_chars` (20,000 by default, 100,000 at most) cuts long messages and sets `text_truncated`.
- **HTML is converted outside the server.** A message can be written to keep a converter busy for a long time or to make it run out of memory, so each HTML-only message is converted by a short-lived process of its own, which gets the first megabyte of the HTML, 5 seconds, and a 128 MB heap. When that is not enough, `get_message` still returns the headers and attachments, with an empty `text` and a `warning`. Conversions run one at a time.
- **Long headers are cut.** Anyone can write to your inbox, so `list_messages` and `get_message` return at most 50 addresses for each of the sender, recipient, copy, and Reply-To fields, 320 characters of each address, 998 characters of the subject and of the message ID, and 255 characters of an attachment's name and type. `get_message` names the first 100 attachments. Whatever was cut is flagged, for example with `to_truncated` or `attachments_truncated`.
- **Replying.** `reply_to_uid` on `send_message` or `create_draft` addresses the message to the sender of the original, or to its Reply-To address, prefixes the subject with `Re:`, and sets the headers that keep both in one conversation. `reply_all` also copies the other recipients. The original text is not quoted automatically. A sent reply marks the original as answered. A reply that the original would send to more than 50 addresses, or copy to more than 50, is refused until the agent names the recipients itself.
- **Deleting** is moving to the mailbox whose role is `trash`. MyMCPs never erases mail permanently.

## Attachment links

`get_message` lists the attachments of a message without downloading them. To get one, an agent calls `get_attachment_link` with the message and the attachment's `part`, and receives a link to this instance that it can fetch itself or hand to you:

```
https://mcp.example.com/files/12/eyJtYWlsYm94Ijoi…?signature=…
```

- **The link is the credential.** It needs no sign-in and no access token, so anyone who has it can download that one file until it expires. It is signed with the instance's `APP_KEY`: changing any part of it makes it invalid, and it cannot be turned into a link to another attachment.
- **It is temporary.** A link works for 15 minutes unless the agent asks for another duration, up to 60 minutes.
- **It follows the MCP.** Each download signs in to iCloud again, so a link stops working as soon as you remove **Read mail**, disable or delete the MCP, or revoke the password.
- **It only downloads.** The file is served as a download with its original name, never displayed in the browser on your instance's address. Files up to 30 MB are served, which covers what iCloud Mail accepts.
- **It needs `APP_URL`**, the public address agents and people reach your instance at.

Downloads are limited to 60 per 15 minutes for each MCP and client address, and one MCP serves 3 downloads at a time. Past either limit the link answers `429 Too Many Requests` with a `Retry-After` header. Requests without a valid signature are refused without being counted, and a download whose client stops reading for a minute is dropped.

## Sender addresses

Messages are sent from your iCloud Mail address. If your account has other addresses, such as aliases, a custom domain, or Hide My Email addresses, list them under **Other sender addresses** in the MCP's dialog to let agents use them:

- `send_message` and `create_draft` take a `from` address, which must be your iCloud Mail address or one of the listed addresses. Any other value is refused before anything is sent, and the error names the addresses that are allowed.
- A reply is sent from the address the original message was written to, when it is one of yours. Your own addresses are also left out of the recipients of a `reply_all`.
- The addresses must already exist on your iCloud account. MyMCPs cannot create them, and it always signs in with the main address.

## Limits

- **Plain text only.** Messages are sent without attachments or HTML.
- **No sender name.** Messages are sent from a bare address, without a display name.
- **Apple's sending limits apply**: 1,000 messages and 1,000 recipients a day, according to [Apple](https://support.apple.com/102198). MyMCPs accepts at most 50 addresses each in `to`, `cc`, and `bcc`, whether the agent names them or a reply takes them from the message it answers.
- **One sign-in per tool call.** Each call opens its own connection to iCloud and signs in again.
- **Call logs can hold mail.** With the logging level set to **arguments** or **responses** in **Settings**, the text agents send and the messages they read are stored in the MyMCPs call logs for the retention period. At **responses**, so are the attachment links, which stay valid until they expire.

## Troubleshooting

| What you see                                                                   | What to do                                                                                                                                                                                   |
| ------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| "Enter an app-specific password, which looks like abcd-efgh-ijkl-mnop"         | The value is not an app-specific password. Your Apple Account password does not work and is never accepted. Create one as described in step 1.                                               |
| "iCloud Mail rejected the sign-in" in the edit dialog or in a tool result      | Check that the address is your iCloud Mail address. The password may have been revoked, which also happens when the Apple Account password changes: create a new one and save it in the MCP. |
| "Could not reach iCloud Mail"                                                  | The instance cannot open a connection to `imap.mail.me.com:993` or `smtp.mail.me.com:587`. Check its outbound firewall rules.                                                                |
| A tool reports that a permission "is not allowed for this iCloud Mail MCP"     | Check that permission in the MCP's dialog and save.                                                                                                                                          |
| "Allow at least one permission"                                                | Every permission is unchecked. Check at least one, or disable the MCP instead.                                                                                                               |
| "Mailbox … does not exist"                                                     | Use the exact path from `list_mailboxes`, such as `Sent Messages` rather than `Sent`.                                                                                                        |
| "Message … was not found" or "None of these UIDs exist"                        | The UID belongs to another mailbox, or the message was moved. List the mailbox again.                                                                                                        |
| "from must be one of the sender addresses allowed for this MCP"                | Add the address under **Other sender addresses** in the MCP's dialog, or send from one of the listed addresses.                                                                              |
| "File links need the public address of this MyMCPs instance"                   | Set `APP_URL` to the public HTTPS origin and redeploy.                                                                                                                                       |
| A reply is refused because "at most 50 are allowed"                            | The message being answered names more than 50 addresses. The agent must name the recipients itself with `to` and `cc`, and leave `reply_all` off.                                            |
| "This message is written in HTML that could not be converted to text"          | Converting it took more than 5 seconds or 128 MB, which ordinary mail never does. Open it in Mail. If every HTML message is affected, the instance cannot start a `node` process.            |
| A link shows "This link is invalid or has expired"                             | Links are temporary. Ask the agent for a new one.                                                                                                                                            |
| A link shows "Too many downloads"                                              | This MCP served 60 downloads in 15 minutes to your address, or is serving 3 at once. Try again after the time given in `Retry-After`.                                                        |
| A link shows "This file is no longer available"                                | The message was moved or deleted, or the MCP no longer allows **Read mail**.                                                                                                                 |
| "iCloud Mail did not confirm the message, so it may or may not have been sent" | The connection dropped while sending. No copy was saved to Sent in that case, so check with the recipient before sending again.                                                              |
| "The message was sent, but its copy could not be saved to the Sent mailbox"    | The message was delivered. Only the copy is missing.                                                                                                                                         |

To disconnect completely, delete the MCP in MyMCPs, then revoke the password under **Sign-In and Security → App-Specific Passwords** at [account.apple.com](https://account.apple.com/account/manage).
