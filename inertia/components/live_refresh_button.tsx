import { useEffect, useState } from 'react'
import { router } from '@inertiajs/react'
import { Button } from '@astryxdesign/core/Button'

const LIVE_REFRESH_INTERVAL_MS = 30_000

export function LiveRefreshButton({
  defaultLive = false,
  intervalMs = LIVE_REFRESH_INTERVAL_MS,
}: {
  defaultLive?: boolean
  intervalMs?: number
} = {}) {
  const [isLive, setIsLive] = useState(defaultLive)

  useEffect(() => {
    if (!isLive) return

    const interval = window.setInterval(() => router.reload(), intervalMs)
    return () => window.clearInterval(interval)
  }, [intervalMs, isLive])

  return (
    <Button
      label="Live"
      variant={isLive ? 'primary' : 'secondary'}
      aria-pressed={isLive}
      tooltip={`${isLive ? 'Disable' : 'Enable'} automatic refresh`}
      onClick={() => setIsLive((current) => !current)}
    />
  )
}
