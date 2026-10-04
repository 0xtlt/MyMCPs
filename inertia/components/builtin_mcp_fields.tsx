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

type BuiltinSetupGuide = {
  provider: string
  /** What the admin does on the provider's site before coming back here. */
  createApplication: (publicApp: PublicApp) => ReactNode
  clientIdPlaceholder: string
  credentialsHint: string
  /** What every authorization can read. */
  readAccess: string
  /** What agents can change once write access is allowed. */
  writeAccess: string
}

const builtinSetupGuides: Record<string, BuiltinSetupGuide> = {
  strava: {
    provider: 'Strava',
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
}

export function builtinProviderName(builtinKey: string | null | undefined) {
  return (builtinKey && builtinSetupGuides[builtinKey]?.provider) || 'Built-in'
}

type Props = {
  builtinKey: string
  clientId: string
  clientSecret: string
  writeEnabled: boolean
  onChange: (patch: {
    oauthClientId?: string
    oauthClientSecret?: string
    builtinWriteEnabled?: boolean
  }) => void
  errors: Partial<Record<string, string>>
  hasSavedClientSecret: boolean
  isConnected: boolean
  publicApp: PublicApp | null
}

/**
 * Setup walkthrough and credentials for an MCP that MyMCPs runs itself. The
 * admin registers their own API application with the provider, so the steps
 * track how far that setup has come.
 */
export function BuiltinMcpFields({
  builtinKey,
  clientId,
  clientSecret,
  writeEnabled,
  onChange,
  errors,
  hasSavedClientSecret,
  isConnected,
  publicApp,
}: Props) {
  const guide = builtinSetupGuides[builtinKey]
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

  const activeStep = isConnected ? 3 : hasSavedClientSecret ? 2 : 0

  return (
    <>
      <input type="hidden" name="transport" value="builtin" />
      <input type="hidden" name="builtinKey" value={builtinKey} />
      <input type="hidden" name="authType" value="auto" />

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
              status={
                errors.oauthClientId ? { type: 'error', message: errors.oauthClientId } : undefined
              }
            />
            <TextInput
              label={hasSavedClientSecret ? 'Client Secret (leave blank to keep)' : 'Client Secret'}
              htmlName="oauthClientSecret"
              type="password"
              value={clientSecret}
              onChange={(oauthClientSecret) => onChange({ oauthClientSecret })}
              description="Encrypted at rest and only sent to the provider."
              autoComplete="off"
              width="100%"
              isOptional={hasSavedClientSecret}
              status={
                errors.oauthClientSecret
                  ? { type: 'error', message: errors.oauthClientSecret }
                  : undefined
              }
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
    </>
  )
}
