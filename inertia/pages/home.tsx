import { Button } from '@astryxdesign/core/Button'
import { Grid } from '@astryxdesign/core/Grid'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { Heading, Text } from '@astryxdesign/core/Text'
import { Icon } from '@astryxdesign/core/Icon'
import { Section } from '@astryxdesign/core/Section'

export default function Home() {
  return (
    <VStack gap={10} hAlign="center">
      <VStack gap={6} hAlign="center" maxWidth={720}>
        <VStack gap={3} hAlign="center">
          <Text type="label" color="accent">
            AdonisJS · Inertia · Astryx
          </Text>
          <Heading level={1} type="display-2" justify="center" textWrap="balance">
            Start building with a full-stack React foundation
          </Heading>
          <Text type="large" color="secondary" justify="center" textWrap="balance">
            Server-driven routing, typed forms, and an agent-ready design system — ready for your
            next product screen.
          </Text>
        </VStack>

        <HStack gap={3} wrap="wrap" hAlign="center">
          <Button label="Create account" variant="primary" href="/signup" size="lg" />
          <Button
            label="Astryx docs"
            variant="secondary"
            href="https://astryx.atmeta.com/"
            size="lg"
            endContent={<Icon icon="externalLink" size="sm" color="inherit" />}
          />
        </HStack>
      </VStack>

      <Section variant="muted" padding={6} maxWidth={960} width="100%">
        <Grid columns={{ minWidth: 220, max: 3 }} gap={6}>
          <VStack gap={2}>
            <Heading level={2}>AdonisJS</Heading>
            <Text type="body" color="secondary">
              Controllers, Lucid, Vine validation, and session auth out of the box.
            </Text>
          </VStack>
          <VStack gap={2}>
            <Heading level={2}>Inertia React</Heading>
            <Text type="body" color="secondary">
              Build SPA screens with server routes — no separate API client required.
            </Text>
          </VStack>
          <VStack gap={2}>
            <Heading level={2}>Astryx</Heading>
            <Text type="body" color="secondary">
              Accessible components, tokens, and templates as the only UI system.
            </Text>
          </VStack>
        </Grid>
      </Section>
    </VStack>
  )
}
