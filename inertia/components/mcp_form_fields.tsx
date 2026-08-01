import { CheckboxInput } from '@astryxdesign/core/CheckboxInput'
import { Button } from '@astryxdesign/core/Button'
import { HStack, StackItem, VStack } from '@astryxdesign/core/Layout'
import { RadioList, RadioListItem } from '@astryxdesign/core/RadioList'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Text } from '@astryxdesign/core/Text'

export type McpAuthType = 'none' | 'bearer' | 'header' | 'oauth'
export type McpTransport = 'http' | 'npm'

export type McpNpmEnvEntry = {
  name: string
  value: string
  hasValue?: boolean
}

export type McpFormSecrets = {
  hasAuthBearer?: boolean
  hasAuthHeaderValue?: boolean
  hasOauthClientSecret?: boolean
  hasOauthAccessToken?: boolean
}

export type McpFormValues = {
  name: string
  description: string
  transport: McpTransport
  httpUrl: string
  npmPackage: string
  npmVersion: string
  npmArgs: string
  npmEnv: McpNpmEnvEntry[]
  authType: McpAuthType
  authBearer: string
  authHeaderName: string
  authHeaderValue: string
  oauthAuthorizeUrl: string
  oauthTokenUrl: string
  oauthScopes: string
  oauthClientId: string
  oauthClientSecret: string
  enabled: boolean
}

export type McpFormPatch =
  Partial<McpFormValues> | ((current: McpFormValues) => Partial<McpFormValues>)

type Errors = Partial<Record<string, string>>

type Props = {
  values: McpFormValues
  onChange: (patch: McpFormPatch) => void
  errors?: Errors
  secrets?: McpFormSecrets
}

export function emptyMcpFormValues(): McpFormValues {
  return {
    name: '',
    description: '',
    transport: 'http',
    httpUrl: '',
    npmPackage: '',
    npmVersion: '',
    npmArgs: '',
    npmEnv: [],
    authType: 'none',
    authBearer: '',
    authHeaderName: '',
    authHeaderValue: '',
    oauthAuthorizeUrl: '',
    oauthTokenUrl: '',
    oauthScopes: '',
    oauthClientId: '',
    oauthClientSecret: '',
    enabled: true,
  }
}

export function mcpFormValuesFromRow(mcp: {
  name: string
  description: string | null
  transport: McpTransport
  httpUrl: string | null
  npmPackage: string | null
  npmVersion: string | null
  npmArgs: string
  npmEnv: Array<{ name: string; hasValue: boolean }>
  authType: McpAuthType
  authHeaderName: string | null
  oauthAuthorizeUrl: string | null
  oauthTokenUrl: string | null
  oauthScopes: string | null
  oauthClientId: string | null
  enabled: boolean
}): McpFormValues {
  return {
    ...emptyMcpFormValues(),
    name: mcp.name,
    description: mcp.description ?? '',
    transport: mcp.transport,
    httpUrl: mcp.httpUrl ?? '',
    npmPackage: mcp.npmPackage ?? '',
    npmVersion: mcp.npmVersion ?? '',
    npmArgs: mcp.npmArgs,
    npmEnv: mcp.npmEnv.map(({ name, hasValue }) => ({ name, value: '', hasValue })),
    authType: mcp.authType,
    authHeaderName: mcp.authHeaderName ?? '',
    oauthAuthorizeUrl: mcp.oauthAuthorizeUrl ?? '',
    oauthTokenUrl: mcp.oauthTokenUrl ?? '',
    oauthScopes: mcp.oauthScopes ?? '',
    oauthClientId: mcp.oauthClientId ?? '',
    enabled: mcp.enabled,
  }
}

export function McpFormFields({ values, onChange, errors = {}, secrets = {} }: Props) {
  return (
    <VStack gap={4} hAlign="stretch">
      <TextInput
        label="Name"
        htmlName="name"
        value={values.name}
        onChange={(name) => onChange({ name })}
        width="100%"
        status={errors.name ? { type: 'error', message: errors.name } : undefined}
      />
      <TextInput
        label="Description"
        htmlName="description"
        value={values.description}
        onChange={(description) => onChange({ description })}
        isOptional
        width="100%"
      />

      <RadioList
        label="Transport"
        htmlName="transport"
        value={values.transport}
        onChange={(transport) =>
          onChange({
            transport: transport as McpTransport,
            ...(transport === 'npm' && values.authType === 'oauth'
              ? { authType: 'none' as McpAuthType }
              : {}),
          })
        }
        orientation="horizontal"
      >
        <RadioListItem value="http" label="HTTP" description="Remote Streamable HTTP URL" />
        <RadioListItem value="npm" label="npm package" description="Runs in a Deno sandbox" />
      </RadioList>

      {values.transport === 'http' ? (
        <TextInput
          label="HTTP URL"
          htmlName="httpUrl"
          value={values.httpUrl}
          onChange={(httpUrl) => onChange({ httpUrl })}
          placeholder="https://example.com/mcp"
          width="100%"
          status={errors.httpUrl ? { type: 'error', message: errors.httpUrl } : undefined}
        />
      ) : (
        <VStack gap={3} hAlign="stretch">
          <TextInput
            label="npm package"
            htmlName="npmPackage"
            value={values.npmPackage}
            onChange={(npmPackage) => onChange({ npmPackage })}
            placeholder="@modelcontextprotocol/server-everything"
            width="100%"
            status={errors.npmPackage ? { type: 'error', message: errors.npmPackage } : undefined}
          />
          <HStack gap={3} wrap="wrap">
            <TextInput
              label="Version"
              htmlName="npmVersion"
              value={values.npmVersion}
              onChange={(npmVersion) => onChange({ npmVersion })}
              placeholder="latest"
              isOptional
              width={200}
            />
            <TextInput
              label="Extra args"
              htmlName="npmArgs"
              value={values.npmArgs}
              onChange={(npmArgs) => onChange({ npmArgs })}
              isOptional
              width={320}
            />
          </HStack>
          <VStack gap={2} hAlign="stretch">
            <HStack gap={2} hAlign="between" vAlign="center">
              <Text type="body" weight="bold">
                Environment variables
              </Text>
              <Button
                label="Add variable"
                variant="secondary"
                size="sm"
                onClick={() =>
                  onChange((current) => ({
                    npmEnv: [...current.npmEnv, { name: '', value: '' }],
                  }))
                }
              />
            </HStack>
            <Text type="supporting" color="secondary">
              Values are encrypted at rest. Leave an existing value blank to keep it unchanged.
            </Text>
            {values.npmEnv.map((entry, index) => {
              const nameError = errors[`npmEnv.${index}.name`]
              const valueError = errors[`npmEnv.${index}.value`]
              return (
                <HStack key={`npm-env-${index}`} gap={3} vAlign="end">
                  <StackItem size="fill">
                    <TextInput
                      label="Variable name"
                      htmlName={`npmEnv[${index}][name]`}
                      value={entry.name}
                      onChange={(name) =>
                        onChange((current) => ({
                          npmEnv: current.npmEnv.map((entry, entryIndex) =>
                            entryIndex === index ? { ...entry, name } : entry
                          ),
                        }))
                      }
                      placeholder="API_KEY"
                      width="100%"
                      status={nameError ? { type: 'error', message: nameError } : undefined}
                    />
                  </StackItem>
                  <StackItem size="fill">
                    <TextInput
                      label={entry.hasValue ? 'Value (leave blank to keep)' : 'Value'}
                      htmlName={`npmEnv[${index}][value]`}
                      type="password"
                      value={entry.value}
                      onChange={(value) =>
                        onChange((current) => ({
                          npmEnv: current.npmEnv.map((entry, entryIndex) =>
                            entryIndex === index ? { ...entry, value } : entry
                          ),
                        }))
                      }
                      width="100%"
                      isOptional={Boolean(entry.hasValue)}
                      status={valueError ? { type: 'error', message: valueError } : undefined}
                    />
                  </StackItem>
                  <Button
                    label="Remove"
                    variant="ghost"
                    size="sm"
                    onClick={() =>
                      onChange((current) => ({
                        npmEnv: current.npmEnv.filter((_, currentIndex) => currentIndex !== index),
                      }))
                    }
                  />
                </HStack>
              )
            })}
          </VStack>
        </VStack>
      )}

      <RadioList
        label="Authentication"
        htmlName="authType"
        value={values.authType}
        onChange={(authType) => onChange({ authType: authType as McpAuthType })}
      >
        <RadioListItem value="none" label="None" />
        <RadioListItem value="bearer" label="Bearer token" />
        <RadioListItem value="header" label="Custom header" />
        {values.transport === 'http' ? <RadioListItem value="oauth" label="OAuth" /> : null}
      </RadioList>

      {values.authType === 'bearer' ? (
        <TextInput
          label={secrets.hasAuthBearer ? 'Bearer token (leave blank to keep)' : 'Bearer token'}
          htmlName="authBearer"
          type="password"
          value={values.authBearer}
          onChange={(authBearer) => onChange({ authBearer })}
          width="100%"
          isOptional={Boolean(secrets.hasAuthBearer)}
        />
      ) : null}

      {values.authType === 'header' ? (
        <HStack gap={3} wrap="wrap">
          <TextInput
            label="Header name"
            htmlName="authHeaderName"
            value={values.authHeaderName}
            onChange={(authHeaderName) => onChange({ authHeaderName })}
            width={240}
          />
          <TextInput
            label={
              secrets.hasAuthHeaderValue ? 'Header value (leave blank to keep)' : 'Header value'
            }
            htmlName="authHeaderValue"
            type="password"
            value={values.authHeaderValue}
            onChange={(authHeaderValue) => onChange({ authHeaderValue })}
            width={320}
            isOptional={Boolean(secrets.hasAuthHeaderValue)}
          />
        </HStack>
      ) : null}

      {values.authType === 'oauth' ? (
        <VStack gap={3} hAlign="stretch">
          <Text type="supporting" color="secondary">
            OAuth endpoints and client registration are discovered automatically from the HTTP MCP
            when you connect. The fields below are optional overrides for providers without standard
            discovery.
          </Text>
          <TextInput
            label="Authorize URL (optional override)"
            htmlName="oauthAuthorizeUrl"
            value={values.oauthAuthorizeUrl}
            onChange={(oauthAuthorizeUrl) => onChange({ oauthAuthorizeUrl })}
            width="100%"
            isOptional
          />
          <TextInput
            label="Token URL (optional override)"
            htmlName="oauthTokenUrl"
            value={values.oauthTokenUrl}
            onChange={(oauthTokenUrl) => onChange({ oauthTokenUrl })}
            width="100%"
            isOptional
          />
          <HStack gap={3} wrap="wrap">
            <TextInput
              label="Client ID (optional override)"
              htmlName="oauthClientId"
              value={values.oauthClientId}
              onChange={(oauthClientId) => onChange({ oauthClientId })}
              width={240}
              isOptional
            />
            <TextInput
              label={
                secrets.hasOauthClientSecret
                  ? 'Client secret (leave blank to keep)'
                  : 'Client secret (optional override)'
              }
              htmlName="oauthClientSecret"
              type="password"
              value={values.oauthClientSecret}
              onChange={(oauthClientSecret) => onChange({ oauthClientSecret })}
              width={240}
              isOptional={Boolean(secrets.hasOauthClientSecret)}
            />
            <TextInput
              label="Scopes"
              htmlName="oauthScopes"
              value={values.oauthScopes}
              onChange={(oauthScopes) => onChange({ oauthScopes })}
              isOptional
              width={240}
            />
          </HStack>
          {secrets.hasOauthAccessToken !== undefined ? (
            <Text type="supporting" color="secondary">
              OAuth access token: {secrets.hasOauthAccessToken ? 'connected' : 'not connected yet'}
            </Text>
          ) : null}
        </VStack>
      ) : null}

      <CheckboxInput
        label="Enabled"
        htmlName="enabled"
        value={values.enabled}
        onChange={(enabled) => onChange({ enabled })}
        description="Disabled MCPs are excluded from the gateway"
      />
    </VStack>
  )
}
