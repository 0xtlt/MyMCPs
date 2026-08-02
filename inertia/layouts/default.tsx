import { type Data } from '@generated/data'
import { usePage } from '@inertiajs/react'
import { type ReactElement, useEffect, useRef } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { AppShell } from '@astryxdesign/core/AppShell'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Center } from '@astryxdesign/core/Center'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Icon } from '@astryxdesign/core/Icon'
import { Link } from '@astryxdesign/core/Link'
import { LayerProvider } from '@astryxdesign/core/Layer'
import { NavIcon } from '@astryxdesign/core/NavIcon'
import { useToast } from '@astryxdesign/core/Toast'
import { TopNav, TopNavHeading, TopNavItem } from '@astryxdesign/core/TopNav'

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
  }, [children.props.flash.error, children.props.flash.success, page.url, showToast])

  return null
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
      <AppShell
        contentPadding={6}
        style={{ height: '100%', minHeight: 0 }}
        topNav={
          <TopNav
            label="Main navigation"
            heading={
              <TopNavHeading
                heading="MyMCPs"
                href={user ? '/' : undefined}
                logo={<NavIcon icon={<Icon icon="wrench" size="sm" color="inherit" />} />}
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
                </>
              ) : undefined
            }
            endContent={
              user ? (
                <HStack gap={2} vAlign="center">
                  <Link href="/settings" isStandalone label="Open settings">
                    {user.initials}
                  </Link>
                  <Form route="session.destroy">
                    <Button type="submit" label="Log out" variant="ghost" size="sm" />
                  </Form>
                </HStack>
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
