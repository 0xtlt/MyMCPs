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
import { Switch } from '@astryxdesign/core/Switch'
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
  const [isCreateOpen, setIsCreateOpen] = useState(false)
  const [isInstallOpen, setIsInstallOpen] = useState(Boolean(createdPlaintext))
  const [installClient, setInstallClient] = useState<McpClient>('codex')
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
    setInstallToken('')
    setIsInstallTokenVisible(false)
    setIsLazyToolModeEnabled(false)
    setIsInstallOpen(true)
  }

  function handleInstallOpenChange(isOpen: boolean) {
    setIsInstallOpen(isOpen)
    if (!isOpen) {
      setInstallToken('')
      setIsInstallTokenVisible(false)
      setIsLazyToolModeEnabled(false)
    }
  }

  const installConfig = createMcpInstallConfig(
    installClient,
    gatewayUrl ?? '<YOUR_GATEWAY_URL>',
    installToken || '<YOUR_ACCESS_TOKEN>',
    isLazyToolModeEnabled
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
            <Button label="Edit" variant="secondary" size="sm" onClick={() => openEdit(token)} />
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
        )
      },
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
