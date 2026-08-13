import { CheckboxInput } from '@astryxdesign/core/CheckboxInput'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { RadioList, RadioListItem } from '@astryxdesign/core/RadioList'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Text } from '@astryxdesign/core/Text'

export type McpAuthType = 'auto' | 'bearer' | 'header'
export type McpTransport = 'http' | 'npm'

export type McpFormSecrets = {
  hasAuthBearer?: boolean
  hasAuthHeaderValue?: boolean
}

export type McpEnvironmentVariable = {
  id: string
  name: string
  value: string
  hasValue: boolean
  originalName?: string
}

export type McpFormValues = {
  name: string
  description: string
  transport: McpTransport
  httpUrl: string
  npmPackage: string
  npmVersion: string
  npmArgs: string
  npmEnv: McpEnvironmentVariable[]
  authType: McpAuthType
  authBearer: string
  authHeaderName: string
  authHeaderValue: string
  enabled: boolean
}

type Errors = Partial<Record<string, string>>

type Props = {
  values: McpFormValues
  onChange: (patch: Partial<McpFormValues>) => void
  errors?: Errors
  secrets?: McpFormSecrets
  cachedVersion?: string | null
}

let nextEnvironmentVariableId = 0

function newEnvironmentVariable(): McpEnvironmentVariable {
  nextEnvironmentVariableId += 1
  return {
    id: `new-env-${nextEnvironmentVariableId}`,
    name: '',
    value: '',
    hasValue: false,
  }
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
    authType: 'auto',
    authBearer: '',
    authHeaderName: '',
    authHeaderValue: '',
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
    npmEnv: mcp.npmEnv.map((entry, index) => ({
      id: `stored-env-${index}`,
      name: entry.name,
      value: '',
      hasValue: entry.hasValue,
      originalName: entry.name,
    })),
    authType: mcp.authType,
    authHeaderName: mcp.authHeaderName ?? '',
    enabled: mcp.enabled,
  }
}

export function McpFormFields({
  values,
  onChange,
  errors = {},
  secrets = {},
  cachedVersion,
}: Props) {
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
        onChange={(transport) => onChange({ transport: transport as McpTransport })}
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
          <HStack className="mobile-full-width-fields" gap={3} wrap="wrap" vAlign="start">
            <VStack className="mobile-full-width" gap={1} hAlign="stretch" width={200}>
              <TextInput
                label="Version"
                htmlName="npmVersion"
                value={values.npmVersion}
                onChange={(npmVersion) => onChange({ npmVersion })}
                placeholder="latest"
                isOptional
                width="100%"
              />
              {cachedVersion ? (
                <Text type="supporting" color="secondary">
                  Cached in Deno: {cachedVersion}
                </Text>
              ) : null}
            </VStack>
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
            <VStack gap={1} hAlign="stretch">
              <Text type="body" weight="bold">
                Environment variables
              </Text>
              <Text type="supporting" color="secondary">
                Values are encrypted and only provided to this MCP process.
              </Text>
            </VStack>
            {values.npmEnv.map((entry, index) => {
              const canKeepExistingValue =
                entry.hasValue &&
                entry.originalName !== undefined &&
                entry.name === entry.originalName

              return (
                <HStack
                  key={entry.id}
                  className="mobile-full-width-fields"
                  gap={2}
                  wrap="wrap"
                  vAlign="end"
                >
                  <TextInput
                    label="Name"
                    htmlName={`npmEnv[${index}][name]`}
                    value={entry.name}
                    onChange={(name) =>
                      onChange({
                        npmEnv: values.npmEnv.map((current, currentIndex) =>
                          currentIndex === index ? { ...current, name } : current
                        ),
                      })
                    }
                    placeholder="API_KEY"
                    width={200}
                    status={
                      errors[`npmEnv.${index}.name`]
                        ? { type: 'error', message: errors[`npmEnv.${index}.name`] }
                        : undefined
                    }
                  />
                  <TextInput
                    label="Value"
                    htmlName={`npmEnv[${index}][value]`}
                    type="password"
                    value={entry.value}
                    onChange={(value) =>
                      onChange({
                        npmEnv: values.npmEnv.map((current, currentIndex) =>
                          currentIndex === index ? { ...current, value } : current
                        ),
                      })
                    }
                    description={
                      canKeepExistingValue ? 'Leave blank to keep the saved value' : undefined
                    }
                    isOptional={canKeepExistingValue}
                    isRequired={!canKeepExistingValue}
                    width={260}
                    status={
                      errors[`npmEnv.${index}.value`]
                        ? { type: 'error', message: errors[`npmEnv.${index}.value`] }
                        : undefined
                    }
                  />
                  <Button
                    type="button"
                    label={`Remove ${entry.name || 'environment variable'}`}
                    variant="secondary"
                    size="sm"
                    onClick={() =>
                      onChange({
                        npmEnv: values.npmEnv.filter((_, currentIndex) => currentIndex !== index),
                      })
                    }
                  >
                    Remove
                  </Button>
                </HStack>
              )
            })}
            <HStack gap={2} hAlign="start">
              <Button
                type="button"
                label="Add variable"
                variant="secondary"
                size="sm"
                isDisabled={values.npmEnv.length >= 50}
                onClick={() => onChange({ npmEnv: [...values.npmEnv, newEnvironmentVariable()] })}
              />
            </HStack>
            {errors.npmEnv ? (
              <Banner
                status="error"
                title="Environment variables error"
                description={errors.npmEnv}
              />
            ) : null}
          </VStack>
        </VStack>
      )}

      <RadioList
        label="Authentication"
        htmlName="authType"
        value={values.authType}
        onChange={(authType) => onChange({ authType: authType as McpAuthType })}
      >
        <RadioListItem
          value="auto"
          label="Auto"
          description="Connect without credentials or detect OAuth automatically"
        />
        <RadioListItem value="bearer" label="Bearer token" />
        <RadioListItem value="header" label="Custom header" />
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
        <HStack className="mobile-full-width-fields" gap={3} wrap="wrap">
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
