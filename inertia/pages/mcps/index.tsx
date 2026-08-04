import { useMemo, useState } from 'react'
import { Head, router } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Badge } from '@astryxdesign/core/Badge'
import { Button } from '@astryxdesign/core/Button'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import { DropdownMenu } from '@astryxdesign/core/DropdownMenu'
import { List, ListItem } from '@astryxdesign/core/List'
import {
  HStack,
  Layout,
  LayoutContent,
  LayoutFooter,
  StackItem,
  VStack,
} from '@astryxdesign/core/Layout'
import { StatusDot } from '@astryxdesign/core/StatusDot'
import { Table, pixel, proportional, type TableColumn } from '@astryxdesign/core/Table'
import { Heading, Text } from '@astryxdesign/core/Text'
import {
  emptyMcpFormValues,
  mcpFormValuesFromRow,
  McpFormFields,
  type McpFormValues,
} from '~/components/mcp_form_fields'

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
  npmEnv: Array<{ name: string; hasValue: boolean }>
  authType: 'auto' | 'bearer' | 'header'
  authHeaderName: string | null
  hasAuthBearer: boolean
  hasAuthHeaderValue: boolean
  hasOauthAccessToken: boolean
  oauthRequired: boolean
  status: 'draft' | 'ready' | 'error'
  lastError: string | null
  enabled: boolean
}

type EditingOverride = {
  sourceId: number | null
  editingId: number | null
}

function statusVariant(status: McpRow['status']): 'success' | 'warning' | 'error' | 'neutral' {
  if (status === 'ready') return 'success'
  if (status === 'error') return 'error'
  if (status === 'draft') return 'warning'
  return 'neutral'
}

function endpointLabel(mcp: McpRow) {
  return mcp.transport === 'http' ? mcp.httpUrl || '—' : mcp.npmPackage || '—'
}

export default function McpsIndex({
  mcps,
  editingMcpId = null,
  appUrlConfigured,
}: {
  mcps: McpRow[]
  editingMcpId?: number | null
  appUrlConfigured: boolean
}) {
  const { isMobile } = useAppShellMobile()
  const [isCreateOpen, setIsCreateOpen] = useState(false)
  const [editingOverride, setEditingOverride] = useState<EditingOverride | null>(null)
  const [createValues, setCreateValues] = useState<McpFormValues>(emptyMcpFormValues)
  const [editValuesOverride, setEditValuesOverride] = useState<McpFormValues | null>(null)

  const editingId =
    editingOverride?.sourceId === editingMcpId ? editingOverride.editingId : editingMcpId

  const editingMcp = useMemo(
    () => mcps.find((mcp) => mcp.id === editingId) ?? null,
    [mcps, editingId]
  )

  const defaultEditValues = useMemo(
    () => (editingMcp ? mcpFormValuesFromRow(editingMcp) : emptyMcpFormValues()),
    [editingMcp]
  )
  const editValues = editValuesOverride ?? defaultEditValues

  function openCreate() {
    setCreateValues(emptyMcpFormValues())
    setIsCreateOpen(true)
  }

  function openEdit(mcp: McpRow) {
    setEditValuesOverride(mcpFormValuesFromRow(mcp))
    setEditingOverride({ sourceId: editingMcpId, editingId: mcp.id })
  }

  function closeEdit() {
    setEditValuesOverride(null)
    setEditingOverride({ sourceId: editingMcpId, editingId: null })
  }

  const columns: TableColumn<McpRow>[] = [
    {
      key: 'status',
      header: 'Status',
      width: pixel(110),
      renderCell: (mcp) => <StatusDot variant={statusVariant(mcp.status)} label={mcp.status} />,
    },
    {
      key: 'name',
      header: 'Name',
      width: proportional(2),
      renderCell: (mcp) => (
        <VStack gap={0}>
          <Text type="body" weight="bold">
            {mcp.name}
          </Text>
          <Text type="supporting" color="secondary">
            {mcp.slug}
          </Text>
        </VStack>
      ),
    },
    {
      key: 'endpoint',
      header: 'Endpoint',
      width: proportional(3),
      renderCell: (mcp) => (
        <Text type="supporting" color="secondary">
          {endpointLabel(mcp)}
        </Text>
      ),
    },
    {
      key: 'authType',
      header: 'Auth',
      width: pixel(100),
      renderCell: (mcp) => (
        <Text type="supporting" color="secondary">
          {mcp.authType}
        </Text>
      ),
    },
    {
      key: 'enabled',
      header: 'Enabled',
      width: pixel(100),
      renderCell: (mcp) =>
        mcp.enabled ? (
          <Badge label="On" variant="success" />
        ) : (
          <Badge label="Off" variant="neutral" />
        ),
    },
    {
      key: 'actions',
      header: 'Actions',
      width: pixel(100),
      align: 'end',
      renderCell: (mcp) => (
        <Button label="Edit" variant="secondary" size="sm" onClick={() => openEdit(mcp)} />
      ),
    },
  ]

  return (
    <VStack gap={6} maxWidth={960} width="100%">
      <Head title="MCPs" />
      <HStack gap={4} hAlign="between" vAlign="start" wrap="wrap">
        <StackItem size="fill">
          <VStack gap={2}>
            <Heading level={1}>MCPs</Heading>
            <Text type="body" color="secondary">
              Register upstream MCP servers. Agents reach them through MyMCPs with an access token.
            </Text>
          </VStack>
        </StackItem>
        <Button label="Add MCP" variant="primary" onClick={openCreate} />
      </HStack>

      {mcps.length === 0 ? (
        <Banner
          status="info"
          title="No MCPs yet"
          description="Create an MCP to start routing agent traffic."
          container="card"
        />
      ) : isMobile ? (
        <List header="Registered MCPs" density="compact" hasDividers>
          {mcps.map((mcp) => (
            <ListItem
              key={mcp.id}
              label={mcp.name}
              startContent={<StatusDot variant={statusVariant(mcp.status)} label={mcp.status} />}
              description={
                <VStack gap={0}>
                  <Text type="supporting" color="secondary">
                    {mcp.slug} · {mcp.transport}
                  </Text>
                  <Text type="supporting" color="secondary" className="mcp-endpoint">
                    {endpointLabel(mcp)}
                  </Text>
                </VStack>
              }
              endContent={
                <DropdownMenu
                  button={{
                    label: `Actions for ${mcp.name}`,
                    children: 'Actions',
                    variant: 'secondary',
                    size: 'sm',
                  }}
                  items={[{ label: 'Edit MCP', onClick: () => openEdit(mcp) }]}
                />
              }
            />
          ))}
        </List>
      ) : (
        <Table
          data={mcps}
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
        width={640}
        maxHeight="85vh"
      >
        <Form
          route="mcps.store"
          className="dialog-form-fill"
          onSuccess={() => setIsCreateOpen(false)}
        >
          {({ errors, processing }) => (
            <Layout
              header={
                <DialogHeader
                  title="Add MCP"
                  subtitle="Register an HTTP or npm upstream server"
                  onOpenChange={setIsCreateOpen}
                />
              }
              content={
                <LayoutContent isScrollable>
                  <McpFormFields
                    values={createValues}
                    onChange={(patch) => setCreateValues((current) => ({ ...current, ...patch }))}
                    errors={errors}
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
                      label="Add MCP"
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
        isOpen={Boolean(editingMcp)}
        onOpenChange={(open) => {
          if (!open) closeEdit()
        }}
        purpose="form"
        width={640}
        maxHeight="85vh"
      >
        {editingMcp ? (
          <Form
            route="mcps.update"
            routeParams={{ id: editingMcp.id }}
            className="dialog-form-fill"
            onSuccess={closeEdit}
          >
            {({ errors, processing }) => (
              <Layout
                header={
                  <DialogHeader
                    title={`Edit ${editingMcp.name}`}
                    subtitle={editingMcp.slug}
                    onOpenChange={() => closeEdit()}
                  />
                }
                content={
                  <LayoutContent isScrollable>
                    <VStack gap={4} hAlign="stretch">
                      {editingMcp.oauthRequired ? (
                        <Banner
                          status="warning"
                          title="Authorization required"
                          description="This MCP requires OAuth. Connect your account to finish setup."
                          container="card"
                          endContent={
                            <Button
                              label={editingMcp.hasOauthAccessToken ? 'Re-authorize' : 'Connect'}
                              variant="secondary"
                              size="sm"
                              isDisabled={!appUrlConfigured}
                              tooltip={
                                !appUrlConfigured ? 'Set APP_URL to connect with OAuth' : undefined
                              }
                              onClick={() =>
                                window.location.assign(`/mcps/${editingMcp.id}/oauth/start`)
                              }
                            />
                          }
                        />
                      ) : editingMcp.lastError ? (
                        <Banner
                          status="error"
                          title="Last connection error"
                          description={editingMcp.lastError}
                          container="card"
                        />
                      ) : null}
                      <HStack gap={2} wrap="wrap">
                        <Button
                          label="Test connection"
                          variant="secondary"
                          size="sm"
                          onClick={() =>
                            router.post(`/mcps/${editingMcp.id}/probe`, undefined, {
                              preserveScroll: true,
                            })
                          }
                        />
                        {editingMcp.authType === 'auto' &&
                        editingMcp.hasOauthAccessToken &&
                        !editingMcp.oauthRequired ? (
                          <Button
                            label="Re-authorize"
                            variant="secondary"
                            size="sm"
                            isDisabled={!appUrlConfigured}
                            tooltip={
                              !appUrlConfigured ? 'Set APP_URL to connect with OAuth' : undefined
                            }
                            onClick={() =>
                              window.location.assign(`/mcps/${editingMcp.id}/oauth/start`)
                            }
                          />
                        ) : null}
                      </HStack>
                      <McpFormFields
                        values={editValues}
                        onChange={(patch) =>
                          setEditValuesOverride((current) => ({
                            ...(current ?? editValues),
                            ...patch,
                          }))
                        }
                        errors={errors}
                        secrets={{
                          hasAuthBearer: editingMcp.hasAuthBearer,
                          hasAuthHeaderValue: editingMcp.hasAuthHeaderValue,
                        }}
                      />
                    </VStack>
                  </LayoutContent>
                }
                footer={
                  <LayoutFooter>
                    <HStack gap={2} hAlign="between" wrap="wrap">
                      <Button
                        label="Delete"
                        variant="destructive"
                        onClick={() =>
                          router.delete(`/mcps/${editingMcp.id}`, {
                            preserveScroll: true,
                            onSuccess: closeEdit,
                          })
                        }
                      />
                      <HStack gap={2}>
                        <Button label="Cancel" variant="secondary" onClick={closeEdit} />
                        <Button
                          type="submit"
                          label="Save changes"
                          variant="primary"
                          isLoading={processing}
                        />
                      </HStack>
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
