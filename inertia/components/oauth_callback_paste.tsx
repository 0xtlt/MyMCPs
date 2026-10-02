import { useState } from 'react'
import { Button } from '@astryxdesign/core/Button'
import { VStack } from '@astryxdesign/core/Layout'
import { TextInput } from '@astryxdesign/core/TextInput'

/**
 * Map the loopback address a provider redirected to onto this instance's OAuth
 * callback, keeping the authorization response it carries.
 */
export function oauthCallbackPathFromPastedUrl(value: string) {
  let url: URL
  try {
    url = new URL(value.trim())
  } catch {
    return null
  }

  const params = url.searchParams
  const hasAuthorizationResponse =
    params.has('error') || (params.has('code') && params.has('state'))
  return hasAuthorizationResponse ? `/mcps/oauth/callback${url.search}` : null
}

/** Finishes OAuth for providers that only redirect to a loopback address. */
export function OauthCallbackPaste() {
  const [value, setValue] = useState('')
  const [isInvalid, setIsInvalid] = useState(false)

  function finish() {
    const path = oauthCallbackPathFromPastedUrl(value)
    if (!path) {
      setIsInvalid(true)
      return
    }
    window.location.assign(path)
  }

  return (
    <VStack gap={3} hAlign="start">
      <TextInput
        label="Callback address"
        description="After you approve access, the provider's tab lands on a localhost address that fails to load. Copy it from the address bar and paste it here."
        value={value}
        onChange={(next) => {
          setValue(next)
          setIsInvalid(false)
        }}
        onKeyDown={(event) => {
          // The field sits inside the edit form; Enter must not save the MCP.
          if (event.key === 'Enter') {
            event.preventDefault()
            finish()
          }
        }}
        placeholder="http://localhost:…/callback?code=…&state=…"
        width="100%"
        autoComplete="off"
        status={
          isInvalid
            ? { type: 'error', message: 'Paste the full localhost address, including ?code=…' }
            : undefined
        }
      />
      <Button label="Finish connecting" variant="secondary" size="sm" onClick={finish} />
    </VStack>
  )
}
