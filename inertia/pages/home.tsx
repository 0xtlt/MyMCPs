import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Heading, Text } from '@astryxdesign/core/Text'
import { usePage } from '@inertiajs/react'
import { type Data } from '@generated/data'

export default function Home() {
  const { props } = usePage<Data.SharedProps>()
  const user = props.user

  return (
    <VStack gap={6} maxWidth={720} width="100%">
      <VStack gap={2}>
        <Heading level={1}>Dashboard</Heading>
        <Text type="body" color="secondary">
          Signed in as {user?.fullName || user?.email}. This instance is ready—connect upstream MCPs
          next (coming soon).
        </Text>
      </VStack>

      {user?.isAdmin ? (
        <Card padding={6} width="100%">
          <HStack gap={4} hAlign="between" vAlign="center" wrap="wrap">
            <VStack gap={1}>
              <Heading level={2}>Teammates</Heading>
              <Text type="body" color="secondary">
                Invite people to this instance. There is no public registration.
              </Text>
            </VStack>
            <Button label="Manage invites" variant="primary" href="/invites" />
          </HStack>
        </Card>
      ) : null}
    </VStack>
  )
}
