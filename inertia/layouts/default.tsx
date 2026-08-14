import { type Data } from '@generated/data'
import { usePage } from '@inertiajs/react'
import { type ReactElement, useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { Form } from '@adonisjs/inertia/react'
import { AppShell, useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Avatar } from '@astryxdesign/core/Avatar'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Center } from '@astryxdesign/core/Center'
import { Divider } from '@astryxdesign/core/Divider'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Link } from '@astryxdesign/core/Link'
import { Toast } from '@astryxdesign/core/Toast'
import { TopNav, TopNavHeading, TopNavItem } from '@astryxdesign/core/TopNav'
import logoUrl from '~/assets/brand/mymcps-m-logo.png?w=64&format=png&quality=100&img'

function FlashToasts() {
  const page = usePage()
  const error = page.flash.error
  const success = page.flash.success
  const flashKey = `${page.url}:${error ?? ''}:${success ?? ''}`
  const [resolved, setResolved] = useState<{ key: string; host: HTMLElement } | null>(null)
  const [dismissedKey, setDismissedKey] = useState<string | null>(null)

  useEffect(() => {
    const timeout = setTimeout(() => {
      setResolved({
        key: flashKey,
        host: document.querySelector('dialog[open]') ?? document.body,
      })
    }, 0)
    return () => clearTimeout(timeout)
  }, [flashKey])

  const body = error || success
  const host = resolved?.key === flashKey ? resolved.host : null
  if (dismissedKey === flashKey || !body || !host) return null

  return createPortal(
    <HStack className="flash-toast" hAlign="end" data-flash-toast="">
      <Toast
        type={error ? 'error' : 'info'}
        body={body}
        isAutoHide={!error}
        autoHideDuration={5000}
        onDismiss={() => setDismissedKey(flashKey)}
      />
    </HStack>,
    host
  )
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
    <>
      <FlashToasts />
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
    </>
  )
}
