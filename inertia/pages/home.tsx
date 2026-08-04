import { Button } from '@astryxdesign/core/Button'
import { Divider } from '@astryxdesign/core/Divider'
import { HStack, StackItem, VStack } from '@astryxdesign/core/Layout'
import { Section } from '@astryxdesign/core/Section'
import { Heading, Text } from '@astryxdesign/core/Text'
import { Head, usePage } from '@inertiajs/react'
import { type Data } from '@generated/data'

function DashboardAction({
  title,
  description,
  label,
  href,
  primary = false,
}: {
  title: string
  description: string
  label: string
  href: string
  primary?: boolean
}) {
  return (
    <Section padding={4} width="100%">
      <HStack gap={4} hAlign="between" vAlign="center" wrap="wrap">
        <StackItem size="fill">
          <VStack gap={1}>
            <Heading level={2}>{title}</Heading>
            <Text type="body" color="secondary">
              {description}
            </Text>
          </VStack>
        </StackItem>
        <Button label={label} variant={primary ? 'primary' : 'secondary'} href={href} />
      </HStack>
    </Section>
  )
}

export default function Home() {
  const { props } = usePage<Data.SharedProps>()
  const user = props.user

  return (
    <VStack gap={6} maxWidth={720} width="100%">
      <Head title="Dashboard" />
      <VStack gap={2}>
        <Heading level={1}>Dashboard</Heading>
        <Text type="body" color="secondary">
          Signed in as {user?.fullName || user?.email}. Register upstream MCPs and issue access
          tokens for your agents.
        </Text>
      </VStack>

      <VStack gap={0} width="100%">
        <DashboardAction
          title="MCPs"
          description="Add upstream servers, configure auth, and inspect connection status."
          label="Manage MCPs"
          href="/mcps"
          primary
        />
        <Divider />
        <DashboardAction
          title="Access tokens"
          description="Issue scoped identifiers for agents using the /mcp gateway."
          label="Manage tokens"
          href="/tokens"
          primary
        />

        {user?.isAdmin ? (
          <>
            <Divider />
            <DashboardAction
              title="Teammates"
              description="Invite people to this instance. There is no public registration."
              label="Manage invites"
              href="/invites"
            />
          </>
        ) : null}
      </VStack>
    </VStack>
  )
}
