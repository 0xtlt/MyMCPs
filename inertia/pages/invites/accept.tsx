import { useState } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { Center } from '@astryxdesign/core/Center'
import { VStack } from '@astryxdesign/core/Layout'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

export default function AcceptInvite({ token, email }: { token: string; email: string }) {
  const [fullName, setFullName] = useState('')
  const [password, setPassword] = useState('')
  const [passwordConfirmation, setPasswordConfirmation] = useState('')

  return (
    <Center axis="both" style={{ minHeight: '100%' }}>
      <VStack gap={4} hAlign="center" maxWidth={400} width="100%">
        <Card padding={8} width="100%">
          <Form route="invites.accept" routeParams={{ token }}>
            {({ errors, processing }) => (
              <VStack gap={4} hAlign="stretch">
                <VStack gap={1} hAlign="center">
                  <Heading level={1}>Join MyMCPs</Heading>
                  <Text type="body" color="secondary" justify="center">
                    You were invited as {email}
                  </Text>
                </VStack>

                {errors.fullName || errors.password ? (
                  <Banner
                    status="error"
                    title={errors.fullName || errors.password || 'Unable to accept invite'}
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
                  label="Password"
                  type="password"
                  htmlName="password"
                  value={password}
                  onChange={setPassword}
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
                  label="Create account"
                  variant="primary"
                  size="lg"
                  width="100%"
                  isLoading={processing}
                />
              </VStack>
            )}
          </Form>
        </Card>
      </VStack>
    </Center>
  )
}
