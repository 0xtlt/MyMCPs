import { Fragment, useEffect, useMemo, useState, type CSSProperties } from 'react'
import { Head, router } from '@inertiajs/react'
import type { JSONDataTypes } from '@adonisjs/core/types/transformers'
import { Banner } from '@astryxdesign/core/Banner'
import { useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Button } from '@astryxdesign/core/Button'
import { CodeBlock } from '@astryxdesign/core/CodeBlock'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import {
  HStack,
  Layout,
  LayoutContent,
  LayoutFooter,
  LayoutPanel,
  StackItem,
  VStack,
} from '@astryxdesign/core/Layout'
import { MetadataList, MetadataListItem } from '@astryxdesign/core/MetadataList'
import { Selector } from '@astryxdesign/core/Selector'
import { Heading, Text } from '@astryxdesign/core/Text'
import { Token } from '@astryxdesign/core/Token'
import { LiveRefreshButton } from '~/components/live_refresh_button'
import { browserTimeZone, formatLocalDateTime } from '~/components/local_time'

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

function CaptureState({ captured, redacted }: { captured: boolean; redacted: boolean }) {
  if (!captured) return <Token label="Not captured" color="gray" />
  return redacted ? (
    <Token label="Sensitive values redacted" color="orange" />
  ) : (
    <Token label="Captured, not redacted" color="green" />
  )
}

function CallDetails({ call, timeZone }: { call: CallRow; timeZone: string }) {
  const capturedArguments = displayPayload(call.arguments, call.argumentsCaptured)
  const capturedResponse = displayPayload(call.response, call.responseCaptured)

  return (
    <VStack gap={5} hAlign="stretch">
      <HStack gap={3} hAlign="between" vAlign="center">
        <Heading level={2}>Call details</Heading>
        <Token
          label={call.outcome === 'success' ? 'Success' : 'Error'}
          color={call.outcome === 'success' ? 'green' : 'red'}
        />
      </HStack>
      <MetadataList columns="single" label={{ position: 'top' }}>
        <MetadataListItem label="Tool">
          <Text type="body">{call.toolName ?? call.requestedToolName}</Text>
        </MetadataListItem>
        <MetadataListItem label="MCP">
          <Text type="body">{call.mcpName ?? call.mcpSlug ?? 'Gateway'}</Text>
        </MetadataListItem>
        <MetadataListItem label="Started">
          <Text type="body">{formatLocalDateTime(call.startedAt ?? call.createdAt, timeZone)}</Text>
        </MetadataListItem>
        <MetadataListItem label="Duration">
          <Text type="body">{formatDuration(call.durationMs)}</Text>
        </MetadataListItem>
        <MetadataListItem label="Input">
          <HStack gap={2} wrap="wrap" vAlign="center">
            <Text type="body">{formatBytes(call.argumentsBytes)}</Text>
            <CaptureState captured={call.argumentsCaptured} redacted={call.argumentsRedacted} />
          </HStack>
        </MetadataListItem>
        <MetadataListItem label="Output">
          <HStack gap={2} wrap="wrap" vAlign="center">
            <Text type="body">{formatBytes(call.responseBytes)}</Text>
            <CaptureState captured={call.responseCaptured} redacted={call.responseRedacted} />
          </HStack>
        </MetadataListItem>
        <MetadataListItem label="Caller IP">
          <Text type="body">{call.callerIp ?? 'Unknown'}</Text>
        </MetadataListItem>
      </MetadataList>
      {call.errorCategory ? (
        <Banner
          status="error"
          title={call.errorCategory.replaceAll('_', ' ')}
          description={call.errorSummary ?? 'No error summary available.'}
          container="section"
        />
      ) : null}
      {capturedArguments === null ? (
        <Banner
          status="info"
          title="Input was not captured"
          description="Only metadata is available for this call."
          container="section"
        />
      ) : capturedArguments === '' ? (
        <Banner status="info" title="No input supplied" container="section" />
      ) : (
        <CodeBlock
          code={capturedArguments}
          language="json"
          title="Input"
          width="100%"
          maxHeight={320}
          isWrapped
        />
      )}
      {capturedResponse === null ? (
        <Banner
          status="info"
          title="Output was not captured"
          description="Only metadata is available for this call."
          container="section"
        />
      ) : capturedResponse === '' ? (
        <Banner status="info" title="No output returned" container="section" />
      ) : (
        <CodeBlock
          code={capturedResponse}
          language="json"
          title="Output"
          width="100%"
          maxHeight={440}
          isWrapped
        />
      )}
    </VStack>
  )
}

function TraceTimeline({
  calls,
  session,
  selectedCallId,
  isMobile,
  observedAt,
  onSelect,
}: {
  calls: CallRow[]
  session: DebugSession
  selectedCallId: number | null
  isMobile: boolean
  observedAt: string
  onSelect: (call: CallRow) => void
}) {
  const callEnds = calls.map((call) => {
    const callOffset =
      call.debugSessionElapsedMs ??
      Math.max(
        0,
        new Date(call.startedAt ?? call.createdAt).getTime() - new Date(session.startedAt).getTime()
      )
    return callOffset + call.durationMs
  })
  const sessionStart = new Date(session.startedAt).getTime()
  const captureEnd = session.endedAt
    ? new Date(session.endedAt).getTime()
    : session.pausedAt
      ? new Date(session.pausedAt).getTime()
      : new Date(observedAt).getTime()
  const windowMs = Math.max(1, captureEnd - sessionStart - session.pausedDurationMs, ...callEnds)

  return (
    <section className="debug-timeline" data-mobile={isMobile || undefined}>
      <HStack gap={3} hAlign="between" vAlign="center" padding={3}>
        <Text type="label">Session start</Text>
        <Text type="supporting" color="secondary">
          +{formatDuration(windowMs)}
        </Text>
      </HStack>
      <ol className="debug-timeline-list" aria-label="Debug session call timeline">
        {calls.map((call) => {
          const callOffset =
            call.debugSessionElapsedMs ??
            Math.max(0, new Date(call.startedAt ?? call.createdAt).getTime() - sessionStart)
          const offset = Math.max(0, (callOffset / windowMs) * 100)
          const width = Math.max(0.5, (call.durationMs / windowMs) * 100)
          const toolLabel = call.toolName ?? call.requestedToolName
          return (
            <li className="debug-timeline-row" key={call.id}>
              <Text type="supporting">{toolLabel}</Text>
              <section className="debug-timeline-track">
                <button
                  type="button"
                  className="debug-timeline-call"
                  data-outcome={call.outcome}
                  data-selected={selectedCallId === call.id || undefined}
                  style={
                    {
                      '--debug-trace-start': `${Math.min(offset, 99.5)}%`,
                      '--debug-trace-width': `${Math.min(width, 100 - offset)}%`,
                    } as CSSProperties
                  }
                  title={`${toolLabel}: ${formatDuration(call.durationMs)}`}
                  aria-label={`Inspect ${toolLabel}, ${formatDuration(call.durationMs)}`}
                  onClick={() => onSelect(call)}
                >
                  <span className="debug-timeline-bar" aria-hidden="true" />
                </button>
              </section>
              <Text type="supporting" color="secondary">
                {formatDuration(call.durationMs)}
              </Text>
            </li>
          )
        })}
      </ol>
    </section>
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
      {/* Responsive contract: 1440px full timeline + inspector; 768px and 375px use a full-screen call inspector and stacked trace rows. */}
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
                        isMobile={isMobile}
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
        end={
          selectedCall && !isMobile ? (
            <LayoutPanel width={420} padding={5} hasDivider label="Call details" isScrollable>
              <CallDetails call={selectedCall} timeZone={filters.timeZone} />
            </LayoutPanel>
          ) : undefined
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

      {selectedCall && isMobile ? (
        <Dialog
          isOpen
          onOpenChange={(open) => {
            if (!open) navigate({ callId: undefined })
          }}
          purpose="info"
          variant="fullscreen"
        >
          <Layout
            header={
              <DialogHeader
                title="Call details"
                onOpenChange={(open) => {
                  if (!open) navigate({ callId: undefined })
                }}
              />
            }
            content={
              <LayoutContent isScrollable>
                <CallDetails call={selectedCall} timeZone={filters.timeZone} />
              </LayoutContent>
            }
          />
        </Dialog>
      ) : null}
    </Fragment>
  )
}
