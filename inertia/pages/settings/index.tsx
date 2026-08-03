import { useState } from 'react'
import { Head, usePage } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { type Data } from '@generated/data'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import { HStack, Layout, LayoutContent, LayoutFooter, VStack } from '@astryxdesign/core/Layout'
import { Section } from '@astryxdesign/core/Section'
import { NumberInput } from '@astryxdesign/core/NumberInput'
import { Selector } from '@astryxdesign/core/Selector'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

export default function SettingsIndex({
  mcpLogging,
}: {
  mcpLogging: {
    level: 'off' | 'metadata' | 'arguments' | 'responses'
    retentionDays: number
  } | null
}) {
  const { props } = usePage<Data.SharedProps>()
  const user = props.user!
  const [isEmailOpen, setIsEmailOpen] = useState(false)
  const [isPasswordOpen, setIsPasswordOpen] = useState(false)
  const [email, setEmail] = useState(user.email)
  const [emailCurrentPassword, setEmailCurrentPassword] = useState('')
  const [passwordCurrentPassword, setPasswordCurrentPassword] = useState('')
  const [newPassword, setNewPassword] = useState('')
  const [passwordConfirmation, setPasswordConfirmation] = useState('')
  const [mcpLogLevel, setMcpLogLevel] = useState(mcpLogging?.level ?? 'metadata')
  const [mcpLogRetentionDays, setMcpLogRetentionDays] = useState(mcpLogging?.retentionDays ?? 14)

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
      <Head title="Settings" />
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
            <Form route="settings.updateMcpLogging">
              {({ errors, processing }) => (
                <VStack gap={5} hAlign="stretch">
                  <VStack gap={1}>
                    <Heading level={2}>My Instance</Heading>
                    <Text type="body" color="secondary">
                      Configure settings that apply to everyone using this MyMCPs instance.
                    </Text>
                  </VStack>
                  <Banner
                    status="warning"
                    title="Arguments and responses can contain sensitive data"
                    description="Argument and response capture stores exact MCP JSON without redaction. Tool responses may contain secrets, personal data, or large payloads."
                    container="section"
                  />
                  <HStack gap={4} vAlign="start" wrap="wrap">
                    <Selector
                      label="Call logging level"
                      htmlName="mcpLogLevel"
                      value={mcpLogLevel}
                      onChange={(value) =>
                        setMcpLogLevel(value as 'off' | 'metadata' | 'arguments' | 'responses')
                      }
                      options={[
                        { value: 'off', label: 'Off' },
                        { value: 'metadata', label: 'Metadata' },
                        { value: 'arguments', label: 'Metadata + arguments' },
                        {
                          value: 'responses',
                          label: 'Metadata + arguments + responses',
                        },
                      ]}
                      description="Changes apply to future tool calls only."
                      width={280}
                      status={
                        errors.mcpLogLevel
                          ? { type: 'error', message: errors.mcpLogLevel }
                          : undefined
                      }
                    />
                    <NumberInput
                      label="Log retention"
                      htmlName="mcpLogRetentionDays"
                      value={mcpLogRetentionDays}
                      onChange={setMcpLogRetentionDays}
                      min={1}
                      max={365}
                      step={1}
                      units="days"
                      isIntegerOnly
                      description="Records older than this are deleted."
                      width={220}
                      status={
                        errors.mcpLogRetentionDays
                          ? { type: 'error', message: errors.mcpLogRetentionDays }
                          : undefined
                      }
                    />
                  </HStack>
                  <HStack gap={3} hAlign="end">
                    <Button
                      type="submit"
                      label="Save logging settings"
                      variant="primary"
                      isLoading={processing}
                    />
                  </HStack>
                </VStack>
              )}
            </Form>
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
