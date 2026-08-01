import { useState } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Badge } from '@astryxdesign/core/Badge'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { CheckboxList, CheckboxListItem } from '@astryxdesign/core/CheckboxList'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { RadioList, RadioListItem } from '@astryxdesign/core/RadioList'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

type TokenRow = {
  id: number
  name: string
  tokenPrefix: string
  scopeMode: 'all' | 'selected'
  mcpIds: number[]
  expiresAt: string | null
  revokedAt: string | null
  lastUsedAt: string | null
  createdAt: string
  isUsable: boolean
}

type McpOption = {
  id: number
  name: string
  slug: string
  enabled: boolean
}

export default function TokensIndex({
  tokens,
  mcps,
  gatewayUrl,
  createdPlaintext,
}: {
  tokens: TokenRow[]
  mcps: McpOption[]
  gatewayUrl: string
  createdPlaintext: string | null
}) {
  const [name, setName] = useState('')
  const [scopeMode, setScopeMode] = useState('all')
  const [selectedMcpIds, setSelectedMcpIds] = useState<string[]>([])
  const [expiresAt, setExpiresAt] = useState('')

  async function copyText(value: string) {
    try {
      await navigator.clipboard.writeText(value)
    } catch {
      // ignore
    }
  }

  return (
    <VStack gap={6} maxWidth={840} width="100%">
      <VStack gap={2}>
        <Heading level={1}>Access tokens</Heading>
        <Text type="body" color="secondary">
          Identifiers agents use to call the MyMCPs gateway. Scope a token to all MCPs (new ones
          included automatically) or to a selected list.
        </Text>
      </VStack>

      <Card padding={6} width="100%">
        <VStack gap={3} hAlign="stretch">
          <Heading level={2}>Gateway URL</Heading>
          <HStack gap={3} vAlign="center" wrap="wrap">
            <Text type="body">{gatewayUrl}</Text>
            <Button label="Copy URL" variant="secondary" size="sm" onClick={() => copyText(gatewayUrl)} />
          </HStack>
          <Text type="supporting" color="secondary">
            Send Authorization: Bearer &lt;token&gt; on every request.
          </Text>
        </VStack>
      </Card>

      {createdPlaintext ? (
        <Banner
          status="success"
          title="Copy your new token now"
          description={createdPlaintext}
          container="card"
        />
      ) : null}

      <Card padding={6} width="100%">
        <Form route="tokens.store">
          {({ errors, processing }) => (
            <VStack gap={4} hAlign="stretch">
              <Heading level={2}>Create token</Heading>
              <TextInput
                label="Name"
                htmlName="name"
                value={name}
                onChange={setName}
                placeholder="Cursor agent"
                width="100%"
                status={errors.name ? { type: 'error', message: errors.name } : undefined}
              />

              <RadioList
                label="MCP access"
                htmlName="scopeMode"
                value={scopeMode}
                onChange={setScopeMode}
                description="All includes every enabled MCP, including ones added later."
              >
                <RadioListItem value="all" label="All MCPs" />
                <RadioListItem value="selected" label="Selected MCPs" />
              </RadioList>

              {scopeMode === 'selected' ? (
                <>
                  {selectedMcpIds.map((id) => (
                    <input key={id} type="hidden" name="mcpIds[]" value={id} />
                  ))}
                  {mcps.length === 0 ? (
                    <Banner status="warning" title="Add an MCP first" container="card" />
                  ) : (
                    <CheckboxList
                      label="MCPs"
                      value={selectedMcpIds}
                      onChange={setSelectedMcpIds}
                      hasDividers
                    >
                      {mcps.map((mcp) => (
                        <CheckboxListItem
                          key={mcp.id}
                          value={String(mcp.id)}
                          label={mcp.name}
                          description={mcp.enabled ? mcp.slug : `${mcp.slug} (disabled)`}
                        />
                      ))}
                    </CheckboxList>
                  )}
                </>
              ) : null}

              <TextInput
                label="Expires at (UTC)"
                htmlName="expiresAt"
                type="datetime-local"
                value={expiresAt}
                onChange={setExpiresAt}
                isOptional
                description="Leave empty for no expiration"
                width={320}
              />

              <Button
                type="submit"
                label="Create token"
                variant="primary"
                isLoading={processing}
              />
            </VStack>
          )}
        </Form>
      </Card>

      <VStack gap={3} hAlign="stretch">
        <Heading level={2}>Tokens</Heading>
        {tokens.length === 0 ? (
          <Banner status="info" title="No tokens yet" container="card" />
        ) : (
          tokens.map((token) => (
            <Card key={token.id} padding={4} width="100%">
              <HStack gap={4} hAlign="between" vAlign="center" wrap="wrap">
                <VStack gap={1}>
                  <Text type="body" weight="bold">
                    {token.name}
                  </Text>
                  <Text type="supporting" color="secondary">
                    {token.tokenPrefix}… ·{' '}
                    {token.scopeMode === 'all'
                      ? 'All MCPs'
                      : `${token.mcpIds.length} MCP${token.mcpIds.length === 1 ? '' : 's'}`}
                    {token.expiresAt
                      ? ` · expires ${new Date(token.expiresAt).toLocaleString()}`
                      : ' · no expiry'}
                  </Text>
                </VStack>
                <HStack gap={2} vAlign="center">
                  {token.revokedAt ? (
                    <Badge label="Revoked" variant="neutral" />
                  ) : token.isUsable ? (
                    <Badge label="Active" variant="success" />
                  ) : (
                    <Badge label="Expired" variant="warning" />
                  )}
                  {token.isUsable ? (
                    <Form route="tokens.revoke" routeParams={{ id: token.id }}>
                      {({ processing }) => (
                        <Button
                          type="submit"
                          label="Revoke"
                          variant="destructive"
                          size="sm"
                          isLoading={processing}
                        />
                      )}
                    </Form>
                  ) : null}
                </HStack>
              </HStack>
            </Card>
          ))
        )}
      </VStack>
    </VStack>
  )
}
