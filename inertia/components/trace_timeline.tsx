import { type CSSProperties } from 'react'
import { useAppShellMobile } from '@astryxdesign/core/AppShell'
import { HStack } from '@astryxdesign/core/Layout'
import { Text } from '@astryxdesign/core/Text'

type TraceTimelineCall = {
  id: number
  requestedToolName: string
  toolName: string | null
  outcome: 'success' | 'error'
  durationMs: number
  debugSessionElapsedMs: number | null
  startedAt: string | null
  createdAt: string
}

type TraceTimelineSession = {
  startedAt: string
  pausedAt: string | null
  endedAt: string | null
  pausedDurationMs: number
}

function formatDuration(durationMs: number) {
  if (durationMs < 1_000) return `${durationMs} ms`
  return `${(durationMs / 1_000).toFixed(durationMs < 10_000 ? 2 : 1)} s`
}

export function TraceTimeline({
  calls,
  session,
  selectedCallId,
  observedAt,
  onSelect,
}: {
  calls: TraceTimelineCall[]
  session: TraceTimelineSession
  selectedCallId: number | null
  observedAt: string
  onSelect: (call: TraceTimelineCall) => void
}) {
  const { isMobile } = useAppShellMobile()
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
          const isSelected = selectedCallId === call.id

          return (
            <li className="debug-timeline-row" key={call.id}>
              <Text type="supporting">{toolLabel}</Text>
              <section className="debug-timeline-track" aria-label={`${toolLabel} timeline lane`}>
                <button
                  type="button"
                  className="debug-timeline-call"
                  data-outcome={call.outcome}
                  data-selected={isSelected || undefined}
                  aria-current={isSelected || undefined}
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
