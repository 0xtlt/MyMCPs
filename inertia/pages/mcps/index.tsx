import { useState } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Badge } from '@astryxdesign/core/Badge'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { CheckboxInput } from '@astryxdesign/core/CheckboxInput'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { List, ListItem } from '@astryxdesign/core/List'
import { RadioList, RadioListItem } from '@astryxdesign/core/RadioList'
import { StatusDot } from '@astryxdesign/core/StatusDot'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

type McpRow = {
  id: number
  name: string
  slug: string
  description: string | null
  transport: 'http' | 'npm'
  httpUrl: string | null
  npmPackage: string | null
  npmVersion: string | null
  npmArgs: string
  authType: 'none' | 'bearer' | 'header' | 'oauth'
  status: 'draft' | 'ready' | 'error'
  lastError: string | null
  enabled: boolean
}

function statusVariant(status: McpRow['status']): 'success' | 'warning' | 'error' | 'neutral' {
  if (status === 'ready') return 'success'
  if (status === 'error') return 'error'
  if (status === 'draft') return 'warning'
  return 'neutral'
}

export default function McpsIndex({ mcps }: { mcps: McpRow[] }) {
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [transport, setTransport] = useState('http')
  const [httpUrl, setHttpUrl] = useState('')
  const [npmPackage, setNpmPackage] = useState('')
  const [npmVersion, setNpmVersion] = useState('')
  const [npmArgs, setNpmArgs] = useState('')
  const [authType, setAuthType] = useState('none')
  const [authBearer, setAuthBearer] = useState('')
  const [authHeaderName, setAuthHeaderName] = useState('')
  const [authHeaderValue, setAuthHeaderValue] = useState('')
  const [oauthAuthorizeUrl, setOauthAuthorizeUrl] = useState('')
  const [oauthTokenUrl, setOauthTokenUrl] = useState('')
  const [oauthScopes, setOauthScopes] = useState('')
  const [oauthClientId, setOauthClientId] = useState('')
  const [oauthClientSecret, setOauthClientSecret] = useState('')
  const [enabled, setEnabled] = useState(true)

  return (
    <VStack gap={6} maxWidth={840} width="100%">
      <VStack gap={2}>
        <Heading level={1}>MCPs</Heading>
        <Text type="body" color="secondary">
          Register upstream MCP servers. Agents reach them through MyMCPs with an access token.
        </Text>
      </VStack>

      <Card padding={6} width="100%">
        <Form route="mcps.store">
          {({ errors, processing }) => (
            <VStack gap={4} hAlign="stretch">
              <Heading level={2}>Add MCP</Heading>

              <TextInput
                label="Name"
                htmlName="name"
                value={name}
                onChange={setName}
                width="100%"
                status={errors.name ? { type: 'error', message: errors.name } : undefined}
              />
              <TextInput
                label="Description"
                htmlName="description"
                value={description}
                onChange={setDescription}
                isOptional
                width="100%"
              />

              <RadioList
                label="Transport"
                htmlName="transport"
                value={transport}
                onChange={setTransport}
                orientation="horizontal"
              >
                <RadioListItem value="http" label="HTTP" description="Remote Streamable HTTP URL" />
                <RadioListItem
                  value="npm"
                  label="npm package"
                  description="Runs in a Deno sandbox"
                />
              </RadioList>

              {transport === 'http' ? (
                <TextInput
                  label="HTTP URL"
                  htmlName="httpUrl"
                  value={httpUrl}
                  onChange={setHttpUrl}
                  placeholder="https://example.com/mcp"
                  width="100%"
                  status={errors.httpUrl ? { type: 'error', message: errors.httpUrl } : undefined}
                />
              ) : (
                <VStack gap={3} hAlign="stretch">
                  <TextInput
                    label="npm package"
                    htmlName="npmPackage"
                    value={npmPackage}
                    onChange={setNpmPackage}
                    placeholder="@modelcontextprotocol/server-everything"
                    width="100%"
                    status={
                      errors.npmPackage ? { type: 'error', message: errors.npmPackage } : undefined
                    }
                  />
                  <HStack gap={3} wrap="wrap">
                    <TextInput
                      label="Version"
                      htmlName="npmVersion"
                      value={npmVersion}
                      onChange={setNpmVersion}
                      placeholder="latest"
                      isOptional
                      width={200}
                    />
                    <TextInput
                      label="Extra args"
                      htmlName="npmArgs"
                      value={npmArgs}
                      onChange={setNpmArgs}
                      isOptional
                      width={320}
                    />
                  </HStack>
                </VStack>
              )}

              <RadioList
                label="Authentication"
                htmlName="authType"
                value={authType}
                onChange={setAuthType}
              >
                <RadioListItem value="none" label="None" />
                <RadioListItem value="bearer" label="Bearer token" />
                <RadioListItem value="header" label="Custom header" />
                <RadioListItem value="oauth" label="OAuth" />
              </RadioList>

              {authType === 'bearer' ? (
                <TextInput
                  label="Bearer token"
                  htmlName="authBearer"
                  type="password"
                  value={authBearer}
                  onChange={setAuthBearer}
                  width="100%"
                />
              ) : null}

              {authType === 'header' ? (
                <HStack gap={3} wrap="wrap">
                  <TextInput
                    label="Header name"
                    htmlName="authHeaderName"
                    value={authHeaderName}
                    onChange={setAuthHeaderName}
                    width={240}
                  />
                  <TextInput
                    label="Header value"
                    htmlName="authHeaderValue"
                    type="password"
                    value={authHeaderValue}
                    onChange={setAuthHeaderValue}
                    width={320}
                  />
                </HStack>
              ) : null}

              {authType === 'oauth' ? (
                <VStack gap={3} hAlign="stretch">
                  <TextInput
                    label="Authorize URL"
                    htmlName="oauthAuthorizeUrl"
                    value={oauthAuthorizeUrl}
                    onChange={setOauthAuthorizeUrl}
                    width="100%"
                  />
                  <TextInput
                    label="Token URL"
                    htmlName="oauthTokenUrl"
                    value={oauthTokenUrl}
                    onChange={setOauthTokenUrl}
                    width="100%"
                  />
                  <HStack gap={3} wrap="wrap">
                    <TextInput
                      label="Client ID"
                      htmlName="oauthClientId"
                      value={oauthClientId}
                      onChange={setOauthClientId}
                      width={240}
                    />
                    <TextInput
                      label="Client secret"
                      htmlName="oauthClientSecret"
                      type="password"
                      value={oauthClientSecret}
                      onChange={setOauthClientSecret}
                      width={240}
                    />
                    <TextInput
                      label="Scopes"
                      htmlName="oauthScopes"
                      value={oauthScopes}
                      onChange={setOauthScopes}
                      isOptional
                      width={240}
                    />
                  </HStack>
                </VStack>
              ) : null}

              <CheckboxInput
                label="Enabled"
                htmlName="enabled"
                value={enabled}
                onChange={setEnabled}
                description="Disabled MCPs are excluded from the gateway"
              />

              <Button type="submit" label="Add MCP" variant="primary" isLoading={processing} />
            </VStack>
          )}
        </Form>
      </Card>

      <VStack gap={3} hAlign="stretch">
        <Heading level={2}>Registered MCPs</Heading>
        {mcps.length === 0 ? (
          <Banner status="info" title="No MCPs yet" container="card" />
        ) : (
          <List density="compact" hasDividers>
            {mcps.map((mcp) => (
              <ListItem
                key={mcp.id}
                href={`/mcps/${mcp.id}`}
                label={mcp.name}
                description={`${mcp.transport === 'http' ? mcp.httpUrl : mcp.npmPackage} · ${mcp.authType}`}
                startContent={
                  <StatusDot variant={statusVariant(mcp.status)} label={mcp.status} />
                }
                endContent={
                  <HStack gap={2} vAlign="center">
                    {!mcp.enabled ? <Badge label="Disabled" variant="neutral" /> : null}
                    <Badge label={mcp.status} variant={statusVariant(mcp.status)} />
                  </HStack>
                }
              />
            ))}
          </List>
        )}
      </VStack>
    </VStack>
  )
}
