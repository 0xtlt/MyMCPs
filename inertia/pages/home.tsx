import { Button } from '@astryxdesign/core/Button'
import { Card } from '@astryxdesign/core/Card'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Heading, Text } from '@astryxdesign/core/Text'
import { usePage } from '@inertiajs/react'
import { type Data } from '@generated/data'
import mcpNetworkUrl from '~/assets/mcp-network.png?w=440&format=webp'

export default function Home() {
  const { props } = usePage<Data.SharedProps>()
  const user = props.user

  return (
    <VStack gap={6} maxWidth={720} width="100%">
      <VStack gap={2}>
        <Heading level={1}>Dashboard</Heading>
        <Text type="body" color="secondary">
          Signed in as {user?.fullName || user?.email}. Register upstream MCPs and issue access
          tokens for your agents.
        </Text>
      </VStack>

      <Card padding={6} width="100%" minHeight={220} className="mcp-hero-card">
        <VStack gap={4} hAlign="start" className="mcp-hero-card__content">
          <VStack gap={1}>
            <Heading level={2}>MCPs</Heading>
            <Text type="body" color="secondary">
              Add HTTP or npm upstream servers, configure auth, and inspect connection status.
            </Text>
          </VStack>
          <Button label="Manage MCPs" variant="primary" href="/mcps" />
        </VStack>
        <img
          src={mcpNetworkUrl}
          alt=""
          className="mcp-hero-card__art"
        />
      </Card>

      <Card padding={6} width="100%">
        <HStack gap={4} hAlign="between" vAlign="center" wrap="wrap">
          <VStack gap={1}>
            <Heading level={2}>Access tokens</Heading>
            <Text type="body" color="secondary">
              Create identifiers with access to all MCPs or a selected subset for the /mcp gateway.
            </Text>
          </VStack>
          <Button label="Manage tokens" variant="primary" href="/tokens" />
        </HStack>
      </Card>

      {user?.isAdmin ? (
        <Card padding={6} width="100%">
          <HStack gap={4} hAlign="between" vAlign="center" wrap="wrap">
            <VStack gap={1}>
              <Heading level={2}>Teammates</Heading>
              <Text type="body" color="secondary">
                Invite people to this instance. There is no public registration.
              </Text>
            </VStack>
            <Button label="Manage invites" variant="secondary" href="/invites" />
          </HStack>
        </Card>
      ) : null}
    </VStack>
  )
}
