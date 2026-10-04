import type { BuiltinPasswordMcpDefinition } from '#services/builtin/definition'
import { withImap } from '#services/builtin/icloud_mail/connection'
import {
  downloadAttachment,
  ICLOUD_MAIL_PERMISSIONS,
  icloudMailTools,
} from '#services/builtin/icloud_mail/tools'

/**
 * Apple has no mail API and no OAuth for third parties. iCloud Mail is reached
 * over IMAP and SMTP with an app-specific password the admin creates at
 * https://account.apple.com, which Apple only offers with two-factor
 * authentication turned on. Apple cannot limit what that password reaches, so
 * the admin chooses what agents may do with it when adding the MCP.
 */
export const icloudMailMcp: BuiltinPasswordMcpDefinition = {
  key: 'icloud-mail',
  name: 'iCloud Mail',
  password: {
    usernamePattern: /^[^\s@]+@[^\s@]+\.[^\s@]+$/,
    usernameHint: 'Enter your iCloud Mail address, such as name@icloud.com',
    // Apple shows them as four groups of four lowercase letters, which an
    // Apple Account password cannot match by accident.
    passwordPattern: /^[a-z]{4}(-?[a-z]{4}){3}$/,
    passwordHint:
      'Enter an app-specific password, which looks like abcd-efgh-ijkl-mnop. Your Apple Account password does not work here.',
    permissions: ICLOUD_MAIL_PERMISSIONS,
    aliasHint:
      'Enter up to 20 other addresses of this iCloud account, such as alias@icloud.com, separated by commas',
  },
  tools: icloudMailTools,
  verify: (signIn) => withImap(signIn, async () => {}),
  download: downloadAttachment,
}
