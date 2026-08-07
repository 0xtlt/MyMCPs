import { Head } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Heading, Text } from '@astryxdesign/core/Text'
import { Token } from '@astryxdesign/core/Token'

type AuthorizationProps = {
  clientName: string
  redirectHost: string
  isLoopbackRedirect: boolean
  scope: string
  userEmail: string
  authorization: {
    clientId: string
    redirectUri: string
    state: string | null
    codeChallenge: string
    resource: string
  }
}

export default function Authorize({
  clientName,
  redirectHost,
  isLoopbackRedirect,
  scope,
  userEmail,
  authorization,
}: AuthorizationProps) {
  return (
    <VStack gap={6} maxWidth={520} width="100%">
      <Head title="Authorize MCP client" />
      <VStack gap={2} hAlign="center">
        <Heading level={1}>Authorize {clientName}</Heading>
        <Text type="body" color="secondary" justify="center">
          Signed in as {userEmail}
        </Text>
      </VStack>

      <Card padding={8} width="100%">
        <Form action={{ url: '/authorize', method: 'post' }}>
          {({ processing }) => (
            <VStack gap={5} hAlign="stretch">
              <VStack gap={2}>
                <Heading level={2}>Allow access to MyMCPs?</Heading>
                <Text type="body" color="secondary">
                  This client will be able to call every enabled MCP through your gateway. You can
                  revoke the connection at any time from Access tokens.
                </Text>
              </VStack>

              <VStack gap={2}>
                <Text type="label">Permission</Text>
                <Token label={scope} color="blue" />
                <Text type="supporting" color="secondary">
                  Callback: {redirectHost}
                </Text>
              </VStack>

              {isLoopbackRedirect ? (
                <Banner
                  status="warning"
                  title="Local callback"
                  description={`After approval, the authorization code will be sent to ${redirectHost}. Only continue if you started this connection from an MCP client on this device.`}
                  container="card"
                />
              ) : null}

              <input type="hidden" name="client_id" value={authorization.clientId} />
              <input type="hidden" name="redirect_uri" value={authorization.redirectUri} />
              <input type="hidden" name="response_type" value="code" />
              <input type="hidden" name="code_challenge" value={authorization.codeChallenge} />
              <input type="hidden" name="code_challenge_method" value="S256" />
              <input type="hidden" name="scope" value={scope} />
              <input type="hidden" name="resource" value={authorization.resource} />
              {authorization.state ? (
                <input type="hidden" name="state" value={authorization.state} />
              ) : null}

              <HStack gap={2} hAlign="end" wrap="wrap">
                <Button
                  type="submit"
                  name="decision"
                  value="deny"
                  label="Cancel"
                  variant="secondary"
                  isDisabled={processing}
                />
                <Button
                  type="submit"
                  name="decision"
                  value="approve"
                  label="Authorize client"
                  variant="primary"
                  isLoading={processing}
                />
              </HStack>
            </VStack>
          )}
        </Form>
      </Card>
    </VStack>
  )
}
