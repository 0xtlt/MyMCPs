import { useState } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Badge } from '@astryxdesign/core/Badge'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { CheckboxInput } from '@astryxdesign/core/CheckboxInput'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { RadioList, RadioListItem } from '@astryxdesign/core/RadioList'
import { StatusDot } from '@astryxdesign/core/StatusDot'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

type McpDetail = {
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
  authHeaderName: string | null
  hasAuthBearer: boolean
  hasAuthHeaderValue: boolean
  oauthAuthorizeUrl: string | null
  oauthTokenUrl: string | null
  oauthScopes: string | null
  oauthClientId: string | null
  hasOauthClientSecret: boolean
  hasOauthAccessToken: boolean
  status: 'draft' | 'ready' | 'error'
  lastError: string | null
  enabled: boolean
}

function statusVariant(status: McpDetail['status']): 'success' | 'warning' | 'error' | 'neutral' {
  if (status === 'ready') return 'success'
  if (status === 'error') return 'error'
  if (status === 'draft') return 'warning'
  return 'neutral'
}

export default function McpShow({ mcp }: { mcp: McpDetail }) {
  const [name, setName] = useState(mcp.name)
  const [description, setDescription] = useState(mcp.description ?? '')
  const [transport, setTransport] = useState(mcp.transport)
  const [httpUrl, setHttpUrl] = useState(mcp.httpUrl ?? '')
  const [npmPackage, setNpmPackage] = useState(mcp.npmPackage ?? '')
  const [npmVersion, setNpmVersion] = useState(mcp.npmVersion ?? '')
  const [npmArgs, setNpmArgs] = useState(mcp.npmArgs)
  const [authType, setAuthType] = useState(mcp.authType)
  const [authBearer, setAuthBearer] = useState('')
  const [authHeaderName, setAuthHeaderName] = useState(mcp.authHeaderName ?? '')
  const [authHeaderValue, setAuthHeaderValue] = useState('')
  const [oauthAuthorizeUrl, setOauthAuthorizeUrl] = useState(mcp.oauthAuthorizeUrl ?? '')
  const [oauthTokenUrl, setOauthTokenUrl] = useState(mcp.oauthTokenUrl ?? '')
  const [oauthScopes, setOauthScopes] = useState(mcp.oauthScopes ?? '')
  const [oauthClientId, setOauthClientId] = useState(mcp.oauthClientId ?? '')
  const [oauthClientSecret, setOauthClientSecret] = useState('')
  const [enabled, setEnabled] = useState(mcp.enabled)

  return (
    <VStack gap={6} maxWidth={840} width="100%">
      <VStack gap={2}>
        <HStack gap={2} vAlign="center">
          <Button label="Back" variant="ghost" size="sm" href="/mcps" />
        </HStack>
        <HStack gap={3} vAlign="center" wrap="wrap">
          <Heading level={1}>{mcp.name}</Heading>
          <StatusDot variant={statusVariant(mcp.status)} label={mcp.status} />
          <Badge label={mcp.status} variant={statusVariant(mcp.status)} />
          <Badge label={mcp.slug} variant="neutral" />
        </HStack>
        <Text type="body" color="secondary">
          {mcp.description || 'No description'}
        </Text>
      </VStack>

      {mcp.lastError ? (
        <Banner status="error" title="Last connection error" description={mcp.lastError} container="card" />
      ) : null}

      <Card padding={6} width="100%">
        <HStack gap={3} wrap="wrap">
          <Form route="mcps.probe" routeParams={{ id: mcp.id }}>
            {({ processing }) => (
              <Button type="submit" label="Test connection" variant="secondary" isLoading={processing} />
            )}
          </Form>
          {mcp.authType === 'oauth' ? (
            <Button
              label={mcp.hasOauthAccessToken ? 'Re-authorize OAuth' : 'Connect OAuth'}
              variant="primary"
              href={`/mcps/${mcp.id}/oauth/start`}
            />
          ) : null}
          <Form route="mcps.destroy" routeParams={{ id: mcp.id }} method="delete">
            {({ processing }) => (
              <Button type="submit" label="Delete" variant="destructive" isLoading={processing} />
            )}
          </Form>
        </HStack>
      </Card>

      <Card padding={6} width="100%">
        <Form route="mcps.update" routeParams={{ id: mcp.id }} method="put">
          {({ errors, processing }) => (
            <VStack gap={4} hAlign="stretch">
              <Heading level={2}>Configuration</Heading>

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
                onChange={(value) => setTransport(value as 'http' | 'npm')}
                orientation="horizontal"
              >
                <RadioListItem value="http" label="HTTP" />
                <RadioListItem value="npm" label="npm package" />
              </RadioList>

              {transport === 'http' ? (
                <TextInput
                  label="HTTP URL"
                  htmlName="httpUrl"
                  value={httpUrl}
                  onChange={setHttpUrl}
                  width="100%"
                />
              ) : (
                <VStack gap={3} hAlign="stretch">
                  <TextInput
                    label="npm package"
                    htmlName="npmPackage"
                    value={npmPackage}
                    onChange={setNpmPackage}
                    width="100%"
                  />
                  <HStack gap={3} wrap="wrap">
                    <TextInput
                      label="Version"
                      htmlName="npmVersion"
                      value={npmVersion}
                      onChange={setNpmVersion}
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
                onChange={(value) => setAuthType(value as McpDetail['authType'])}
              >
                <RadioListItem value="none" label="None" />
                <RadioListItem value="bearer" label="Bearer token" />
                <RadioListItem value="header" label="Custom header" />
                <RadioListItem value="oauth" label="OAuth" />
              </RadioList>

              {authType === 'bearer' ? (
                <TextInput
                  label={mcp.hasAuthBearer ? 'Bearer token (leave blank to keep)' : 'Bearer token'}
                  htmlName="authBearer"
                  type="password"
                  value={authBearer}
                  onChange={setAuthBearer}
                  width="100%"
                  isOptional={mcp.hasAuthBearer}
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
                    label={
                      mcp.hasAuthHeaderValue
                        ? 'Header value (leave blank to keep)'
                        : 'Header value'
                    }
                    htmlName="authHeaderValue"
                    type="password"
                    value={authHeaderValue}
                    onChange={setAuthHeaderValue}
                    width={320}
                    isOptional={mcp.hasAuthHeaderValue}
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
                      label={
                        mcp.hasOauthClientSecret
                          ? 'Client secret (leave blank to keep)'
                          : 'Client secret'
                      }
                      htmlName="oauthClientSecret"
                      type="password"
                      value={oauthClientSecret}
                      onChange={setOauthClientSecret}
                      width={240}
                      isOptional={mcp.hasOauthClientSecret}
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
                  <Text type="supporting" color="secondary">
                    OAuth access token:{' '}
                    {mcp.hasOauthAccessToken ? 'connected' : 'not connected yet'}
                  </Text>
                </VStack>
              ) : null}

              <CheckboxInput
                label="Enabled"
                htmlName="enabled"
                value={enabled}
                onChange={setEnabled}
              />

              <Button type="submit" label="Save changes" variant="primary" isLoading={processing} />
            </VStack>
          )}
        </Form>
      </Card>
    </VStack>
  )
}
