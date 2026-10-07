import type { ReactNode } from 'react'
import { Banner } from '@astryxdesign/core/Banner'
import { CheckboxInput } from '@astryxdesign/core/CheckboxInput'
import { CodeBlock } from '@astryxdesign/core/CodeBlock'
import { VStack } from '@astryxdesign/core/Layout'
import { Link } from '@astryxdesign/core/Link'
import { Step, Stepper } from '@astryxdesign/core/Stepper'
import { Text } from '@astryxdesign/core/Text'
import { TextInput } from '@astryxdesign/core/TextInput'
import { AppIconDownload } from '~/components/app_icon_download'

/** The instance's public origin, which providers ask for when registering an app. */
export type PublicApp = { url: string; hostname: string }

type SetupGuide = {
  provider: string
  /** How the MCP reaches the provider, shown in the registry. */
  endpoint: string
  /** What the admin brings to the setup, shown under the dialog title. */
  requirement: string
}

/** The admin registers an API application, then approves access on the provider's site. */
type OauthSetupGuide = SetupGuide & {
  signIn: 'oauth'
  /** What the admin does on the provider's site before coming back here. */
  createApplication: (publicApp: PublicApp) => ReactNode
  clientIdPlaceholder: string
  credentialsHint: string
  /** What every authorization can read. */
  readAccess: string
  /** What agents can change once write access is allowed. */
  writeAccess: string
}

/** The admin creates a password for apps on the provider's site and pastes it here. */
type PasswordSetupGuide = SetupGuide & {
  signIn: 'password'
  /** What the admin does on the provider's site before coming back here. */
  createPasswordLabel: string
  createPassword: ReactNode
  passwordLabel: string
  usernameLabel: string
  usernamePlaceholder: string
  usernameHint: string
  passwordPlaceholder: string
  /** Other addresses of the same account that agents may act as. */
  aliasesLabel: string
  aliasesPlaceholder: string
  aliasesHint: string
  /** Why the permissions are chosen here and not on the provider's site. */
  permissionsHint: string
  /** What the admin can allow. The provider cannot restrict the password itself. */
  permissions: ReadonlyArray<{ key: string; label: string; description: string }>
}

const builtinSetupGuides: Record<string, OauthSetupGuide | PasswordSetupGuide> = {
  'strava': {
    signIn: 'oauth',
    provider: 'Strava',
    endpoint: 'Strava API',
    requirement: 'Runs inside MyMCPs with your own API application',
    createApplication: (publicApp) => (
      <>
        <Text type="body" color="secondary">
          Open{' '}
          <Link href="https://www.strava.com/settings/api" isExternalLink>
            Strava API settings
          </Link>{' '}
          and create an application. Strava states that this requires a Strava subscription. Any
          name and category work. Enter these two values:
        </Text>
        <CodeBlock title="Website" code={publicApp.url} width="100%" size="sm" />
        <CodeBlock
          title="Authorization Callback Domain"
          code={publicApp.hostname}
          width="100%"
          size="sm"
        />
        <Text type="supporting" color="secondary">
          The callback domain has no https:// and no path. Strava then asks for an application icon
          (JPG or PNG) before it shows the credentials. This one is ready to upload:
        </Text>
        <AppIconDownload />
      </>
    ),
    clientIdPlaceholder: '123456',
    credentialsHint: 'Both are shown on the My API Application page once the application exists.',
    readAccess:
      'MyMCPs reads your profile, activities, routes, and segments, including private ones. Permissions you uncheck on Strava hide the matching tools.',
    writeAccess:
      'Lets agents create manual activities, edit activity details, star segments, and update your weight.',
  },
  'icloud-mail': {
    signIn: 'password',
    provider: 'iCloud Mail',
    endpoint: 'iCloud Mail over IMAP and SMTP',
    requirement: 'Runs inside MyMCPs with an app-specific password',
    createPasswordLabel: 'Create an app-specific password',
    passwordLabel: 'App-specific password',
    createPassword: (
      <>
        <Text type="body" color="secondary">
          Open{' '}
          <Link href="https://account.apple.com/account/manage" isExternalLink>
            your Apple Account
          </Link>
          , select Sign-In and Security, then App-Specific Passwords, and create one named MyMCPs.
          Apple offers this once two-factor authentication is turned on.
        </Text>
        <Text type="supporting" color="secondary">
          Apple shows the password once, as four groups of letters. You can revoke it from the same
          page at any time without changing your Apple Account password.
        </Text>
      </>
    ),
    usernameLabel: 'iCloud Mail address',
    usernamePlaceholder: 'name@icloud.com',
    usernameHint:
      'The address ending in @icloud.com, @me.com, or @mac.com, even if you sign in to Apple with another one.',
    passwordPlaceholder: 'abcd-efgh-ijkl-mnop',
    aliasesLabel: 'Other sender addresses',
    aliasesPlaceholder: 'alias@icloud.com, me@example.com',
    aliasesHint:
      'Aliases, custom domain addresses, or Hide My Email addresses of this account that agents may also send from, separated by commas. They must already be set up in iCloud Mail.',
    permissionsHint:
      'Apple cannot limit what an app-specific password reaches, so MyMCPs enforces these permissions itself. Agents only get the tools of the permissions you allow.',
    permissions: [
      {
        key: 'read',
        label: 'Read mail',
        description:
          'List mailboxes, search, read messages, and get temporary links to download their attachments. Reading does not mark a message as read.',
      },
      {
        key: 'draft',
        label: 'Save drafts',
        description:
          'Write messages, with the files agents attach, to your Drafts mailbox for you to review and send yourself.',
      },
      {
        key: 'send',
        label: 'Send mail',
        description:
          'Send messages from your address, with the files agents attach. A sent message cannot be recalled.',
      },
      {
        key: 'organize',
        label: 'Organize mail',
        description:
          'Mark messages as read or flagged and move them between mailboxes, including to the Trash.',
      },
    ],
  },
}

export function builtinSetupGuide(builtinKey: string | null | undefined) {
  return builtinKey ? builtinSetupGuides[builtinKey] : undefined
}

export function builtinProviderName(builtinKey: string | null | undefined) {
  return builtinSetupGuide(builtinKey)?.provider ?? 'Built-in'
}

type Props = {
  builtinKey: string
  clientId: string
  clientSecret: string
  username: string
  password: string
  aliases: string
  permissions: string[]
  writeEnabled: boolean
  onChange: (patch: {
    oauthClientId?: string
    oauthClientSecret?: string
    builtinUsername?: string
    builtinPassword?: string
    builtinAliases?: string
    builtinPermissions?: string[]
    builtinWriteEnabled?: boolean
  }) => void
  errors: Partial<Record<string, string>>
  hasSavedClientSecret: boolean
  hasSavedPassword: boolean
  isConnected: boolean
  publicApp: PublicApp | null
}

function fieldStatus(message: string | undefined) {
  return message ? ({ type: 'error', message } as const) : undefined
}

/**
 * Setup walkthrough and credentials for an MCP that MyMCPs runs itself. The
 * admin gets the credentials from the provider, so the steps track how far
 * that setup has come.
 */
export function BuiltinMcpFields({
  builtinKey,
  clientId,
  clientSecret,
  username,
  password,
  aliases,
  permissions,
  writeEnabled,
  onChange,
  errors,
  hasSavedClientSecret,
  hasSavedPassword,
  isConnected,
  publicApp,
}: Props) {
  const guide = builtinSetupGuide(builtinKey)
  if (!guide) {
    return (
      <Banner
        status="error"
        title="Unknown built-in MCP"
        description="This version of MyMCPs does not include this built-in MCP."
        container="card"
      />
    )
  }

  const hasSavedSecret = guide.signIn === 'oauth' ? hasSavedClientSecret : hasSavedPassword
  // Saved OAuth credentials still wait for Connect. A saved password that does
  // not work has to be entered again.
  const pendingStep = guide.signIn === 'oauth' ? 2 : 1
  const activeStep = isConnected ? 3 : hasSavedSecret ? pendingStep : 0

  return (
    <>
      <input type="hidden" name="transport" value="builtin" />
      <input type="hidden" name="builtinKey" value={builtinKey} />
      <input type="hidden" name="authType" value="auto" />

      {guide.signIn === 'oauth' ? (
        <Stepper activeStep={activeStep} orientation="vertical" label={`${guide.provider} setup`}>
          <Step step={0} label={`Create a ${guide.provider} API application`}>
            <VStack gap={3} hAlign="stretch">
              {publicApp ? (
                guide.createApplication(publicApp)
              ) : (
                <Banner
                  status="warning"
                  title="Set APP_URL first"
                  description={`${guide.provider} sends you back to this instance after you approve access. Set APP_URL to its public HTTPS origin and redeploy to see the values to enter.`}
                  container="card"
                />
              )}
            </VStack>
          </Step>
          <Step step={1} label="Paste its Client ID and Client Secret">
            <VStack gap={3} hAlign="stretch">
              <Text type="supporting" color="secondary">
                {guide.credentialsHint}
              </Text>
              <TextInput
                label="Client ID"
                htmlName="oauthClientId"
                value={clientId}
                onChange={(oauthClientId) => onChange({ oauthClientId })}
                placeholder={guide.clientIdPlaceholder}
                autoComplete="off"
                width="100%"
                status={fieldStatus(errors.oauthClientId)}
              />
              <TextInput
                label={
                  hasSavedClientSecret ? 'Client Secret (leave blank to keep)' : 'Client Secret'
                }
                htmlName="oauthClientSecret"
                type="password"
                value={clientSecret}
                onChange={(oauthClientSecret) => onChange({ oauthClientSecret })}
                description="Encrypted at rest and only sent to the provider."
                autoComplete="off"
                width="100%"
                isOptional={hasSavedClientSecret}
                status={fieldStatus(errors.oauthClientSecret)}
              />
            </VStack>
          </Step>
          <Step step={2} label={`Connect your ${guide.provider} account`}>
            <VStack gap={3} hAlign="stretch">
              <Text type="supporting" color="secondary">
                {isConnected
                  ? `Connected. ${guide.readAccess}`
                  : `Once this MCP is saved, select Connect and approve access on ${guide.provider}. ${guide.readAccess}`}
              </Text>
              <CheckboxInput
                label="Allow write access"
                htmlName="builtinWriteEnabled"
                value={writeEnabled}
                onChange={(builtinWriteEnabled) => onChange({ builtinWriteEnabled })}
                description={`${guide.writeAccess} Turning it on applies the next time you connect or re-authorize.`}
              />
            </VStack>
          </Step>
        </Stepper>
      ) : (
        <Stepper activeStep={activeStep} orientation="vertical" label={`${guide.provider} setup`}>
          <Step step={0} label={guide.createPasswordLabel}>
            <VStack gap={3} hAlign="stretch">
              {guide.createPassword}
            </VStack>
          </Step>
          <Step step={1} label="Enter your address and the password">
            <VStack gap={3} hAlign="stretch">
              <TextInput
                label={guide.usernameLabel}
                htmlName="builtinUsername"
                value={username}
                onChange={(builtinUsername) => onChange({ builtinUsername })}
                placeholder={guide.usernamePlaceholder}
                description={guide.usernameHint}
                autoComplete="off"
                width="100%"
                status={fieldStatus(errors.builtinUsername)}
              />
              <TextInput
                label={
                  hasSavedPassword
                    ? `${guide.passwordLabel} (leave blank to keep)`
                    : guide.passwordLabel
                }
                htmlName="builtinPassword"
                type="password"
                value={password}
                onChange={(builtinPassword) => onChange({ builtinPassword })}
                placeholder={hasSavedPassword ? undefined : guide.passwordPlaceholder}
                description="Encrypted at rest and only sent to the provider's mail servers."
                autoComplete="off"
                width="100%"
                isOptional={hasSavedPassword}
                status={fieldStatus(errors.builtinPassword)}
              />
              <TextInput
                label={guide.aliasesLabel}
                htmlName="builtinAliases"
                value={aliases}
                onChange={(builtinAliases) => onChange({ builtinAliases })}
                placeholder={guide.aliasesPlaceholder}
                description={guide.aliasesHint}
                autoComplete="off"
                width="100%"
                isOptional
                status={fieldStatus(errors.builtinAliases)}
              />
            </VStack>
          </Step>
          <Step step={2} label="Choose what agents can do">
            <VStack gap={3} hAlign="stretch">
              <Text type="supporting" color="secondary">
                {isConnected
                  ? `Connected. ${guide.permissionsHint}`
                  : `Saving this MCP signs in to ${guide.provider} to check the password. ${guide.permissionsHint}`}
              </Text>
              {guide.permissions.map(({ key, label, description }) => (
                <CheckboxInput
                  key={key}
                  label={label}
                  value={permissions.includes(key)}
                  onChange={(isAllowed) =>
                    onChange({
                      builtinPermissions: isAllowed
                        ? [...permissions, key]
                        : permissions.filter((permission) => permission !== key),
                    })
                  }
                  description={description}
                />
              ))}
              {permissions.map((permission) => (
                <input
                  key={permission}
                  type="hidden"
                  name="builtinPermissions[]"
                  value={permission}
                />
              ))}
              {errors.builtinPermissions ? (
                <Banner
                  status="error"
                  title={errors.builtinPermissions}
                  description="Without a permission, agents would get no tool from this MCP."
                  container="card"
                />
              ) : null}
            </VStack>
          </Step>
        </Stepper>
      )}
    </>
  )
}
