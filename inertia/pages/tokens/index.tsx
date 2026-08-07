import { useEffect, useRef, useState, type SVGProps } from 'react'
import { Head, router } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { CheckboxList, CheckboxListItem } from '@astryxdesign/core/CheckboxList'
import { CodeBlock } from '@astryxdesign/core/CodeBlock'
import { DateTimeInput, type ISODateTimeString } from '@astryxdesign/core/DateTimeInput'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import { DropdownMenu } from '@astryxdesign/core/DropdownMenu'
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
import { List, ListItem } from '@astryxdesign/core/List'
import { RadioList, RadioListItem } from '@astryxdesign/core/RadioList'
import { Table, pixel, proportional, type TableColumn } from '@astryxdesign/core/Table'
import { Tab, TabList } from '@astryxdesign/core/TabList'
import { Switch } from '@astryxdesign/core/Switch'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'
import { Token } from '@astryxdesign/core/Token'
import { ToggleButton } from '@astryxdesign/core/ToggleButton'
import {
  createMcpInstallConfig,
  type McpClient,
  type McpInstallAuthMode,
} from '~/components/mcp_install_config'

type TokenRow = {
  id: number
  name: string
  tokenPrefix: string
  source: 'manual' | 'oauth'
  scopeMode: 'all' | 'selected'
  mcpIds: number[]
  expiresAt: string | null
  revokedAt: string | null
  lastUsedAt: string | null
  createdAt: string | null
  isUsable: boolean
  isActive: boolean
  canRevoke: boolean
  displayExpiresAt: string | null
  oauthClientName: string | null
}

type McpOption = {
  id: number
  name: string
  slug: string
  enabled: boolean
}

type TokenFormValues = {
  name: string
  scopeMode: 'all' | 'selected'
  selectedMcpIds: string[]
  expiresAt: ISODateTimeString | undefined
}

type TokenFormErrors = Partial<Record<'name' | 'scopeMode' | 'mcpIds' | 'expiresAt', string>>

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
  color: 'green' | 'orange' | 'gray'
} {
  if (token.revokedAt) return { label: 'Revoked', color: 'gray' }
  if (token.isActive) return { label: 'Active', color: 'green' }
  return { label: 'Expired', color: 'orange' }
}

function scopeLabel(token: TokenRow) {
  if (token.scopeMode === 'all') return 'All MCPs'
  const count = token.mcpIds.length
  return `${count} MCP${count === 1 ? '' : 's'}`
}

function emptyTokenFormValues(): TokenFormValues {
  return {
    name: '',
    scopeMode: 'all',
    selectedMcpIds: [],
    expiresAt: undefined,
  }
}

function toLocalDateTime(value: string | null): ISODateTimeString | undefined {
  if (!value) return undefined

  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return undefined

  const pad = (part: number) => String(part).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}` as ISODateTimeString
}

function tokenFormValuesFromRow(token: TokenRow): TokenFormValues {
  return {
    name: token.name,
    scopeMode: token.scopeMode,
    selectedMcpIds: token.mcpIds.map(String),
    expiresAt: toLocalDateTime(token.expiresAt),
  }
}

function TokenFormFields({
  values,
  onChange,
  errors,
  mcps,
}: {
  values: TokenFormValues
  onChange: (patch: Partial<TokenFormValues>) => void
  errors: TokenFormErrors
  mcps: McpOption[]
}) {
  return (
    <VStack gap={4} hAlign="stretch">
      <TextInput
        label="Name"
        htmlName="name"
        value={values.name}
        onChange={(name) => onChange({ name })}
        placeholder="Cursor agent"
        width="100%"
        status={errors.name ? { type: 'error', message: errors.name } : undefined}
      />

      <RadioList
        label="MCP access"
        htmlName="scopeMode"
        value={values.scopeMode}
        onChange={(scopeMode) => onChange({ scopeMode: scopeMode as TokenFormValues['scopeMode'] })}
        description="All includes every enabled MCP, including ones added later."
        status={errors.scopeMode ? { type: 'error', message: errors.scopeMode } : undefined}
      >
        <RadioListItem value="all" label="All MCPs" />
        <RadioListItem value="selected" label="Selected MCPs" />
      </RadioList>

      {values.scopeMode === 'selected' ? (
        <>
          {values.selectedMcpIds.map((id) => (
            <input key={id} type="hidden" name="mcpIds[]" value={id} />
          ))}
          {mcps.length === 0 ? (
            <Banner status="warning" title="Add an MCP first" container="card" />
          ) : (
            <CheckboxList
              label="MCPs"
              value={values.selectedMcpIds}
              onChange={(selectedMcpIds) => onChange({ selectedMcpIds })}
              hasDividers
              status={errors.mcpIds ? { type: 'error', message: errors.mcpIds } : undefined}
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
        value={values.expiresAt}
        onChange={(expiresAt) => onChange({ expiresAt })}
        isOptional
        hasClear
        description="Leave empty for no expiration. Time is interpreted in your local timezone."
        width="100%"
        status={errors.expiresAt ? { type: 'error', message: errors.expiresAt } : undefined}
      />
      <input type="hidden" name="expiresAt" value={values.expiresAt ?? ''} />
    </VStack>
  )
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
  const { isMobile } = useAppShellMobile()
  const [isCreateOpen, setIsCreateOpen] = useState(false)
  const [isInstallOpen, setIsInstallOpen] = useState(Boolean(createdPlaintext))
  const [installClient, setInstallClient] = useState<McpClient>('codex')
  const [installAuthMode, setInstallAuthMode] = useState<McpInstallAuthMode>(
    createdPlaintext ? 'token' : 'oauth'
  )
  const [installToken, setInstallToken] = useState(createdPlaintext ?? '')
  const [isInstallTokenVisible, setIsInstallTokenVisible] = useState(Boolean(createdPlaintext))
  const [isLazyToolModeEnabled, setIsLazyToolModeEnabled] = useState(false)
  const [createValues, setCreateValues] = useState<TokenFormValues>(emptyTokenFormValues)
  const [editingToken, setEditingToken] = useState<TokenRow | null>(null)
  const [editValues, setEditValues] = useState<TokenFormValues>(emptyTokenFormValues)
  const [gatewayCopyState, setGatewayCopyState] = useState<CopyState>('idle')
  const [tokenCopyState, setTokenCopyState] = useState<CopyState>('idle')
  const copyResetTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    return () => {
      if (copyResetTimer.current) clearTimeout(copyResetTimer.current)
    }
  }, [])

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
    setCreateValues(emptyTokenFormValues())
    setIsCreateOpen(true)
  }

  function openEdit(token: TokenRow) {
    setEditValues(tokenFormValuesFromRow(token))
    setEditingToken(token)
  }

  function closeEdit() {
    setEditingToken(null)
    setEditValues(emptyTokenFormValues())
  }

  function openInstall() {
    setInstallClient('codex')
    setInstallAuthMode('oauth')
    setInstallToken('')
    setIsInstallTokenVisible(false)
    setIsLazyToolModeEnabled(false)
    setIsInstallOpen(true)
  }

  function handleInstallOpenChange(isOpen: boolean) {
    setIsInstallOpen(isOpen)
    if (!isOpen) {
      setInstallToken('')
      setInstallAuthMode('oauth')
      setIsInstallTokenVisible(false)
      setIsLazyToolModeEnabled(false)
    }
  }

  const installConfig = createMcpInstallConfig(
    installClient,
    gatewayUrl ?? '<YOUR_GATEWAY_URL>',
    installToken || '<YOUR_ACCESS_TOKEN>',
    isLazyToolModeEnabled,
    installAuthMode
  )
  const canCopyInstallConfig = Boolean(gatewayUrl && (installAuthMode === 'oauth' || installToken))
  const installStatus = !gatewayUrl
    ? 'Configure APP_URL before copying this configuration.'
    : installAuthMode === 'token' && !installToken
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
      key: 'source',
      header: 'Type',
      width: pixel(100),
      renderCell: (token) => (
        <Token
          label={token.source === 'oauth' ? 'OAuth' : 'Manual'}
          color={token.source === 'oauth' ? 'blue' : 'gray'}
          size="sm"
        />
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
          {token.displayExpiresAt ? new Date(token.displayExpiresAt).toLocaleString() : 'No expiry'}
        </Text>
      ),
    },
    {
      key: 'status',
      header: 'Status',
      width: pixel(110),
      renderCell: (token) => {
        const status = tokenStatus(token)
        return <Token label={status.label} color={status.color} size="sm" />
      },
    },
    {
      key: 'actions',
      header: 'Actions',
      width: pixel(190),
      align: 'end',
      renderCell: (token) => {
        if (token.revokedAt) {
          return (
            <Text type="supporting" color="secondary">
              —
            </Text>
          )
        }

        return (
          <HStack gap={2} hAlign="end">
            {token.source === 'manual' ? (
              <Button label="Edit" variant="secondary" size="sm" onClick={() => openEdit(token)} />
            ) : null}
            {token.canRevoke ? (
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
        )
      },
    },
  ]

  return (
    <VStack gap={6} maxWidth={960} width="100%">
      <Head title="Access tokens" />
      <HStack gap={4} hAlign="between" vAlign="start" wrap="wrap">
        <StackItem size="fill">
          <VStack gap={2}>
            <Heading level={1}>Access tokens</Heading>
            <Text type="body" color="secondary">
              Manage manual access tokens and OAuth connections used by MCP clients. Revoking an
              OAuth connection stops both its access and refresh tokens.
            </Text>
          </VStack>
        </StackItem>
        <Button label="Create token" variant="primary" onClick={openCreate} />
      </HStack>

      <Card padding={6} width="100%">
        <VStack gap={3} hAlign="stretch">
          <Heading level={2}>Gateway URL</Heading>
          <HStack gap={3} vAlign="center" wrap="wrap">
            <Text type="body" className="gateway-url">
              {gatewayUrl ?? 'Configure APP_URL to reveal the gateway URL.'}
            </Text>
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
            MCP clients can sign in with OAuth. Manual integrations can still send Authorization:
            Bearer &lt;token&gt; on every request.
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
      ) : isMobile ? (
        <List header="Issued tokens" density="compact" hasDividers>
          {tokens.map((token) => {
            const status = tokenStatus(token)
            const actions = [
              ...(token.source === 'manual'
                ? [{ label: 'Edit', onClick: () => openEdit(token) }]
                : []),
              ...(token.canRevoke
                ? [
                    {
                      label: 'Revoke',
                      onClick: () =>
                        router.post(`/tokens/${token.id}/revoke`, undefined, {
                          preserveScroll: true,
                        }),
                    },
                  ]
                : []),
            ]

            return (
              <ListItem
                key={token.id}
                label={token.name}
                description={
                  <VStack gap={0}>
                    <Text type="supporting" color="secondary">
                      {token.source === 'oauth' ? 'OAuth' : 'Manual'} · {token.tokenPrefix}… ·{' '}
                      {scopeLabel(token)}
                    </Text>
                    <Text type="supporting" color="secondary">
                      {token.displayExpiresAt
                        ? `Expires ${new Date(token.displayExpiresAt).toLocaleDateString()}`
                        : 'No expiry'}
                    </Text>
                  </VStack>
                }
                endContent={
                  <VStack gap={1} hAlign="end">
                    <Token label={status.label} color={status.color} size="sm" />
                    {actions.length > 0 ? (
                      <DropdownMenu
                        button={{
                          label: `Actions for ${token.name}`,
                          children: 'Actions',
                          variant: 'secondary',
                          size: 'sm',
                        }}
                        items={actions}
                      />
                    ) : null}
                  </VStack>
                }
              />
            )
          })}
        </List>
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
                <TabList
                  value={installAuthMode}
                  onChange={(value) => setInstallAuthMode(value as McpInstallAuthMode)}
                  layout="fill"
                  hasDivider
                >
                  <Tab value="oauth" label="OAuth (recommended)" />
                  <Tab value="token" label="Access token" />
                </TabList>
                {installAuthMode === 'oauth' ? (
                  <Banner
                    status="info"
                    title="No token to copy"
                    description="Add the gateway URL to your MCP client. The client will open this instance in your browser so you can sign in and approve the connection."
                    container="card"
                  />
                ) : (
                  <VStack gap={3} hAlign="stretch">
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
                  </VStack>
                )}
                <Switch
                  label="Enable lazy tool mode"
                  description="Adds X-MyMCPs-Tool-Mode: lazy so clients discover tools on demand."
                  value={isLazyToolModeEnabled}
                  onChange={setIsLazyToolModeEnabled}
                  labelPosition="start"
                  labelSpacing="spread"
                  width="100%"
                />
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
          onSuccess={(page) => {
            setIsCreateOpen(false)

            const newToken = page.props.createdPlaintext
            if (typeof newToken !== 'string') return

            setInstallClient('codex')
            setInstallAuthMode('token')
            setInstallToken(newToken)
            setIsInstallTokenVisible(true)
            setIsInstallOpen(true)
          }}
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
                  <TokenFormFields
                    values={createValues}
                    onChange={(patch) => setCreateValues((current) => ({ ...current, ...patch }))}
                    errors={errors}
                    mcps={mcps}
                  />
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

      <Dialog
        isOpen={Boolean(editingToken)}
        onOpenChange={(open) => {
          if (!open) closeEdit()
        }}
        purpose="form"
        width={520}
        maxHeight="85vh"
      >
        {editingToken ? (
          <Form
            action={{ url: `/tokens/${editingToken.id}`, method: 'put' }}
            className="dialog-form-fill"
            onSuccess={closeEdit}
          >
            {({ errors, processing }) => (
              <Layout
                header={
                  <DialogHeader
                    title={`Edit ${editingToken.name}`}
                    subtitle={`${editingToken.tokenPrefix}…`}
                    onOpenChange={() => closeEdit()}
                  />
                }
                content={
                  <LayoutContent isScrollable>
                    <TokenFormFields
                      values={editValues}
                      onChange={(patch) => setEditValues((current) => ({ ...current, ...patch }))}
                      errors={errors}
                      mcps={mcps}
                    />
                  </LayoutContent>
                }
                footer={
                  <LayoutFooter>
                    <HStack gap={2} hAlign="end">
                      <Button label="Cancel" variant="secondary" onClick={closeEdit} />
                      <Button
                        type="submit"
                        label="Save changes"
                        variant="primary"
                        isLoading={processing}
                      />
                    </HStack>
                  </LayoutFooter>
                }
              />
            )}
          </Form>
        ) : null}
      </Dialog>
    </VStack>
  )
}
