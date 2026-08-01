import { useState } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Badge } from '@astryxdesign/core/Badge'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { CheckboxList, CheckboxListItem } from '@astryxdesign/core/CheckboxList'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import {
  HStack,
  Layout,
  LayoutContent,
  LayoutFooter,
  StackItem,
  VStack,
} from '@astryxdesign/core/Layout'
import { RadioList, RadioListItem } from '@astryxdesign/core/RadioList'
import { Table, pixel, proportional, type TableColumn } from '@astryxdesign/core/Table'
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
} & Record<string, unknown>

type McpOption = {
  id: number
  name: string
  slug: string
  enabled: boolean
}

function tokenStatus(token: TokenRow): { label: string; variant: 'success' | 'warning' | 'neutral' } {
  if (token.revokedAt) return { label: 'Revoked', variant: 'neutral' }
  if (token.isUsable) return { label: 'Active', variant: 'success' }
  return { label: 'Expired', variant: 'warning' }
}

function scopeLabel(token: TokenRow) {
  if (token.scopeMode === 'all') return 'All MCPs'
  const count = token.mcpIds.length
  return `${count} MCP${count === 1 ? '' : 's'}`
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
  const [isCreateOpen, setIsCreateOpen] = useState(false)
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

  function openCreate() {
    setName('')
    setScopeMode('all')
    setSelectedMcpIds([])
    setExpiresAt('')
    setIsCreateOpen(true)
  }

  const columns: TableColumn<TokenRow>[] = [
    {
      key: 'name',
      header: 'Name',
      width: proportional(2),
      renderCell: (token) => (
        <Text type="body" weight="bold">
          {token.name}
        </Text>
      ),
    },
    {
      key: 'tokenPrefix',
      header: 'Identifier',
      width: proportional(2),
      renderCell: (token) => (
        <Text type="supporting" color="secondary">
          {token.tokenPrefix}…
        </Text>
      ),
    },
    {
      key: 'scopeMode',
      header: 'Scope',
      width: proportional(1),
      renderCell: (token) => (
        <Text type="supporting" color="secondary">
          {scopeLabel(token)}
        </Text>
      ),
    },
    {
      key: 'expiresAt',
      header: 'Expires',
      width: proportional(2),
      renderCell: (token) => (
        <Text type="supporting" color="secondary">
          {token.expiresAt ? new Date(token.expiresAt).toLocaleString() : 'No expiry'}
        </Text>
      ),
    },
    {
      key: 'status',
      header: 'Status',
      width: pixel(110),
      renderCell: (token) => {
        const status = tokenStatus(token)
        return <Badge label={status.label} variant={status.variant} />
      },
    },
    {
      key: 'actions',
      header: 'Actions',
      width: pixel(110),
      align: 'end',
      renderCell: (token) =>
        token.isUsable ? (
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
        ) : (
          <Text type="supporting" color="secondary">
            —
          </Text>
        ),
    },
  ]

  return (
    <VStack gap={6} maxWidth={960} width="100%">
      <HStack gap={4} hAlign="between" vAlign="start">
        <StackItem size="fill">
          <VStack gap={2}>
            <Heading level={1}>Access tokens</Heading>
            <Text type="body" color="secondary">
              Identifiers agents use to call the MyMCPs gateway. Scope a token to all MCPs (new ones
              included automatically) or to a selected list.
            </Text>
          </VStack>
        </StackItem>
        <Button label="Create token" variant="primary" onClick={openCreate} />
      </HStack>

      <Card padding={6} width="100%">
        <VStack gap={3} hAlign="stretch">
          <Heading level={2}>Gateway URL</Heading>
          <HStack gap={3} vAlign="center" wrap="wrap">
            <Text type="body">{gatewayUrl}</Text>
            <Button
              label="Copy URL"
              variant="secondary"
              size="sm"
              onClick={() => copyText(gatewayUrl)}
            />
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

      {tokens.length === 0 ? (
        <Banner
          status="info"
          title="No tokens yet"
          description="Create a token so agents can call the gateway."
          container="card"
        />
      ) : (
        <Table
          data={tokens}
          columns={columns}
          idKey="id"
          hasHover
          density="compact"
          textOverflow="truncate"
        />
      )}

      <Dialog
        isOpen={isCreateOpen}
        onOpenChange={setIsCreateOpen}
        purpose="form"
        width={520}
        maxHeight="85vh"
      >
        <Form route="tokens.store" className="dialog-form-fill">
          {({ errors, processing }) => (
            <Layout
              header={
                <DialogHeader
                  title="Create token"
                  subtitle="Issue an identifier for the /mcp gateway"
                  onOpenChange={setIsCreateOpen}
                />
              }
              content={
                <LayoutContent isScrollable>
                  <VStack gap={4} hAlign="stretch">
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
                      label="Expires at"
                      htmlName="expiresAt"
                      type="datetime-local"
                      value={expiresAt}
                      onChange={setExpiresAt}
                      isOptional
                      description="Leave empty for no expiration. Time is interpreted in your local timezone."
                      width="100%"
                    />
                  </VStack>
                </LayoutContent>
              }
              footer={
                <LayoutFooter>
                  <HStack gap={2} hAlign="end">
                    <Button
                      label="Cancel"
                      variant="secondary"
                      onClick={() => setIsCreateOpen(false)}
                    />
                    <Button
                      type="submit"
                      label="Create token"
                      variant="primary"
                      isLoading={processing}
                    />
                  </HStack>
                </LayoutFooter>
              }
            />
          )}
        </Form>
      </Dialog>
    </VStack>
  )
}
