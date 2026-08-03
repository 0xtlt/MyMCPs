import { useEffect, useRef, useState, type SVGProps } from 'react'
import { Head } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Badge } from '@astryxdesign/core/Badge'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { CheckboxList, CheckboxListItem } from '@astryxdesign/core/CheckboxList'
import { CodeBlock } from '@astryxdesign/core/CodeBlock'
import { DateTimeInput, type ISODateTimeString } from '@astryxdesign/core/DateTimeInput'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import { Icon } from '@astryxdesign/core/Icon'
import { InputGroup } from '@astryxdesign/core/InputGroup'
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
import { Tab, TabList } from '@astryxdesign/core/TabList'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'
import { Token } from '@astryxdesign/core/Token'
import { ToggleButton } from '@astryxdesign/core/ToggleButton'
import { createMcpInstallConfig, type McpClient } from '~/components/mcp_install_config'

type TokenRow = {
  id: number
  name: string
  tokenPrefix: string
  scopeMode: 'all' | 'selected'
  mcpIds: number[]
  expiresAt: string | null
  revokedAt: string | null
  lastUsedAt: string | null
  createdAt: string | null
  isUsable: boolean
}

type McpOption = {
  id: number
  name: string
  slug: string
  enabled: boolean
}

type CopyState = 'idle' | 'copied' | 'error'

function EyeIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <svg
      {...props}
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M2.25 12s3.75-6.75 9.75-6.75S21.75 12 21.75 12 18 18.75 12 18.75 2.25 12 2.25 12Z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  )
}

function tokenStatus(token: TokenRow): {
  label: string
  variant: 'success' | 'warning' | 'neutral'
} {
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
  gatewayUrl: string | null
  createdPlaintext: string | null
}) {
  const [isCreateOpen, setIsCreateOpen] = useState(false)
  const [isInstallOpen, setIsInstallOpen] = useState(Boolean(createdPlaintext))
  const [installClient, setInstallClient] = useState<McpClient>('codex')
  const [installToken, setInstallToken] = useState(createdPlaintext ?? '')
  const [isInstallTokenVisible, setIsInstallTokenVisible] = useState(Boolean(createdPlaintext))
  const [name, setName] = useState('')
  const [scopeMode, setScopeMode] = useState('all')
  const [selectedMcpIds, setSelectedMcpIds] = useState<string[]>([])
  const [expiresAt, setExpiresAt] = useState<ISODateTimeString>()
  const [gatewayCopyState, setGatewayCopyState] = useState<CopyState>('idle')
  const [tokenCopyState, setTokenCopyState] = useState<CopyState>('idle')
  const copyResetTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    return () => {
      if (copyResetTimer.current) clearTimeout(copyResetTimer.current)
    }
  }, [])

  useEffect(() => {
    if (!createdPlaintext) return

    setInstallClient('codex')
    setInstallToken(createdPlaintext)
    setIsInstallTokenVisible(true)
    setIsInstallOpen(true)
  }, [createdPlaintext])

  async function copyText(value: string, setState: (state: CopyState) => void) {
    if (copyResetTimer.current) clearTimeout(copyResetTimer.current)
    setGatewayCopyState('idle')
    setTokenCopyState('idle')

    try {
      await navigator.clipboard.writeText(value)
      setState('copied')
    } catch {
      setState('error')
    }

    copyResetTimer.current = setTimeout(() => setState('idle'), 2000)
  }

  function openCreate() {
    setName('')
    setScopeMode('all')
    setSelectedMcpIds([])
    setExpiresAt(undefined)
    setIsCreateOpen(true)
  }

  function openInstall() {
    setInstallClient('codex')
    setInstallToken('')
    setIsInstallTokenVisible(false)
    setIsInstallOpen(true)
  }

  function handleInstallOpenChange(isOpen: boolean) {
    setIsInstallOpen(isOpen)
    if (!isOpen) {
      setInstallToken('')
      setIsInstallTokenVisible(false)
    }
  }

  const installConfig = createMcpInstallConfig(
    installClient,
    gatewayUrl ?? '<YOUR_GATEWAY_URL>',
    installToken || '<YOUR_ACCESS_TOKEN>'
  )
  const canCopyInstallConfig = Boolean(gatewayUrl && installToken)
  const installStatus = !gatewayUrl
    ? 'Configure APP_URL before copying this configuration.'
    : !installToken
      ? 'Paste an access token to enable copying.'
      : null

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
      <Head title="Access tokens" />
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
            <Text type="body">{gatewayUrl ?? 'Configure APP_URL to reveal the gateway URL.'}</Text>
            <Button
              label={
                gatewayCopyState === 'copied'
                  ? 'Copied!'
                  : gatewayCopyState === 'error'
                    ? 'Copy failed'
                    : 'Copy URL'
              }
              variant={gatewayCopyState === 'copied' ? 'primary' : 'secondary'}
              size="sm"
              isDisabled={!gatewayUrl}
              tooltip={!gatewayUrl ? 'Set APP_URL to enable public links' : undefined}
              clickAction={gatewayUrl ? () => copyText(gatewayUrl, setGatewayCopyState) : undefined}
            />
            <Button label="Install MCP" variant="primary" size="sm" onClick={openInstall} />
          </HStack>
          <Text type="supporting" color="secondary">
            Send Authorization: Bearer &lt;token&gt; on every request.
          </Text>
          <Banner
            status="info"
            title="Reduce tool-definition overhead"
            description="Optional: configure your MCP client to send X-MyMCPs-Tool-Mode: lazy. The gateway will announce available MCPs first and expose list_mcps, tool_search, and call_tool instead of loading every upstream tool definition. Without the header, eager mode remains active."
            container="card"
          />
        </VStack>
      </Card>

      {createdPlaintext ? (
        <Banner
          status="success"
          title={
            tokenCopyState === 'copied'
              ? 'Token copied'
              : tokenCopyState === 'error'
                ? 'Copy failed — click to retry'
                : 'Click your new token to copy it'
          }
          description={
            <Token
              label={createdPlaintext}
              color={
                tokenCopyState === 'copied' ? 'green' : tokenCopyState === 'error' ? 'red' : 'gray'
              }
              onClick={() => copyText(createdPlaintext, setTokenCopyState)}
              description="Copy the new access token to the clipboard"
            />
          }
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
        isOpen={isInstallOpen}
        onOpenChange={handleInstallOpenChange}
        purpose="info"
        width={680}
        maxHeight="85vh"
      >
        <Layout
          header={
            <DialogHeader
              title="Install MyMCPs"
              subtitle="Connect this gateway to your MCP client"
              onOpenChange={handleInstallOpenChange}
            />
          }
          content={
            <LayoutContent isScrollable>
              <VStack gap={4} hAlign="stretch">
                <Banner
                  status="warning"
                  title="Your token will be stored in plaintext"
                  description="These quick-install configurations include the access token directly. Keep the configuration private and revoke the token immediately if it is exposed."
                  container="card"
                />
                <InputGroup
                  label="Access token"
                  description="This value stays in this browser tab and is cleared when you close the modal."
                >
                  <TextInput
                    type={isInstallTokenVisible ? 'text' : 'password'}
                    label="Access token"
                    isLabelHidden
                    value={installToken}
                    onChange={setInstallToken}
                    placeholder="Paste your MyMCPs access token"
                    hasClear
                  />
                  <ToggleButton
                    label={isInstallTokenVisible ? 'Hide access token' : 'Show access token'}
                    tooltip={isInstallTokenVisible ? 'Hide access token' : 'Show access token'}
                    icon={<Icon icon={EyeIcon} />}
                    pressedIcon={<Icon icon="eyeSlash" />}
                    isPressed={isInstallTokenVisible}
                    onPressedChange={setIsInstallTokenVisible}
                    isIconOnly
                    style={{
                      borderWidth: 'var(--border-width)',
                      borderStyle: 'solid',
                      borderColor: 'var(--color-border)',
                      borderStartStartRadius: 'var(--radius-none)',
                      borderEndStartRadius: 'var(--radius-none)',
                      marginInlineStart: 'calc(-1 * var(--border-width))',
                    }}
                  />
                </InputGroup>
                <TabList
                  value={installClient}
                  onChange={(value) => setInstallClient(value as McpClient)}
                  layout="fill"
                  hasDivider
                >
                  <Tab value="codex" label="Codex" />
                  <Tab value="claude" label="Claude" />
                  <Tab value="cursor" label="Cursor" />
                </TabList>
                <VStack gap={3} hAlign="stretch">
                  <CodeBlock
                    code={installConfig.code}
                    language={installConfig.language}
                    title={installConfig.title}
                    width="100%"
                    size="sm"
                    hasCopyButton={canCopyInstallConfig}
                  />
                  {installStatus ? (
                    <Banner status="info" title={installStatus} container="card" />
                  ) : null}
                  <Text type="body">{installConfig.restartInstruction}</Text>
                  <Text type="supporting" color="secondary">
                    {installConfig.verifyInstruction}
                  </Text>
                </VStack>
              </VStack>
            </LayoutContent>
          }
          footer={
            <LayoutFooter>
              <HStack gap={2} hAlign="end">
                <Button
                  label="Close"
                  variant="secondary"
                  onClick={() => handleInstallOpenChange(false)}
                />
              </HStack>
            </LayoutFooter>
          }
        />
      </Dialog>

      <Dialog
        isOpen={isCreateOpen}
        onOpenChange={setIsCreateOpen}
        purpose="form"
        width={520}
        maxHeight="85vh"
      >
        <Form
          route="tokens.store"
          className="dialog-form-fill"
          onSuccess={() => setIsCreateOpen(false)}
        >
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

                    <DateTimeInput
                      label="Expires at"
                      value={expiresAt}
                      onChange={setExpiresAt}
                      isOptional
                      description="Leave empty for no expiration. Time is interpreted in your local timezone."
                      width="100%"
                    />
                    <input type="hidden" name="expiresAt" value={expiresAt ?? ''} />
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
