import { useCallback, useMemo } from 'react'
import { router } from '@inertiajs/react'
import type { JSONDataTypes } from '@adonisjs/core/types/transformers'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { CodeBlock } from '@astryxdesign/core/CodeBlock'
import { Grid } from '@astryxdesign/core/Grid'
import {
  HStack,
  Layout,
  LayoutContent,
  LayoutPanel,
  StackItem,
  VStack,
} from '@astryxdesign/core/Layout'
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

type Filters = { range: string; outcome: string; mcp: string; token: string }
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
  const navigate = useCallback(
    (changes: Record<string, string | number | undefined>) => {
      const next = { ...filters, page: pagination.page, ...changes }
      router.get('/logs', next, { preserveState: true, replace: true })
    },
    [filters, pagination.page]
  )

  const columns: TableColumn<LogRow>[] = [
    {
      key: 'createdAt',
      header: 'Time',
      width: proportional(2),
      renderCell: (log) => (
        <Text type="supporting" color="secondary">
          {new Date(log.createdAt).toLocaleString()}
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

  const capturedArguments = selectedLog ? displayArguments(selectedLog) : null
  const capturedResponse = selectedLog ? displayResponse(selectedLog) : null
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
    <Layout
      contentWidth={1200}
      height="fill"
      style={{ width: '100%' }}
      content={
        <LayoutContent padding={6} label="MCP call logs">
          <VStack gap={6} width="100%">
            <HStack gap={4} hAlign="between" vAlign="start" wrap="wrap">
              <StackItem size="fill">
                <VStack gap={2}>
                  <Heading level={1}>MCP call logs</Heading>
                  <Text type="body" color="secondary">
                    Inspect individual tool calls, failures, arguments, and timing.
                  </Text>
                </VStack>
              </StackItem>
              <Button label="View analytics" variant="secondary" href="/analytics" />
            </HStack>

            {loggingLevel === 'off' ? (
              <Banner
                status="warning"
                title="Call logging is off"
                description="Existing records remain available until retention removes them. Enable logging in Settings to capture new calls."
                container="card"
              />
            ) : null}

            <VStack gap={10} hAlign="stretch">
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
                  onChange={(value) => navigate({ outcome: value === 'all' ? '' : value, page: 1 })}
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
                  onChange={(value) => navigate({ token: value === 'all' ? '' : value, page: 1 })}
                  width={200}
                />
                <Button
                  label="Clear filters"
                  variant="secondary"
                  onClick={() => router.get('/logs', {}, { replace: true })}
                />
              </Grid>

              {logs.length === 0 ? (
                <Banner
                  status="info"
                  title="No calls in this view"
                  description="Change the filters or make a tool call through the MCP gateway."
                  container="card"
                />
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
                  {pagination.totalPages > 1 ? (
                    <Pagination
                      page={pagination.page}
                      pageSize={pagination.pageSize}
                      totalItems={pagination.total}
                      variant="count"
                      size="sm"
                      onChange={(page) => navigate({ page, logId: undefined })}
                    />
                  ) : null}
                </VStack>
              )}
            </VStack>
          </VStack>
        </LayoutContent>
      }
      end={
        selectedLog ? (
          <LayoutPanel width={420} padding={5} hasDivider label="Call details">
            <VStack gap={4} hAlign="stretch">
              <HStack gap={3} hAlign="between" vAlign="center">
                <Heading level={2}>Call details</Heading>
                <HStack gap={2} vAlign="center">
                  <Token
                    label={selectedLog.outcome === 'success' ? 'Success' : 'Error'}
                    color={selectedLog.outcome === 'success' ? 'green' : 'red'}
                  />
                  <Button
                    label="Close"
                    variant="ghost"
                    size="sm"
                    onClick={() => navigate({ logId: undefined })}
                  />
                </HStack>
              </HStack>
              <VStack gap={2}>
                <Text type="label">Requested tool</Text>
                <Text type="body">{selectedLog.requestedToolName}</Text>
                <Text type="label">MCP</Text>
                <Text type="body">{selectedLog.mcpName ?? selectedLog.mcpSlug ?? 'Unknown'}</Text>
                <Text type="label">Access token</Text>
                <Text type="body">
                  {selectedLog.accessTokenName} ({selectedLog.accessTokenPrefix}…)
                </Text>
                <Text type="label">Caller IP</Text>
                <Text type="body">{selectedLog.callerIp ?? 'Unknown'}</Text>
                <Text type="label">Started</Text>
                <Text type="body">{new Date(selectedLog.createdAt).toLocaleString()}</Text>
                <Text type="label">Duration</Text>
                <Text type="body">{selectedLog.durationMs} ms</Text>
              </VStack>
              {selectedLog.errorCategory ? (
                <Banner
                  status="error"
                  title={selectedLog.errorCategory.replaceAll('_', ' ')}
                  description={selectedLog.errorSummary ?? 'No error summary available.'}
                  container="section"
                />
              ) : null}
              {!selectedLog.argumentsCaptured ? (
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
              {!selectedLog.responseCaptured ? (
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
          </LayoutPanel>
        ) : undefined
      }
    />
  )
}
