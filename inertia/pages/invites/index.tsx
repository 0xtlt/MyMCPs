import { useState } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'
import { Badge } from '@astryxdesign/core/Badge'

type InviteRow = {
  id: number
  email: string
  role: string
  token: string
  acceptedAt: string | null
  expiresAt: string
  createdAt: string
  isUsable: boolean
}

export default function InvitesIndex({
  invites,
  appUrl,
}: {
  invites: InviteRow[]
  appUrl: string
}) {
  const [email, setEmail] = useState('')

  function inviteUrl(token: string) {
    return `${appUrl}/invite/${token}`
  }

  async function copyLink(token: string) {
    try {
      await navigator.clipboard.writeText(inviteUrl(token))
    } catch {
      // ignore clipboard failures
    }
  }

  return (
    <VStack gap={6} maxWidth={720} width="100%">
      <VStack gap={2}>
        <Heading level={1}>Invites</Heading>
        <Text type="body" color="secondary">
          Invite teammates with a link. There is no public registration—only people you invite can
          join this instance.
        </Text>
      </VStack>

      <Card padding={6} width="100%">
        <Form route="invites.store">
          {({ errors, processing }) => (
            <VStack gap={4} hAlign="stretch">
              <Heading level={2}>Create invite</Heading>
              <HStack gap={3} vAlign="end" wrap="wrap">
                <TextInput
                  label="Email"
                  type="email"
                  htmlName="email"
                  value={email}
                  onChange={setEmail}
                  placeholder="teammate@example.com"
                  width={320}
                  status={errors.email ? { type: 'error', message: errors.email } : undefined}
                />
                <Button
                  type="submit"
                  label="Create invite"
                  variant="primary"
                  isLoading={processing}
                />
              </HStack>
            </VStack>
          )}
        </Form>
      </Card>

      <VStack gap={3} hAlign="stretch">
        <Heading level={2}>Pending and past invites</Heading>
        {invites.length === 0 ? (
          <Banner status="info" title="No invites yet" container="card" />
        ) : (
          invites.map((invite) => (
            <Card key={invite.id} padding={4} width="100%">
              <HStack gap={4} hAlign="between" vAlign="center" wrap="wrap">
                <VStack gap={1}>
                  <Text type="body" weight="bold">
                    {invite.email}
                  </Text>
                  <Text type="supporting" color="secondary">
                    Expires {new Date(invite.expiresAt).toLocaleString()}
                  </Text>
                </VStack>
                <HStack gap={2} vAlign="center">
                  {invite.acceptedAt ? (
                    <Badge label="Accepted" variant="success" />
                  ) : invite.isUsable ? (
                    <Badge label="Pending" variant="info" />
                  ) : (
                    <Badge label="Expired" variant="neutral" />
                  )}
                  {invite.isUsable ? (
                    <Button
                      label="Copy link"
                      variant="secondary"
                      size="sm"
                      onClick={() => copyLink(invite.token)}
                    />
                  ) : null}
                </HStack>
              </HStack>
            </Card>
          ))
        )}
      </VStack>
    </VStack>
  )
}
