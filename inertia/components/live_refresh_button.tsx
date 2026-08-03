import { useEffect, useState } from 'react'
import { router } from '@inertiajs/react'
import { Button } from '@astryxdesign/core/Button'

const LIVE_REFRESH_INTERVAL_MS = 30_000

export function LiveRefreshButton() {
  const [isLive, setIsLive] = useState(false)

  useEffect(() => {
    if (!isLive) return

    const interval = window.setInterval(() => router.reload(), LIVE_REFRESH_INTERVAL_MS)
    return () => window.clearInterval(interval)
  }, [isLive])

  return (
    <Button
      label="Live"
      variant={isLive ? 'primary' : 'secondary'}
      aria-pressed={isLive}
      tooltip={`${isLive ? 'Disable' : 'Enable'} automatic refresh every 30 seconds`}
      onClick={() => setIsLive((current) => !current)}
    />
  )
}
