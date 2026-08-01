import { type Data } from '@generated/data'
import { toast, Toaster } from 'sonner'
import { usePage } from '@inertiajs/react'
import { type ReactElement, useEffect } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { AppShell } from '@astryxdesign/core/AppShell'
import { Button } from '@astryxdesign/core/Button'
import { HStack } from '@astryxdesign/core/Layout'
import { Icon } from '@astryxdesign/core/Icon'
import { NavIcon } from '@astryxdesign/core/NavIcon'
import { Text } from '@astryxdesign/core/Text'
import { TopNav, TopNavHeading, TopNavItem } from '@astryxdesign/core/TopNav'

export default function Layout({ children }: { children: ReactElement<Data.SharedProps> }) {
  const { url } = usePage()
  const user = children.props.user

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
                href="/"
                logo={<NavIcon icon={<Icon icon="wrench" size="sm" color="inherit" />} />}
              />
            }
            startContent={
              <TopNavItem label="Home" href="/" isSelected={url === '/'} />
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
              ) : (
                <HStack gap={2} vAlign="center">
                  <TopNavItem label="Signup" href="/signup" isSelected={url.startsWith('/signup')} />
                  <TopNavItem label="Login" href="/login" isSelected={url.startsWith('/login')} />
                </HStack>
              )
            }
          />
        }
      >
        {children}
      </AppShell>
      <Toaster position="top-center" richColors />
    </>
  )
}
