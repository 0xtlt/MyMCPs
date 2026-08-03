import { useState } from 'react'
import { Head } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { Center } from '@astryxdesign/core/Center'
import { VStack } from '@astryxdesign/core/Layout'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

export default function Login() {
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')

  return (
    <Center axis="both" style={{ minHeight: '100%' }}>
      <Head title="Sign in" />
      <VStack gap={4} hAlign="center" maxWidth={400} width="100%">
        <Card padding={8} width="100%">
          <Form route="session.store">
            {({ errors, processing }) => (
              <VStack gap={4} hAlign="stretch">
                <VStack gap={1} hAlign="center">
                  <Heading level={1}>Sign in</Heading>
                  <Text type="body" color="secondary" justify="center">
                    Access this self-hosted MyMCPs instance. New users join by invite only.
                  </Text>
                </VStack>

                {errors.email || errors.password ? (
                  <Banner
                    status="error"
                    title={errors.email || errors.password || 'Unable to sign in'}
                    container="card"
                  />
                ) : null}

                <TextInput
                  label="Email"
                  type="email"
                  htmlName="email"
                  value={email}
                  onChange={setEmail}
                  autoComplete="username"
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
                  autoComplete="current-password"
                  size="lg"
                  width="100%"
                  status={errors.password ? { type: 'error', message: errors.password } : undefined}
                />

                <Button
                  type="submit"
                  label="Sign in"
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
