import { type Data } from '@generated/data'
import { toast, Toaster } from 'sonner'
import { usePage } from '@inertiajs/react'
import { type ReactElement, useEffect } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { AppShell } from '@astryxdesign/core/AppShell'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Center } from '@astryxdesign/core/Center'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Icon } from '@astryxdesign/core/Icon'
import { NavIcon } from '@astryxdesign/core/NavIcon'
import { Text } from '@astryxdesign/core/Text'
import { TopNav, TopNavHeading, TopNavItem } from '@astryxdesign/core/TopNav'

export default function Layout({ children }: { children: ReactElement<Data.SharedProps> }) {
  const page = usePage<Data.SharedProps>()
  const { url } = page
  const user = children.props.user
  const setupComplete = children.props.setupComplete ?? true
  const isOnboarding = url.startsWith('/onboarding')
  const isAuthScreen = isOnboarding || url.startsWith('/login') || url.startsWith('/invite/')

  useEffect(() => {
    toast.dismiss()
  }, [url])

  useEffect(() => {
    if (children.props.flash.error) {
      toast.error(children.props.flash.error)
    }
    if (children.props.flash.success) {
      toast.success(children.props.flash.success)
    }
  })

  return (
    <>
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
                    <TopNavItem
                      label="Invites"
                      href="/invites"
                      isSelected={url.startsWith('/invites')}
                    />
                  ) : null}
                </>
              ) : undefined
            }
            endContent={
              user ? (
                <HStack gap={2} vAlign="center">
                  <Text type="label" color="secondary">
                    {user.initials}
                  </Text>
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
      <Toaster position="top-center" richColors />
    </>
  )
}
