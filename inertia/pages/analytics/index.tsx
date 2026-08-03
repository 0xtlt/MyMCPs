import { useMemo } from 'react'
import { Head, router } from '@inertiajs/react'
import type { JSONDataTypes } from '@adonisjs/core/types/transformers'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { Grid } from '@astryxdesign/core/Grid'
import { HStack, StackItem, VStack } from '@astryxdesign/core/Layout'
import { SegmentedControl, SegmentedControlItem } from '@astryxdesign/core/SegmentedControl'
import { Table, pixel, proportional, type TableColumn } from '@astryxdesign/core/Table'
import { Heading, Text } from '@astryxdesign/core/Text'
import {
  CategoryScale,
  Chart as ChartJS,
  Legend,
  LinearScale,
  LineElement,
  PointElement,
  Tooltip as ChartTooltip,
  type ChartData,
  type ChartOptions,
} from 'chart.js'
import { Line } from 'react-chartjs-2'
import { useTheme } from '@astryxdesign/core/theme'

ChartJS.register(CategoryScale, LinearScale, PointElement, LineElement, ChartTooltip, Legend)

interface BreakdownRow extends Record<string, JSONDataTypes> {
  label: string
  total: number
  errors: number
  averageDurationMs: number
}

type TimelinePoint = { bucket: string; label: string; total: number; errors: number }

const breakdownColumns: TableColumn<BreakdownRow>[] = [
  { key: 'label', header: 'Name', width: proportional(2) },
  { key: 'total', header: 'Calls', width: pixel(80), align: 'end' },
  { key: 'errors', header: 'Errors', width: pixel(80), align: 'end' },
  {
    key: 'averageDurationMs',
    header: 'Avg.',
    width: pixel(90),
    align: 'end',
    renderCell: (row) => <Text type="supporting">{row.averageDurationMs} ms</Text>,
  },
]

function Breakdown({ title, rows }: { title: string; rows: BreakdownRow[] }) {
  return (
    <Card padding={4} width="100%">
      <VStack gap={3} hAlign="stretch">
        <Heading level={2}>{title}</Heading>
        {rows.length > 0 ? (
          <Table data={rows} columns={breakdownColumns} idKey="label" density="compact" />
        ) : (
          <Text type="body" color="secondary">
            No data for this period.
          </Text>
        )}
      </VStack>
    </Card>
  )
}

export default function AnalyticsIndex({
  range,
  loggingLevel,
  metrics,
  timeline,
  topMcps,
  topTools,
  topTokens,
}: {
  range: '24h' | '7d' | '30d'
  loggingLevel: 'off' | 'metadata' | 'arguments' | 'responses'
  metrics: {
    total: number
    successes: number
    errors: number
    successRate: number
    errorRate: number
    averageDurationMs: number
  }
  timeline: TimelinePoint[]
  topMcps: BreakdownRow[]
  topTools: BreakdownRow[]
  topTokens: BreakdownRow[]
}) {
  const { token } = useTheme()

  function changeRange(value: string) {
    router.get('/analytics', { range: value }, { preserveState: true, replace: true })
  }

  const cards = [
    { label: 'Total calls', value: metrics.total.toLocaleString() },
    { label: 'Success rate', value: `${metrics.successRate.toFixed(1)}%` },
    { label: 'Error rate', value: `${metrics.errorRate.toFixed(1)}%` },
    { label: 'Average duration', value: `${metrics.averageDurationMs} ms` },
  ]

  const chartData = useMemo<ChartData<'line'>>(
    () => ({
      labels: timeline.map((point) => point.label),
      datasets: [
        {
          label: 'Calls',
          data: timeline.map((point) => point.total),
          borderColor: token('--color-data-categorical-blue'),
          backgroundColor: token('--color-data-categorical-blue'),
          pointBackgroundColor: token('--color-data-categorical-blue'),
        },
        {
          label: 'Errors',
          data: timeline.map((point) => point.errors),
          borderColor: token('--color-data-categorical-red'),
          backgroundColor: token('--color-data-categorical-red'),
          pointBackgroundColor: token('--color-data-categorical-red'),
        },
      ],
    }),
    [timeline, token]
  )

  const chartOptions = useMemo<ChartOptions<'line'>>(
    () => ({
      responsive: true,
      maintainAspectRatio: false,
      interaction: { mode: 'index', intersect: false },
      animation: { duration: 250 },
      elements: {
        line: { borderWidth: 2, tension: 0.3 },
        point: { radius: 3, hoverRadius: 5 },
      },
      plugins: {
        legend: {
          position: 'bottom',
          labels: {
            color: token('--color-text-secondary'),
            usePointStyle: true,
            boxWidth: 8,
          },
        },
      },
      scales: {
        x: {
          grid: { display: false },
          ticks: { color: token('--color-text-secondary') },
          border: { color: token('--color-border-secondary') },
        },
        y: {
          beginAtZero: true,
          grid: { color: token('--color-border-secondary') },
          ticks: { color: token('--color-text-secondary'), precision: 0 },
          border: { display: false },
        },
      },
    }),
    [token]
  )

  return (
    <VStack gap={6} maxWidth={1200} width="100%">
      <Head title="MCP analytics" />
      <HStack gap={4} hAlign="between" vAlign="start" wrap="wrap">
        <StackItem size="fill">
          <VStack gap={2}>
            <Heading level={1}>MCP analytics</Heading>
            <Text type="body" color="secondary">
              Track gateway usage, reliability, and tool-call latency.
            </Text>
          </VStack>
        </StackItem>
        <HStack gap={3} vAlign="center" wrap="wrap">
          <SegmentedControl value={range} onChange={changeRange} label="Analytics time range">
            <SegmentedControlItem value="24h" label="24 hours" />
            <SegmentedControlItem value="7d" label="7 days" />
            <SegmentedControlItem value="30d" label="30 days" />
          </SegmentedControl>
          <Button label="View logs" variant="secondary" href="/logs" />
        </HStack>
      </HStack>

      {loggingLevel === 'off' ? (
        <Banner
          status="warning"
          title="Call logging is off"
          description="Analytics includes retained records only. Enable logging in Settings to collect new data."
          container="card"
        />
      ) : null}

      <Grid gap={4} columns={{ minWidth: 220, repeat: 'fit' }}>
        {cards.map((metric) => (
          <Card key={metric.label} padding={5} width="100%">
            <VStack gap={2}>
              <Text type="label" color="secondary">
                {metric.label}
              </Text>
              <Heading level={2}>{metric.value}</Heading>
            </VStack>
          </Card>
        ))}
      </Grid>

      {metrics.total === 0 ? (
        <Banner
          status="info"
          title="No calls in this period"
          description="Choose another time range or make a tool call through the MCP gateway."
          container="card"
        />
      ) : (
        <Card padding={5} width="100%">
          <VStack gap={4} hAlign="stretch">
            <VStack gap={1}>
              <Heading level={2}>Calls and errors over time</Heading>
              <Text type="supporting" color="secondary">
                Successful and failed tool-call attempts recorded by the gateway.
              </Text>
            </VStack>
            <VStack height={300} hAlign="stretch">
              <Line
                data={chartData}
                options={chartOptions}
                role="img"
                aria-label="Calls and errors over the selected time range"
              />
            </VStack>
          </VStack>
        </Card>
      )}

      <Grid gap={6} columns={{ minWidth: 320, repeat: 'fit' }}>
        <Breakdown title="Top MCPs" rows={topMcps} />
        <Breakdown title="Top tools" rows={topTools} />
        <Breakdown title="Top access tokens" rows={topTokens} />
      </Grid>
    </VStack>
  )
}
