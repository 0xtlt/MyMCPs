import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Head, router } from '@inertiajs/react'
import type { JSONDataTypes } from '@adonisjs/core/types/transformers'
import { Banner } from '@astryxdesign/core/Banner'
import { useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Button } from '@astryxdesign/core/Button'
import { CodeBlock } from '@astryxdesign/core/CodeBlock'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import { Grid } from '@astryxdesign/core/Grid'
import {
  HStack,
  Layout,
  LayoutContent,
  LayoutFooter,
  LayoutPanel,
  StackItem,
  VStack,
} from '@astryxdesign/core/Layout'
import { List, ListItem } from '@astryxdesign/core/List'
import { Pagination } from '@astryxdesign/core/Pagination'
import { Selector } from '@astryxdesign/core/Selector'
import {
  Table,
  pixel,
  proportional,
  type TableColumn,
  type TablePlugin,
} from '@astryxdesign/core/Table'
import { Heading, Text } from '@astryxdesign/core/Text'
import { Token } from '@astryxdesign/core/Token'
import { LiveRefreshButton } from '~/components/live_refresh_button'
import { browserTimeZone, formatLocalDateTime, formatTimeZoneLabel } from '~/components/local_time'

interface LogRow extends Record<string, JSONDataTypes> {
  id: number
  accessTokenId: number | null
  accessTokenName: string
  accessTokenPrefix: string
  callerIp: string | null
  mcpId: number | null
  mcpName: string | null
  mcpSlug: string | null
  requestedToolName: string
  toolName: string | null
  outcome: 'success' | 'error'
  errorCategory: string | null
  errorSummary: string | null
  arguments: string | null
  argumentsCaptured: boolean
  response: string | null
  responseCaptured: boolean
  durationMs: number
  createdAt: string
}

type Filters = {
  range: string
  outcome: string
  mcp: string
  token: string
  timeZone: string
}
type Option = { value: string; label: string }

function displayArguments(log: LogRow) {
  if (!log.argumentsCaptured) return null
  if (log.arguments === null) return ''
  try {
    return JSON.stringify(JSON.parse(log.arguments), null, 2)
  } catch {
    return log.arguments
  }
}

function displayResponse(log: LogRow) {
  if (!log.responseCaptured) return null
  if (log.response === null) return ''
  try {
    return JSON.stringify(JSON.parse(log.response), null, 2)
  } catch {
    return log.response
  }
}

function CallDetails({
  log,
  timeZone,
  onClose,
  showHeader = true,
}: {
  log: LogRow
  timeZone: string
  onClose: () => void
  showHeader?: boolean
}) {
  const capturedArguments = displayArguments(log)
  const capturedResponse = displayResponse(log)

  return (
    <VStack gap={4} hAlign="stretch">
      {showHeader ? (
        <HStack gap={3} hAlign="between" vAlign="center">
          <Heading level={2}>Call details</Heading>
          <HStack gap={2} vAlign="center">
            <Token
              label={log.outcome === 'success' ? 'Success' : 'Error'}
              color={log.outcome === 'success' ? 'green' : 'red'}
            />
            <Button label="Close" variant="ghost" size="sm" onClick={onClose} />
          </HStack>
        </HStack>
      ) : null}
      <VStack gap={2}>
        <Text type="label">Requested tool</Text>
        <Text type="body">{log.requestedToolName}</Text>
        <Text type="label">MCP</Text>
        <Text type="body">{log.mcpName ?? log.mcpSlug ?? 'Unknown'}</Text>
        <Text type="label">Access token</Text>
        <Text type="body">
          {log.accessTokenName} ({log.accessTokenPrefix}…)
        </Text>
        <Text type="label">Caller IP</Text>
        <Text type="body">{log.callerIp ?? 'Unknown'}</Text>
        <Text type="label">Started</Text>
        <Text type="body">{formatLocalDateTime(log.createdAt, timeZone)}</Text>
        <Text type="label">Duration</Text>
        <Text type="body">{log.durationMs} ms</Text>
      </VStack>
      {log.errorCategory ? (
        <Banner
          status="error"
          title={log.errorCategory.replaceAll('_', ' ')}
          description={log.errorSummary ?? 'No error summary available.'}
          container="section"
        />
      ) : null}
      {!log.argumentsCaptured ? (
        <Banner
          status="info"
          title="Arguments were not captured"
          description="The metadata logging level was active for this call."
          container="section"
        />
      ) : capturedArguments === '' ? (
        <Banner
          status="info"
          title="No arguments supplied"
          description="Argument capture was active, but this call did not include arguments."
          container="section"
        />
      ) : (
        <CodeBlock
          code={capturedArguments ?? ''}
          language="json"
          title="Arguments"
          width="100%"
          maxHeight={360}
          isWrapped
        />
      )}
      {!log.responseCaptured ? (
        <Banner
          status="info"
          title="Response was not captured"
          description="Response capture was not active for this call."
          container="section"
        />
      ) : capturedResponse === '' ? (
        <Banner
          status="info"
          title="No MCP response received"
          description="Response capture was active, but the upstream MCP did not return a result."
          container="section"
        />
      ) : (
        <CodeBlock
          code={capturedResponse ?? ''}
          language="json"
          title="Response"
          width="100%"
          maxHeight={480}
          isWrapped
        />
      )}
    </VStack>
  )
}

export default function LogsIndex({
  logs,
  selectedLog,
  pagination,
  filters,
  options,
  loggingLevel,
}: {
  logs: LogRow[]
  selectedLog: LogRow | null
  pagination: { page: number; pageSize: number; total: number; totalPages: number }
  filters: Filters
  options: { mcps: Option[]; tokens: Option[] }
  loggingLevel: 'off' | 'metadata' | 'arguments' | 'responses'
}) {
  const { isMobile } = useAppShellMobile()
  const [isFiltersOpen, setIsFiltersOpen] = useState(false)
  const pendingPageSize = useRef<number | null>(null)
  const navigate = useCallback(
    (changes: Record<string, string | number | undefined>) => {
      const next = {
        ...filters,
        page: pagination.page,
        pageSize: pagination.pageSize,
        ...changes,
      }
      router.get('/logs', next, { preserveState: true, replace: true })
    },
    [filters, pagination.page, pagination.pageSize]
  )

  useEffect(() => {
    const localTimeZone = browserTimeZone()
    if (!localTimeZone || localTimeZone === filters.timeZone) return

    navigate({ timeZone: localTimeZone })
  }, [filters.timeZone, navigate])

  const activeFilterCount = [filters.outcome, filters.mcp, filters.token].filter(Boolean).length

  const columns: TableColumn<LogRow>[] = [
    {
      key: 'createdAt',
      header: 'Time',
      width: proportional(2),
      renderCell: (log) => (
        <Text type="supporting" color="secondary">
          {formatLocalDateTime(log.createdAt, filters.timeZone)}
        </Text>
      ),
    },
    {
      key: 'mcpName',
      header: 'MCP',
      width: proportional(1),
      renderCell: (log) => <Text type="supporting">{log.mcpName ?? log.mcpSlug ?? '—'}</Text>,
    },
    {
      key: 'toolName',
      header: 'Tool',
      width: proportional(2),
      renderCell: (log) => <Text type="body">{log.toolName ?? log.requestedToolName}</Text>,
    },
    {
      key: 'accessTokenName',
      header: 'Token',
      width: proportional(1),
      renderCell: (log) => (
        <Text type="supporting" color="secondary">
          {log.accessTokenName}
        </Text>
      ),
    },
    {
      key: 'outcome',
      header: 'Outcome',
      width: pixel(110),
      renderCell: (log) => (
        <Token
          label={log.outcome === 'success' ? 'Success' : 'Error'}
          color={log.outcome === 'success' ? 'green' : 'red'}
        />
      ),
    },
    {
      key: 'durationMs',
      header: 'Duration',
      width: pixel(100),
      align: 'end',
      renderCell: (log) => <Text type="supporting">{log.durationMs} ms</Text>,
    },
  ]

  const rowNavigationPlugin = useMemo<TablePlugin<LogRow>>(
    () => ({
      transformBodyRow(props, log) {
        const isSelected = selectedLog?.id === log.id
        const selectLog = () => navigate({ logId: log.id })

        return {
          ...props,
          htmlProps: {
            ...props.htmlProps,
            'aria-selected': isSelected,
            'tabIndex': 0,
            'style': {
              ...props.htmlProps.style,
              cursor: 'pointer',
              backgroundColor: isSelected
                ? 'var(--color-accent-muted)'
                : props.htmlProps.style?.backgroundColor,
            },
            'onClick': (event) => {
              props.htmlProps.onClick?.(event)
              if (!event.defaultPrevented) selectLog()
            },
            'onKeyDown': (event) => {
              props.htmlProps.onKeyDown?.(event)
              if (!event.defaultPrevented && (event.key === 'Enter' || event.key === ' ')) {
                event.preventDefault()
                selectLog()
              }
            },
          },
          xstyle: props.xstyle,
        }
      },
    }),
    [navigate, selectedLog?.id]
  )

  return (
    <Fragment>
      <Layout
        contentWidth={1200}
        height={isMobile ? 'auto' : 'fill'}
        style={{ width: '100%' }}
        content={
          <LayoutContent padding={isMobile ? 0 : 6} label="MCP call logs">
            <Head title="MCP call logs" />
            <VStack gap={isMobile ? 4 : 6} width="100%">
              <HStack gap={4} hAlign="between" vAlign="start" wrap="wrap">
                <StackItem size="fill">
                  <VStack gap={2}>
                    <Heading level={1}>MCP call logs</Heading>
                    <Text type="body" color="secondary">
                      Inspect individual tool calls, failures, arguments, and timing.
                    </Text>
                  </VStack>
                </StackItem>
                <HStack gap={3} vAlign="center">
                  <LiveRefreshButton />
                  <Button label="View analytics" variant="secondary" href="/analytics" />
                </HStack>
              </HStack>

              {loggingLevel === 'off' ? (
                <Banner
                  status="warning"
                  title="Call logging is off"
                  description="Existing records remain available until retention removes them. Enable logging in Settings to capture new calls."
                  container="card"
                />
              ) : null}

              <VStack gap={isMobile ? 4 : 10} hAlign="stretch">
                {isMobile ? (
                  <VStack gap={3} hAlign="stretch">
                    <Selector
                      label="Time range"
                      value={filters.range}
                      options={[
                        { value: '24h', label: 'Last 24 hours' },
                        { value: '7d', label: 'Last 7 days' },
                        { value: '30d', label: 'Last 30 days' },
                        { value: 'all', label: 'All retained' },
                      ]}
                      onChange={(value) => navigate({ range: value, page: 1, logId: undefined })}
                      width="100%"
                    />
                    <HStack gap={2} wrap="wrap">
                      <Button
                        label={activeFilterCount ? `Filters (${activeFilterCount})` : 'Filters'}
                        variant="secondary"
                        onClick={() => setIsFiltersOpen(true)}
                      />
                      {activeFilterCount ? (
                        <Button
                          label="Clear filters"
                          variant="ghost"
                          onClick={() =>
                            navigate({ outcome: '', mcp: '', token: '', page: 1, logId: undefined })
                          }
                        />
                      ) : null}
                    </HStack>
                  </VStack>
                ) : (
                  <Grid gap={3} align="end" columns={{ minWidth: 180, repeat: 'fit' }}>
                    <Selector
                      label="Time range"
                      value={filters.range}
                      options={[
                        { value: '24h', label: 'Last 24 hours' },
                        { value: '7d', label: 'Last 7 days' },
                        { value: '30d', label: 'Last 30 days' },
                        { value: 'all', label: 'All retained' },
                      ]}
                      onChange={(value) => navigate({ range: value, page: 1, logId: undefined })}
                      width={180}
                    />
                    <Selector
                      label="Outcome"
                      value={filters.outcome || 'all'}
                      options={[
                        { value: 'all', label: 'All outcomes' },
                        { value: 'success', label: 'Success' },
                        { value: 'error', label: 'Error' },
                      ]}
                      onChange={(value) =>
                        navigate({ outcome: value === 'all' ? '' : value, page: 1 })
                      }
                      width={180}
                    />
                    <Selector
                      label="MCP"
                      value={filters.mcp || 'all'}
                      options={[{ value: 'all', label: 'All MCPs' }, ...options.mcps]}
                      onChange={(value) => navigate({ mcp: value === 'all' ? '' : value, page: 1 })}
                      width={200}
                    />
                    <Selector
                      label="Access token"
                      value={filters.token || 'all'}
                      options={[{ value: 'all', label: 'All tokens' }, ...options.tokens]}
                      onChange={(value) =>
                        navigate({ token: value === 'all' ? '' : value, page: 1 })
                      }
                      width={200}
                    />
                    <Button
                      label="Clear filters"
                      variant="secondary"
                      onClick={() => router.get('/logs', {}, { replace: true })}
                    />
                  </Grid>
                )}

                {logs.length === 0 ? (
                  <Banner
                    status="info"
                    title="No calls in this view"
                    description="Change the filters or make a tool call through the MCP gateway."
                    container="card"
                  />
                ) : isMobile ? (
                  <VStack gap={3} hAlign="stretch">
                    <List header="Tool calls" density="compact" hasDividers>
                      {logs.map((log) => (
                        <ListItem
                          key={log.id}
                          label={log.toolName ?? log.requestedToolName}
                          description={
                            <VStack gap={0}>
                              <Text type="supporting" color="secondary">
                                {log.mcpName ?? log.mcpSlug ?? 'Unknown MCP'} ·{' '}
                                {log.accessTokenName}
                              </Text>
                              <Text type="supporting" color="secondary">
                                {formatLocalDateTime(log.createdAt, filters.timeZone)}
                              </Text>
                            </VStack>
                          }
                          endContent={
                            <VStack gap={1} hAlign="end">
                              <Token
                                label={log.outcome === 'success' ? 'Success' : 'Error'}
                                color={log.outcome === 'success' ? 'green' : 'red'}
                              />
                              <Text type="supporting" color="secondary">
                                {log.durationMs} ms
                              </Text>
                            </VStack>
                          }
                          onClick={() => navigate({ logId: log.id })}
                          isSelected={selectedLog?.id === log.id}
                        />
                      ))}
                    </List>
                    {pagination.total > 0 ? (
                      <Pagination
                        page={pagination.page}
                        pageSize={pagination.pageSize}
                        totalItems={pagination.total}
                        variant="count"
                        size="sm"
                        pageSizeOptions={[10, 25, 50, 100]}
                        onPageSizeChange={(pageSize) => {
                          pendingPageSize.current = pageSize
                        }}
                        onChange={(page) => {
                          const pageSize = pendingPageSize.current ?? pagination.pageSize
                          pendingPageSize.current = null
                          navigate({ pageSize, page, logId: undefined })
                        }}
                      />
                    ) : null}
                    <Text type="supporting" color="secondary">
                      Times shown in {formatTimeZoneLabel(filters.timeZone)}.
                    </Text>
                  </VStack>
                ) : (
                  <VStack gap={3} hAlign="stretch">
                    <Table
                      data={logs}
                      columns={columns}
                      idKey="id"
                      density="compact"
                      hasHover
                      textOverflow="truncate"
                      plugins={{ rowNavigation: rowNavigationPlugin }}
                      rowIndexStart={(pagination.page - 1) * pagination.pageSize + 1}
                      rowCount={pagination.total}
                    />
                    {pagination.total > 0 ? (
                      <Pagination
                        page={pagination.page}
                        pageSize={pagination.pageSize}
                        totalItems={pagination.total}
                        variant="count"
                        size="sm"
                        pageSizeOptions={[10, 25, 50, 100]}
                        onPageSizeChange={(pageSize) => {
                          pendingPageSize.current = pageSize
                        }}
                        onChange={(page) => {
                          const pageSize = pendingPageSize.current ?? pagination.pageSize
                          pendingPageSize.current = null
                          navigate({ pageSize, page, logId: undefined })
                        }}
                      />
                    ) : null}
                    <Text type="supporting" color="secondary">
                      Times shown in {formatTimeZoneLabel(filters.timeZone)}.
                    </Text>
                  </VStack>
                )}
              </VStack>
            </VStack>
          </LayoutContent>
        }
        end={
          selectedLog && !isMobile ? (
            <LayoutPanel width={420} padding={5} hasDivider label="Call details">
              <CallDetails
                log={selectedLog}
                timeZone={filters.timeZone}
                onClose={() => navigate({ logId: undefined })}
              />
            </LayoutPanel>
          ) : undefined
        }
      />
      {isMobile && isFiltersOpen ? (
        <Dialog isOpen onOpenChange={setIsFiltersOpen} purpose="info" variant="fullscreen">
          <Layout
            header={<DialogHeader title="Filters" onOpenChange={setIsFiltersOpen} />}
            content={
              <LayoutContent isScrollable>
                <VStack gap={4} hAlign="stretch">
                  <Selector
                    label="Outcome"
                    value={filters.outcome || 'all'}
                    options={[
                      { value: 'all', label: 'All outcomes' },
                      { value: 'success', label: 'Success' },
                      { value: 'error', label: 'Error' },
                    ]}
                    onChange={(value) =>
                      navigate({ outcome: value === 'all' ? '' : value, page: 1 })
                    }
                    width="100%"
                  />
                  <Selector
                    label="MCP"
                    value={filters.mcp || 'all'}
                    options={[{ value: 'all', label: 'All MCPs' }, ...options.mcps]}
                    onChange={(value) => navigate({ mcp: value === 'all' ? '' : value, page: 1 })}
                    width="100%"
                  />
                  <Selector
                    label="Access token"
                    value={filters.token || 'all'}
                    options={[{ value: 'all', label: 'All tokens' }, ...options.tokens]}
                    onChange={(value) => navigate({ token: value === 'all' ? '' : value, page: 1 })}
                    width="100%"
                  />
                </VStack>
              </LayoutContent>
            }
            footer={
              <LayoutFooter>
                <HStack gap={2} hAlign="end" wrap="wrap">
                  {activeFilterCount ? (
                    <Button
                      label="Clear filters"
                      variant="secondary"
                      onClick={() =>
                        navigate({ outcome: '', mcp: '', token: '', page: 1, logId: undefined })
                      }
                    />
                  ) : null}
                  <Button label="Done" variant="primary" onClick={() => setIsFiltersOpen(false)} />
                </HStack>
              </LayoutFooter>
            }
          />
        </Dialog>
      ) : null}
      {selectedLog && isMobile ? (
        <Dialog
          isOpen
          onOpenChange={(open) => {
            if (!open) navigate({ logId: undefined })
          }}
          purpose="info"
          variant="fullscreen"
        >
          <Layout
            header={
              <DialogHeader
                title="Call details"
                endContent={
                  <Token
                    label={selectedLog.outcome === 'success' ? 'Success' : 'Error'}
                    color={selectedLog.outcome === 'success' ? 'green' : 'red'}
                  />
                }
                onOpenChange={(open) => {
                  if (!open) navigate({ logId: undefined })
                }}
              />
            }
            content={
              <LayoutContent isScrollable>
                <CallDetails
                  log={selectedLog}
                  timeZone={filters.timeZone}
                  onClose={() => navigate({ logId: undefined })}
                  showHeader={false}
                />
              </LayoutContent>
            }
          />
        </Dialog>
      ) : null}
    </Fragment>
  )
}
