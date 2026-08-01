import { useState } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { Center } from '@astryxdesign/core/Center'
import { VStack } from '@astryxdesign/core/Layout'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

export default function Onboarding() {
  const [fullName, setFullName] = useState('')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [passwordConfirmation, setPasswordConfirmation] = useState('')

  return (
    <Center axis="both" style={{ minHeight: '100%' }}>
      <Card padding={8} maxWidth={420} width="100%">
        <Form route="onboarding.store">
          {({ errors, processing }) => (
            <VStack gap={4} hAlign="stretch">
              <VStack gap={1}>
                <Heading level={1}>Set up MyMCPs</Heading>
                <Text type="body" color="secondary">
                  Create the admin account for this self-hosted instance.
                </Text>
              </VStack>

              {errors.email || errors.password || errors.fullName ? (
                <Banner
                  status="error"
                  title={
                    errors.fullName ||
                    errors.email ||
                    errors.password ||
                    'Unable to complete setup'
                  }
                  container="card"
                />
              ) : null}

              <TextInput
                label="Full name"
                htmlName="fullName"
                value={fullName}
                onChange={setFullName}
                size="lg"
                width="100%"
                status={errors.fullName ? { type: 'error', message: errors.fullName } : undefined}
              />

              <TextInput
                label="Email"
                type="email"
                htmlName="email"
                value={email}
                onChange={setEmail}
                autoComplete="email"
                size="lg"
                width="100%"
                status={errors.email ? { type: 'error', message: errors.email } : undefined}
              />

              <TextInput
                label="Password"
                type="password"
                htmlName="password"
                value={password}
                onChange={setPassword}
                autoComplete="new-password"
                size="lg"
                width="100%"
                status={errors.password ? { type: 'error', message: errors.password } : undefined}
              />

              <TextInput
                label="Confirm password"
                type="password"
                htmlName="passwordConfirmation"
                value={passwordConfirmation}
                onChange={setPasswordConfirmation}
                autoComplete="new-password"
                size="lg"
                width="100%"
                status={
                  errors.passwordConfirmation
                    ? { type: 'error', message: errors.passwordConfirmation }
                    : undefined
                }
              />

              <Button
                type="submit"
                label="Create admin"
                variant="primary"
                size="lg"
                width="100%"
                isLoading={processing}
              />
            </VStack>
          )}
        </Form>
      </Card>
    </Center>
  )
}
