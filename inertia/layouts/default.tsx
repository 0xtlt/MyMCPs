import { type Data } from '@generated/data'
import { usePage } from '@inertiajs/react'
import { type ReactElement, useEffect, useRef } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { AppShell, useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Avatar } from '@astryxdesign/core/Avatar'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Center } from '@astryxdesign/core/Center'
import { Divider } from '@astryxdesign/core/Divider'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Link } from '@astryxdesign/core/Link'
import { LayerProvider } from '@astryxdesign/core/Layer'
import { useToast } from '@astryxdesign/core/Toast'
import { TopNav, TopNavHeading, TopNavItem } from '@astryxdesign/core/TopNav'
import logoUrl from '~/assets/brand/mymcps-m-logo.png?w=64&format=png&quality=100&img'

const TOAST_VIEWPORT_SELECTOR = '[role="region"][popover], [role="region"][data-toast-dialog-host]'

let toastViewportPlaceholder: Comment | null = null

function toastViewportElement() {
  return document.querySelector<HTMLElement>(TOAST_VIEWPORT_SELECTOR)
}

function toastIsAlreadyOnTop(toast: Element) {
  const rect = toast.getBoundingClientRect()
  if (rect.width <= 0 || rect.height <= 0) {
    return false
  }

  const topEl = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2)
  return Boolean(topEl && toast.contains(topEl))
}

function restoreToastViewport() {
  const viewport = toastViewportElement()
  if (!viewport || !toastViewportPlaceholder?.parentNode) {
    return
  }
  if (viewport.parentNode === toastViewportPlaceholder.parentNode) {
    return
  }

  toastViewportPlaceholder.parentNode.insertBefore(viewport, toastViewportPlaceholder)
  toastViewportPlaceholder.remove()
  toastViewportPlaceholder = null
  viewport.removeAttribute('data-toast-dialog-host')
  if (!viewport.hasAttribute('popover')) {
    viewport.setAttribute('popover', 'manual')
  }
  try {
    viewport.showPopover()
  } catch {
    // Already showing, or the Popover API is unavailable.
  }
}

function promoteToastViewportAboveDialogs() {
  const dialog = document.querySelector('dialog[open]')
  const viewport = toastViewportElement()
  if (!viewport) {
    return
  }
  if (!dialog) {
    restoreToastViewport()
    return
  }

  const toast = viewport.querySelector('[data-toast-id]')
  if (!toast) {
    return
  }
  if (dialog.contains(viewport) && toastIsAlreadyOnTop(toast)) {
    return
  }

  if (!dialog.contains(viewport)) {
    if (!toastViewportPlaceholder) {
      toastViewportPlaceholder = document.createComment('toast-viewport')
      viewport.parentNode?.insertBefore(toastViewportPlaceholder, viewport)
    }
    try {
      viewport.hidePopover()
    } catch {
      // Not open yet.
    }
    dialog.appendChild(viewport)
    viewport.setAttribute('data-toast-dialog-host', '')
    viewport.removeAttribute('popover')
  }
}

function ToastTopLayer() {
  useEffect(() => {
    let frame = 0
    let timeout: ReturnType<typeof setTimeout> | undefined

    const schedulePromote = () => {
      cancelAnimationFrame(frame)
      if (timeout !== undefined) {
        clearTimeout(timeout)
      }
      frame = requestAnimationFrame(() => {
        promoteToastViewportAboveDialogs()
        timeout = setTimeout(promoteToastViewportAboveDialogs, 50)
      })
    }

    const observer = new MutationObserver(schedulePromote)
    observer.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ['open'],
    })
    schedulePromote()

    return () => {
      observer.disconnect()
      cancelAnimationFrame(frame)
      if (timeout !== undefined) {
        clearTimeout(timeout)
      }
      restoreToastViewport()
    }
  }, [])

  return null
}

function FlashToasts({ children }: { children: ReactElement<Data.SharedProps> }) {
  const page = usePage<Data.SharedProps>()
  const showToast = useToast()
  const dismissToasts = useRef<Array<() => void>>([])

  useEffect(() => {
    for (const dismiss of dismissToasts.current) dismiss()
    dismissToasts.current = []

    if (children.props.flash.error) {
      dismissToasts.current.push(
        showToast({ body: children.props.flash.error, type: 'error', uniqueID: 'flash-error' })
      )
    }
    if (children.props.flash.success) {
      dismissToasts.current.push(
        showToast({ body: children.props.flash.success, type: 'info', uniqueID: 'flash-success' })
      )
    }

    requestAnimationFrame(() => {
      requestAnimationFrame(promoteToastViewportAboveDialogs)
    })
  }, [children.props.flash.error, children.props.flash.success, page.url, showToast])

  return null
}

function DesktopAccountActions({ user }: { user: NonNullable<Data.SharedProps['user']> }) {
  const { isMobile } = useAppShellMobile()

  if (isMobile) return null

  return (
    <HStack gap={2} vAlign="center">
      <Link href="/settings" isStandalone>
        <HStack gap={1} vAlign="center">
          <Avatar name={user.fullName || user.initials} size="sm" tooltip={false} />
          Settings
        </HStack>
      </Link>
      <Form route="session.destroy">
        <Button type="submit" label="Log out" variant="ghost" size="sm" />
      </Form>
    </HStack>
  )
}

function MobileAccountActions({
  user,
  url,
}: {
  user: NonNullable<Data.SharedProps['user']>
  url: string
}) {
  const { isMobile, closeMobileNav } = useAppShellMobile()

  if (!isMobile) return null

  return (
    <VStack gap={2} hAlign="stretch">
      <Divider />
      <TopNavItem
        label={`${user.fullName || user.email} · Settings`}
        href="/settings"
        isSelected={url.startsWith('/settings')}
      />
      <Form route="session.destroy">
        <Button
          type="submit"
          label="Log out"
          variant="ghost"
          width="100%"
          onClick={closeMobileNav}
        />
      </Form>
    </VStack>
  )
}

export default function Layout({ children }: { children: ReactElement<Data.SharedProps> }) {
  const page = usePage<Data.SharedProps>()
  const { url } = page
  const user = children.props.user
  const setupComplete = children.props.setupComplete ?? true
  const isOnboarding = url.startsWith('/onboarding')
  const isAuthScreen = isOnboarding || url.startsWith('/login') || url.startsWith('/invite/')

  return (
    <LayerProvider toast={{ position: 'topEnd' }}>
      <FlashToasts>{children}</FlashToasts>
      <ToastTopLayer />
      <AppShell
        contentPadding={6}
        style={{ height: '100%', minHeight: 0 }}
        topNav={
          <TopNav
            label="Main navigation"
            heading={
              <TopNavHeading
                heading="MyMCPs"
                headingHref={user ? '/' : undefined}
                logo={<img src={logoUrl} alt="" className="app-logo" />}
              />
            }
            startContent={
              user ? (
                <>
                  <TopNavItem label="Home" href="/" isSelected={url === '/'} />
                  <TopNavItem label="MCPs" href="/mcps" isSelected={url.startsWith('/mcps')} />
                  <TopNavItem
                    label="Tokens"
                    href="/tokens"
                    isSelected={url.startsWith('/tokens')}
                  />
                  {user.isAdmin ? (
                    <>
                      <TopNavItem label="Logs" href="/logs" isSelected={url.startsWith('/logs')} />
                      <TopNavItem
                        label="Analytics"
                        href="/analytics"
                        isSelected={url.startsWith('/analytics')}
                      />
                      <TopNavItem
                        label="Invites"
                        href="/invites"
                        isSelected={url.startsWith('/invites')}
                      />
                    </>
                  ) : null}
                  <MobileAccountActions user={user} url={url} />
                </>
              ) : undefined
            }
            endContent={
              user ? (
                <DesktopAccountActions user={user} />
              ) : setupComplete && !isAuthScreen ? (
                <TopNavItem label="Login" href="/login" isSelected={url.startsWith('/login')} />
              ) : undefined
            }
          />
        }
      >
        <VStack gap={4} width="100%" hAlign="stretch">
          {user && !children.props.appUrlConfigured ? (
            <Banner
              status="warning"
              title="Set APP_URL to enable public links"
              description="Define APP_URL as this instance’s public HTTPS origin, then redeploy."
              container="section"
            />
          ) : null}
          <Center axis="horizontal" width="100%">
            {children}
          </Center>
        </VStack>
      </AppShell>
    </LayerProvider>
  )
}
