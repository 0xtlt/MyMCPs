import { Fragment, useEffect, useMemo, useState } from 'react'
import { Head, router } from '@inertiajs/react'
import type { JSONDataTypes } from '@adonisjs/core/types/transformers'
import { Banner } from '@astryxdesign/core/Banner'
import { useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { CodeBlock } from '@astryxdesign/core/CodeBlock'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import { Grid } from '@astryxdesign/core/Grid'
import {
  HStack,
  Layout,
  LayoutContent,
  LayoutFooter,
  StackItem,
  VStack,
} from '@astryxdesign/core/Layout'
import { Selector } from '@astryxdesign/core/Selector'
import { Heading, Text } from '@astryxdesign/core/Text'
import { Token } from '@astryxdesign/core/Token'
import { LiveRefreshButton } from '~/components/live_refresh_button'
import { browserTimeZone, formatLocalDateTime } from '~/components/local_time'
import { TraceTimeline } from '~/components/trace_timeline'

interface CallRow extends Record<string, JSONDataTypes> {
  id: number
  accessTokenName: string
  accessTokenPrefix: string
  callerIp: string | null
  mcpName: string | null
  mcpSlug: string | null
  requestedToolName: string
  toolName: string | null
  outcome: 'success' | 'error'
  errorCategory: string | null
  errorSummary: string | null
  arguments: string | null
  argumentsCaptured: boolean
  argumentsBytes: number
  argumentsRedacted: boolean
  response: string | null
  responseCaptured: boolean
  responseBytes: number
  responseRedacted: boolean
  durationMs: number
  debugSessionElapsedMs: number | null
  debugSessionId: number | null
  startedAt: string | null
  createdAt: string
}

type DebugSession = {
  id: number
  accessTokenId: number | null
  accessTokenName: string
  accessTokenPrefix: string
  status: 'active' | 'paused' | 'stopped'
  startedAt: string
  pausedAt: string | null
  endedAt: string | null
  pausedDurationMs: number
}

type TokenOption = { id: number; name: string; prefix: string }

function formatBytes(bytes: number) {
  if (bytes < 1_024) return `${bytes} B`
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KiB`
  return `${(bytes / 1_048_576).toFixed(1)} MiB`
}

function formatDuration(durationMs: number) {
  if (durationMs < 1_000) return `${durationMs} ms`
  return `${(durationMs / 1_000).toFixed(durationMs < 10_000 ? 2 : 1)} s`
}

function displayPayload(payload: string | null, isCaptured: boolean) {
  if (!isCaptured) return null
  if (payload === null) return ''
  try {
    return JSON.stringify(JSON.parse(payload), null, 2)
  } catch {
    return payload
  }
}

function SessionStatus({ status }: { status: DebugSession['status'] }) {
  const presentation = {
    active: { label: 'Capturing', color: 'green' as const },
    paused: { label: 'Paused', color: 'orange' as const },
    stopped: { label: 'Stopped', color: 'gray' as const },
  }[status]
  return <Token label={presentation.label} color={presentation.color} />
}

function PayloadCaptureState({ captured, redacted }: { captured: boolean; redacted: boolean }) {
  if (!captured) {
    return (
      <Text type="supporting" color="secondary">
        Not captured
      </Text>
    )
  }
  if (redacted) return <Token label="Redacted" color="orange" />
  return (
    <Text type="supporting" color="secondary">
      Not redacted
    </Text>
  )
}

function CallMetric({ label, value }: { label: string; value: string }) {
  return (
    <Card variant="muted" padding={4} width="100%">
      <VStack gap={2} hAlign="stretch">
        <Text type="label" color="secondary">
          {label}
        </Text>
        <Heading level={3}>{value}</Heading>
      </VStack>
    </Card>
  )
}

function CallPayload({
  label,
  payload,
  captured,
  redacted,
  bytes,
}: {
  label: 'Input' | 'Output'
  payload: string | null
  captured: boolean
  redacted: boolean
  bytes: number
}) {
  const capturedPayload = displayPayload(payload, captured)
  const emptyTitle = label === 'Input' ? 'No input supplied' : 'No output returned'

  return (
    <VStack gap={3} hAlign="stretch" width="100%">
      <HStack gap={3} hAlign="between" vAlign="center" wrap="wrap">
        <VStack gap={1}>
          <Heading level={3}>{label}</Heading>
          <Text type="supporting" color="secondary">
            {formatBytes(bytes)}
          </Text>
        </VStack>
        <PayloadCaptureState captured={captured} redacted={redacted} />
      </HStack>
      {capturedPayload === null ? (
        <Banner
          status="info"
          title={`${label} was not captured`}
          description="Only metadata is available for this payload."
          container="section"
        />
      ) : capturedPayload === '' ? (
        <Banner status="info" title={emptyTitle} container="section" />
      ) : (
        <CodeBlock code={capturedPayload} language="json" width="100%" maxHeight={360} isWrapped />
      )}
    </VStack>
  )
}

function CallDetails({ call }: { call: CallRow }) {
  return (
    <VStack gap={6} hAlign="stretch">
      <Grid columns={{ minWidth: 180, max: 4, repeat: 'fit' }} gap={3} width="100%">
        <CallMetric label="Duration" value={formatDuration(call.durationMs)} />
        <CallMetric
          label="Timeline position"
          value={
            call.debugSessionElapsedMs === null
              ? 'Unavailable'
              : `+${formatDuration(call.debugSessionElapsedMs)}`
          }
        />
        <CallMetric label="Input size" value={formatBytes(call.argumentsBytes)} />
        <CallMetric label="Output size" value={formatBytes(call.responseBytes)} />
      </Grid>
      {call.errorCategory ? (
        <Banner
          status="error"
          title={call.errorCategory.replaceAll('_', ' ')}
          description={call.errorSummary ?? 'No error summary available.'}
          container="section"
        />
      ) : null}
      <Grid columns={{ minWidth: 320, max: 2, repeat: 'fit' }} gap={6} width="100%">
        <CallPayload
          label="Input"
          payload={call.arguments}
          captured={call.argumentsCaptured}
          redacted={call.argumentsRedacted}
          bytes={call.argumentsBytes}
        />
        <CallPayload
          label="Output"
          payload={call.response}
          captured={call.responseCaptured}
          redacted={call.responseRedacted}
          bytes={call.responseBytes}
        />
      </Grid>
    </VStack>
  )
}

export default function DebugIndex({
  sessions,
  selectedSession,
  calls,
  selectedCall,
  tokens,
  filters,
  observedAt,
  maxSessionCalls,
}: {
  sessions: DebugSession[]
  selectedSession: DebugSession | null
  calls: CallRow[]
  selectedCall: CallRow | null
  tokens: TokenOption[]
  filters: { timeZone: string }
  observedAt: string
  maxSessionCalls: number
}) {
  const { isMobile } = useAppShellMobile()
  const [isStartOpen, setIsStartOpen] = useState(false)
  const [selectedTokenId, setSelectedTokenId] = useState(tokens[0] ? String(tokens[0].id) : '')
  const [isMutating, setIsMutating] = useState(false)

  const navigate = (changes: { sessionId?: number; callId?: number; timeZone?: string }) => {
    router.get(
      '/debug',
      {
        sessionId: selectedSession?.id,
        timeZone: filters.timeZone,
        ...changes,
      },
      { preserveState: true, replace: true }
    )
  }

  useEffect(() => {
    const localTimeZone = browserTimeZone()
    if (!localTimeZone || localTimeZone === filters.timeZone) return
    router.get(
      '/debug',
      { sessionId: selectedSession?.id, timeZone: localTimeZone },
      { preserveState: true, replace: true }
    )
  }, [filters.timeZone, selectedSession?.id])

  const sessionOptions = useMemo(
    () =>
      sessions.map((debugSession) => ({
        value: String(debugSession.id),
        label: `${debugSession.accessTokenName} · ${debugSession.status}`,
      })),
    [sessions]
  )

  function updateSession(action: 'pause' | 'resume' | 'stop') {
    if (!selectedSession || isMutating) return
    setIsMutating(true)
    router.patch(
      `/debug-sessions/${selectedSession.id}`,
      { action },
      { onFinish: () => setIsMutating(false) }
    )
  }

  function startSession() {
    if (!selectedTokenId || isMutating) return
    setIsMutating(true)
    router.post(
      '/debug-sessions',
      { accessTokenId: Number(selectedTokenId) },
      {
        onSuccess: () => setIsStartOpen(false),
        onFinish: () => setIsMutating(false),
      }
    )
  }

  return (
    <Fragment>
      {/* Responsive contract: the timeline keeps its full width; details open in a focused dialog, switching to full-screen at 768px and below. */}
      <Layout
        contentWidth={1440}
        height={isMobile ? 'auto' : 'fill'}
        style={{ width: '100%' }}
        content={
          <LayoutContent padding={isMobile ? 0 : 6} label="MCP debugger">
            <Head title="MCP debugger" />
            <VStack gap={isMobile ? 4 : 6} paddingBlock={isMobile ? 3 : 4} width="100%">
              <HStack gap={4} hAlign="between" vAlign="start" wrap="wrap">
                <StackItem size="fill">
                  <VStack gap={2}>
                    <Heading level={1}>MCP debugger</Heading>
                    <Text type="body" color="secondary">
                      Capture one client session, inspect every tool call, and follow its timing.
                    </Text>
                  </VStack>
                </StackItem>
                <HStack gap={2} vAlign="center" wrap="wrap">
                  {selectedSession?.status === 'active' ? (
                    <LiveRefreshButton
                      key={`live-${selectedSession.id}`}
                      defaultLive
                      intervalMs={3_000}
                    />
                  ) : (
                    <LiveRefreshButton key="live-manual" />
                  )}
                  <Button
                    label="Start debug session"
                    variant="primary"
                    onClick={() => setIsStartOpen(true)}
                    isDisabled={tokens.length === 0}
                    tooltip={
                      tokens.length === 0 ? 'Create an active access token first' : undefined
                    }
                  />
                </HStack>
              </HStack>

              {selectedSession ? (
                <VStack gap={4} hAlign="stretch">
                  <HStack gap={3} hAlign="between" vAlign="end" wrap="wrap">
                    <Selector
                      label="Debug session"
                      value={String(selectedSession.id)}
                      options={sessionOptions}
                      onChange={(value) =>
                        navigate({ sessionId: Number(value), callId: undefined })
                      }
                      width={isMobile ? '100%' : 360}
                    />
                    <HStack gap={2} wrap="wrap" vAlign="center">
                      <SessionStatus status={selectedSession.status} />
                      {selectedSession.status === 'active' ? (
                        <Button
                          label="Pause capture"
                          variant="secondary"
                          isLoading={isMutating}
                          onClick={() => updateSession('pause')}
                        />
                      ) : selectedSession.status === 'paused' ? (
                        <Button
                          label="Continue capture"
                          variant="secondary"
                          isLoading={isMutating}
                          onClick={() => updateSession('resume')}
                        />
                      ) : null}
                      {selectedSession.status !== 'stopped' ? (
                        <Button
                          label="Stop session"
                          variant="ghost"
                          isLoading={isMutating}
                          onClick={() => updateSession('stop')}
                        />
                      ) : null}
                    </HStack>
                  </HStack>

                  <Banner
                    status={selectedSession.status === 'active' ? 'info' : 'warning'}
                    title={
                      selectedSession.status === 'active'
                        ? `Capturing ${selectedSession.accessTokenName}`
                        : selectedSession.status === 'paused'
                          ? 'Capture is paused'
                          : 'Session is read-only'
                    }
                    description={
                      selectedSession.status === 'active'
                        ? 'Arguments and responses are captured for this token only. Credential-shaped values are redacted before storage.'
                        : selectedSession.status === 'paused'
                          ? 'Continue when you are ready; calls made while paused are excluded from this trace.'
                          : `Capture stopped ${formatLocalDateTime(selectedSession.endedAt!, filters.timeZone)}.`
                    }
                    container="section"
                  />

                  {calls.length === 0 ? (
                    <Banner
                      status="info"
                      title="Waiting for calls"
                      description="Use the selected access token from an MCP client. New calls will appear while capture is active."
                      container="card"
                    />
                  ) : (
                    <VStack gap={3} hAlign="stretch">
                      <HStack gap={3} hAlign="between" vAlign="center" wrap="wrap">
                        <Heading level={2}>Timeline</Heading>
                        <Text type="supporting" color="secondary">
                          {calls.length} call{calls.length === 1 ? '' : 's'} · click a bar to
                          inspect
                        </Text>
                      </HStack>
                      {calls.length === maxSessionCalls ? (
                        <Banner
                          status="warning"
                          title={`Showing the first ${maxSessionCalls} calls`}
                          description="Stop this session and start a narrower trace if you need later calls."
                          container="section"
                        />
                      ) : null}
                      <TraceTimeline
                        calls={calls}
                        session={selectedSession}
                        selectedCallId={selectedCall?.id ?? null}
                        observedAt={observedAt}
                        onSelect={(call) => navigate({ callId: call.id })}
                      />
                    </VStack>
                  )}
                </VStack>
              ) : (
                <Banner
                  status="info"
                  title="Start a focused debug session"
                  description="Choose an active access token to capture its next MCP calls without increasing logging for every client."
                  container="card"
                />
              )}
            </VStack>
          </LayoutContent>
        }
      />

      {isStartOpen ? (
        <Dialog
          isOpen
          onOpenChange={setIsStartOpen}
          purpose="form"
          variant={isMobile ? 'fullscreen' : undefined}
        >
          <Layout
            header={<DialogHeader title="Start debug session" onOpenChange={setIsStartOpen} />}
            content={
              <LayoutContent isScrollable>
                <VStack gap={4} hAlign="stretch">
                  <Text type="body" color="secondary">
                    Capture sanitized arguments and responses for one access token until you pause
                    or stop the session.
                  </Text>
                  <Selector
                    label="Access token"
                    value={selectedTokenId}
                    options={tokens.map((token) => ({
                      value: String(token.id),
                      label: `${token.name} (${token.prefix}…)`,
                    }))}
                    onChange={setSelectedTokenId}
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
                    onClick={() => setIsStartOpen(false)}
                  />
                  <Button
                    label="Start capture"
                    variant="primary"
                    isLoading={isMutating}
                    isDisabled={!selectedTokenId}
                    onClick={startSession}
                  />
                </HStack>
              </LayoutFooter>
            }
          />
        </Dialog>
      ) : null}

      {selectedCall ? (
        <Dialog
          isOpen
          onOpenChange={(open) => {
            if (!open) navigate({ callId: undefined })
          }}
          purpose="info"
          width={1040}
          maxHeight="calc(100dvh - var(--spacing-8))"
          variant={isMobile ? 'fullscreen' : undefined}
        >
          <Layout
            header={
              <DialogHeader
                title={selectedCall.toolName ?? selectedCall.requestedToolName}
                subtitle={`${selectedCall.mcpName ?? selectedCall.mcpSlug ?? 'Gateway'} · ${formatLocalDateTime(selectedCall.startedAt ?? selectedCall.createdAt, filters.timeZone)} · ${selectedCall.callerIp ?? 'Unknown caller IP'}`}
                endContent={
                  <Token
                    label={selectedCall.outcome === 'success' ? 'Success' : 'Error'}
                    color={selectedCall.outcome === 'success' ? 'green' : 'red'}
                  />
                }
                onOpenChange={(open) => {
                  if (!open) navigate({ callId: undefined })
                }}
              />
            }
            content={
              <LayoutContent isScrollable>
                <CallDetails call={selectedCall} />
              </LayoutContent>
            }
          />
        </Dialog>
      ) : null}
    </Fragment>
  )
}
