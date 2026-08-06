import { useEffect, useMemo, useState } from 'react'
import { Head, router } from '@inertiajs/react'
import type { JSONDataTypes } from '@adonisjs/core/types/transformers'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { DateRangeInput, type DateRange } from '@astryxdesign/core/DateRangeInput'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import { Divider } from '@astryxdesign/core/Divider'
import { Grid } from '@astryxdesign/core/Grid'
import {
  HStack,
  Layout,
  LayoutContent,
  LayoutFooter,
  StackItem,
  VStack,
} from '@astryxdesign/core/Layout'
import { SegmentedControl, SegmentedControlItem } from '@astryxdesign/core/SegmentedControl'
import { Table, pixel, proportional, type TableColumn } from '@astryxdesign/core/Table'
import { Heading, Text } from '@astryxdesign/core/Text'
import { TimeInput, type ISOTimeString } from '@astryxdesign/core/TimeInput'
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
import { DateTime } from 'luxon'
import { LiveRefreshButton } from '~/components/live_refresh_button'
import { browserTimeZone, formatTimeZoneLabel } from '~/components/local_time'

ChartJS.register(CategoryScale, LinearScale, PointElement, LineElement, ChartTooltip, Legend)

interface BreakdownRow extends Record<string, JSONDataTypes> {
  label: string
  total: number
  errors: number
  averageDurationMs: number
}

type AnalyticsRange = '24h' | '7d' | '30d' | 'custom'
type TimelinePoint = {
  bucket: string
  label: string
  total: number
  errors: number
}

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
    <VStack gap={3} hAlign="stretch">
      <Heading level={2}>{title}</Heading>
      {rows.length > 0 ? (
        <Table data={rows} columns={breakdownColumns} idKey="label" density="compact" />
      ) : (
        <Text type="body" color="secondary">
          No data for this period.
        </Text>
      )}
      <Divider />
    </VStack>
  )
}

function MetricCell({ label, value }: { label: string; value: string }) {
  return (
    <StackItem className="analytics-metric-cell" size="fill">
      <VStack gap={1} padding={4} width="100%">
        <Text type="label" color="secondary">
          {label}
        </Text>
        <Heading level={2}>{value}</Heading>
      </VStack>
    </StackItem>
  )
}

function MetricStrip({ metrics }: { metrics: Array<{ label: string; value: string }> }) {
  return (
    <Grid className="analytics-metric-strip" columns={4} gap={0} width="100%">
      {metrics.map((metric) => (
        <MetricCell key={metric.label} {...metric} />
      ))}
    </Grid>
  )
}

function rangeDates(start: string, end: string): DateRange {
  return { start: start.slice(0, 10), end: end.slice(0, 10) } as DateRange
}

function rangeTime(value: string): ISOTimeString {
  return value.slice(11, 16) as ISOTimeString
}

export default function AnalyticsIndex({
  range,
  start,
  end,
  timeZone,
  loggingLevel,
  metrics,
  timeline,
  topMcps,
  topTools,
  topTokens,
}: {
  range: AnalyticsRange
  start: string
  end: string
  timeZone: string
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
  const [isCustomRangeOpen, setIsCustomRangeOpen] = useState(false)
  const [customDates, setCustomDates] = useState<DateRange | null>(rangeDates(start, end))
  const [customStartTime, setCustomStartTime] = useState<ISOTimeString | undefined>(
    rangeTime(start)
  )
  const [customEndTime, setCustomEndTime] = useState<ISOTimeString | undefined>(rangeTime(end))

  const customStart =
    customDates && customStartTime ? `${customDates.start}T${customStartTime}` : undefined
  const customEnd = customDates && customEndTime ? `${customDates.end}T${customEndTime}` : undefined

  function openCustomRange() {
    setCustomDates(rangeDates(start, end))
    setCustomStartTime(rangeTime(start))
    setCustomEndTime(rangeTime(end))
    setIsCustomRangeOpen(true)
  }

  function changeRange(value: string) {
    if (value === 'custom') {
      openCustomRange()
      return
    }
    router.get('/analytics', { range: value, timeZone }, { preserveState: true, replace: true })
  }

  const customRangeError = useMemo(() => {
    if (!customStart || !customEnd) return 'Choose both a start and an end time.'

    const customStartDate = DateTime.fromISO(customStart, { zone: timeZone })
    const customEndDate = DateTime.fromISO(customEnd, { zone: timeZone })
    if (!customStartDate.isValid || !customEndDate.isValid) return 'Enter valid dates and times.'
    if (customEndDate.toMillis() <= customStartDate.toMillis()) {
      return 'End time must be after start time.'
    }
    if (customEndDate.toMillis() > customStartDate.plus({ days: 365 }).toMillis()) {
      return 'Custom ranges can span up to 365 days.'
    }
    return null
  }, [customEnd, customStart, timeZone])

  function applyCustomRange() {
    if (customRangeError || !customStart || !customEnd) return

    setIsCustomRangeOpen(false)
    router.get(
      '/analytics',
      { range: 'custom', start: customStart, end: customEnd, timeZone },
      { preserveState: true, preserveScroll: true, replace: true }
    )
  }

  useEffect(() => {
    const localTimeZone = browserTimeZone()
    if (!localTimeZone || localTimeZone === timeZone) return

    router.get(
      '/analytics',
      {
        range,
        timeZone: localTimeZone,
        ...(range === 'custom' ? { start, end } : {}),
      },
      { preserveState: true, preserveScroll: true, replace: true }
    )
  }, [end, range, start, timeZone])

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
            <SegmentedControlItem value="custom" label="Custom" onClick={openCustomRange} />
          </SegmentedControl>
          <LiveRefreshButton />
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

      <MetricStrip metrics={cards} />

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
                Successful and failed tool-call attempts in {formatTimeZoneLabel(timeZone)}.
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

      <Grid gap={6} columns={{ minWidth: 320, max: 2, repeat: 'fit' }}>
        <Breakdown title="Top MCPs" rows={topMcps} />
        <Breakdown title="Top tools" rows={topTools} />
        <Breakdown title="Top access tokens" rows={topTokens} />
      </Grid>

      <Dialog
        isOpen={isCustomRangeOpen}
        onOpenChange={setIsCustomRangeOpen}
        purpose="form"
        width={560}
      >
        <Layout
          header={
            <DialogHeader
              title="Custom analytics range"
              subtitle={`Choose exact times in ${formatTimeZoneLabel(timeZone)}`}
              onOpenChange={setIsCustomRangeOpen}
            />
          }
          content={
            <LayoutContent>
              <VStack gap={4} hAlign="stretch">
                <DateRangeInput
                  label="Dates"
                  value={customDates}
                  onChange={setCustomDates}
                  numberOfMonths={2}
                  width="100%"
                  isRequired
                />
                <HStack className="mobile-full-width-fields" gap={4} vAlign="start" wrap="wrap">
                  <TimeInput
                    label="Start time"
                    value={customStartTime}
                    onChange={setCustomStartTime}
                    hourFormat="24h"
                    increment={5}
                    width={240}
                    isRequired
                  />
                  <TimeInput
                    label="End time"
                    value={customEndTime}
                    onChange={setCustomEndTime}
                    hourFormat="24h"
                    increment={5}
                    width={240}
                    isRequired
                    status={
                      customRangeError ? { type: 'error', message: customRangeError } : undefined
                    }
                  />
                </HStack>
              </VStack>
            </LayoutContent>
          }
          footer={
            <LayoutFooter>
              <HStack gap={2} hAlign="end">
                <Button
                  label="Cancel"
                  variant="secondary"
                  onClick={() => setIsCustomRangeOpen(false)}
                />
                <Button
                  label="Apply range"
                  variant="primary"
                  onClick={applyCustomRange}
                  isDisabled={Boolean(customRangeError)}
                />
              </HStack>
            </LayoutFooter>
          }
        />
      </Dialog>
    </VStack>
  )
}
