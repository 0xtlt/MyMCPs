import { useState } from 'react'
import { usePage } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { type Data } from '@generated/data'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import { HStack, Layout, LayoutContent, LayoutFooter, VStack } from '@astryxdesign/core/Layout'
import { Section } from '@astryxdesign/core/Section'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

export default function SettingsIndex() {
  const { props } = usePage<Data.SharedProps>()
  const user = props.user!
  const [isEmailOpen, setIsEmailOpen] = useState(false)
  const [isPasswordOpen, setIsPasswordOpen] = useState(false)
  const [email, setEmail] = useState(user.email)
  const [emailCurrentPassword, setEmailCurrentPassword] = useState('')
  const [passwordCurrentPassword, setPasswordCurrentPassword] = useState('')
  const [newPassword, setNewPassword] = useState('')
  const [passwordConfirmation, setPasswordConfirmation] = useState('')

  function openEmailDialog() {
    setEmail(user.email)
    setEmailCurrentPassword('')
    setIsEmailOpen(true)
  }

  function closeEmailDialogAfterSuccess() {
    setIsEmailOpen(false)
    setEmail('')
    setEmailCurrentPassword('')
  }

  function openPasswordDialog() {
    setPasswordCurrentPassword('')
    setNewPassword('')
    setPasswordConfirmation('')
    setIsPasswordOpen(true)
  }

  function closePasswordDialogAfterSuccess() {
    setIsPasswordOpen(false)
    setPasswordCurrentPassword('')
    setNewPassword('')
    setPasswordConfirmation('')
  }

  return (
    <>
      <VStack gap={6} maxWidth={720} width="100%">
        <VStack gap={2}>
          <Heading level={1}>Settings</Heading>
          <Text type="body" color="secondary">
            Manage your account and this MyMCPs instance.
          </Text>
        </VStack>

        <Section padding={6} width="100%" dividers={user.isAdmin ? ['bottom'] : undefined}>
          <VStack gap={5}>
            <VStack gap={1}>
              <Heading level={2}>My account</Heading>
              <Text type="body" color="secondary">
                Update the credentials used to sign in to MyMCPs.
              </Text>
            </VStack>

            <HStack gap={4} hAlign="between" vAlign="center" wrap="wrap">
              <VStack gap={1}>
                <Text type="label">Name</Text>
                <Text type="body" color="secondary">
                  {user.fullName || 'Not set'}
                </Text>
              </VStack>
            </HStack>

            <HStack gap={4} hAlign="between" vAlign="center" wrap="wrap">
              <VStack gap={1}>
                <Text type="label">Email</Text>
                <Text type="body" color="secondary">
                  {user.email}
                </Text>
              </VStack>
              <Button label="Change email" variant="secondary" onClick={openEmailDialog} />
            </HStack>

            <HStack gap={4} hAlign="between" vAlign="center" wrap="wrap">
              <VStack gap={1}>
                <Text type="label">Password</Text>
                <Text type="body" color="secondary">
                  Change your password whenever you need to secure your account.
                </Text>
              </VStack>
              <Button label="Change password" variant="secondary" onClick={openPasswordDialog} />
            </HStack>
          </VStack>
        </Section>

        {user.isAdmin ? (
          <Section padding={6} width="100%">
            <VStack gap={4}>
              <VStack gap={1}>
                <Heading level={2}>My Instance</Heading>
                <Text type="body" color="secondary">
                  Configure settings that apply to everyone using this MyMCPs instance.
                </Text>
              </VStack>
              <Banner
                status="warning"
                title="Work in progress"
                description="Instance settings are not available yet."
                container="card"
              />
            </VStack>
          </Section>
        ) : null}
      </VStack>

      <Dialog isOpen={isEmailOpen} onOpenChange={setIsEmailOpen} purpose="form" width={480}>
        <Form
          route="settings.updateEmail"
          className="dialog-form-fill"
          onSuccess={closeEmailDialogAfterSuccess}
        >
          {({ errors, processing }) => (
            <Layout
              header={
                <DialogHeader
                  title="Change email"
                  subtitle="Confirm the change with your current password"
                  onOpenChange={setIsEmailOpen}
                />
              }
              content={
                <LayoutContent isScrollable>
                  <VStack gap={4}>
                    <TextInput
                      label="New email"
                      type="email"
                      htmlName="email"
                      value={email}
                      onChange={setEmail}
                      autoComplete="email"
                      width="100%"
                      isRequired
                      status={errors.email ? { type: 'error', message: errors.email } : undefined}
                    />
                    <TextInput
                      label="Current password"
                      type="password"
                      htmlName="currentPassword"
                      value={emailCurrentPassword}
                      onChange={setEmailCurrentPassword}
                      autoComplete="current-password"
                      width="100%"
                      isRequired
                      status={
                        errors.currentPassword
                          ? { type: 'error', message: errors.currentPassword }
                          : undefined
                      }
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
                      onClick={() => setIsEmailOpen(false)}
                    />
                    <Button
                      type="submit"
                      label="Save email"
                      variant="primary"
                      isLoading={processing}
                    />
                  </HStack>
                </LayoutFooter>
              }
            />
          )}
        </Form>
      </Dialog>

      <Dialog isOpen={isPasswordOpen} onOpenChange={setIsPasswordOpen} purpose="form" width={480}>
        <Form
          route="settings.updatePassword"
          className="dialog-form-fill"
          onSuccess={closePasswordDialogAfterSuccess}
        >
          {({ errors, processing }) => (
            <Layout
              header={
                <DialogHeader
                  title="Change password"
                  subtitle="Use between 8 and 32 characters"
                  onOpenChange={setIsPasswordOpen}
                />
              }
              content={
                <LayoutContent isScrollable>
                  <VStack gap={4}>
                    <TextInput
                      label="Current password"
                      type="password"
                      htmlName="currentPassword"
                      value={passwordCurrentPassword}
                      onChange={setPasswordCurrentPassword}
                      autoComplete="current-password"
                      width="100%"
                      isRequired
                      status={
                        errors.currentPassword
                          ? { type: 'error', message: errors.currentPassword }
                          : undefined
                      }
                    />
                    <TextInput
                      label="New password"
                      type="password"
                      htmlName="newPassword"
                      value={newPassword}
                      onChange={setNewPassword}
                      autoComplete="new-password"
                      width="100%"
                      isRequired
                      status={
                        errors.newPassword
                          ? { type: 'error', message: errors.newPassword }
                          : undefined
                      }
                    />
                    <TextInput
                      label="Confirm new password"
                      type="password"
                      htmlName="passwordConfirmation"
                      value={passwordConfirmation}
                      onChange={setPasswordConfirmation}
                      autoComplete="new-password"
                      width="100%"
                      isRequired
                      status={
                        errors.passwordConfirmation
                          ? { type: 'error', message: errors.passwordConfirmation }
                          : undefined
                      }
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
                      onClick={() => setIsPasswordOpen(false)}
                    />
                    <Button
                      type="submit"
                      label="Save password"
                      variant="primary"
                      isLoading={processing}
                    />
                  </HStack>
                </LayoutFooter>
              }
            />
          )}
        </Form>
      </Dialog>
    </>
  )
}
