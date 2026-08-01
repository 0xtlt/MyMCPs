import { useState } from 'react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { Badge } from '@astryxdesign/core/Badge'
import { Button } from '@astryxdesign/core/Button'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import {
  HStack,
  Layout,
  LayoutContent,
  LayoutFooter,
  StackItem,
  VStack,
} from '@astryxdesign/core/Layout'
import { Table, pixel, proportional, type TableColumn } from '@astryxdesign/core/Table'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'

type InviteRow = {
  id: number
  email: string
  role: string
  token: string
  acceptedAt: string | null
  expiresAt: string
  createdAt: string
  isUsable: boolean
} & Record<string, unknown>

export default function InvitesIndex({
  invites,
  appUrl,
}: {
  invites: InviteRow[]
  appUrl: string
}) {
  const [isCreateOpen, setIsCreateOpen] = useState(false)
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

  function openCreate() {
    setEmail('')
    setIsCreateOpen(true)
  }

  function inviteStatus(invite: InviteRow): {
    label: string
    variant: 'success' | 'info' | 'neutral'
  } {
    if (invite.acceptedAt) return { label: 'Accepted', variant: 'success' }
    if (invite.isUsable) return { label: 'Pending', variant: 'info' }
    return { label: 'Expired', variant: 'neutral' }
  }

  const columns: TableColumn<InviteRow>[] = [
    {
      key: 'email',
      header: 'Email',
      width: proportional(2),
      renderCell: (invite) => (
        <Text type="body" weight="bold">
          {invite.email}
        </Text>
      ),
    },
    {
      key: 'expiresAt',
      header: 'Expires',
      width: proportional(2),
      renderCell: (invite) => (
        <Text type="supporting" color="secondary">
          {new Date(invite.expiresAt).toLocaleString()}
        </Text>
      ),
    },
    {
      key: 'status',
      header: 'Status',
      width: pixel(120),
      renderCell: (invite) => {
        const status = inviteStatus(invite)
        return <Badge label={status.label} variant={status.variant} />
      },
    },
    {
      key: 'actions',
      header: 'Actions',
      width: pixel(120),
      align: 'end',
      renderCell: (invite) =>
        invite.isUsable ? (
          <Button
            label="Copy link"
            variant="secondary"
            size="sm"
            onClick={() => copyLink(invite.token)}
          />
        ) : (
          <Text type="supporting" color="secondary">
            —
          </Text>
        ),
    },
  ]

  return (
    <VStack gap={6} maxWidth={960} width="100%">
      <HStack gap={4} hAlign="between" vAlign="start">
        <StackItem size="fill">
          <VStack gap={2}>
            <Heading level={1}>Invites</Heading>
            <Text type="body" color="secondary">
              Invite teammates with a link. There is no public registration—only people you invite can
              join this instance.
            </Text>
          </VStack>
        </StackItem>
        <Button label="Create invite" variant="primary" onClick={openCreate} />
      </HStack>

      {invites.length === 0 ? (
        <Banner
          status="info"
          title="No invites yet"
          description="Create an invite link to add a teammate."
          container="card"
        />
      ) : (
        <Table
          data={invites}
          columns={columns}
          idKey="id"
          hasHover
          density="compact"
          textOverflow="truncate"
        />
      )}

      <Dialog
        isOpen={isCreateOpen}
        onOpenChange={setIsCreateOpen}
        purpose="form"
        width={480}
      >
        <Form route="invites.store">
          {({ errors, processing }) => (
            <Layout
              height="auto"
              header={
                <DialogHeader
                  title="Create invite"
                  subtitle="Send a one-time join link to a teammate"
                  onOpenChange={setIsCreateOpen}
                />
              }
              content={
                <LayoutContent>
                  <TextInput
                    label="Email"
                    type="email"
                    htmlName="email"
                    value={email}
                    onChange={setEmail}
                    placeholder="teammate@example.com"
                    width="100%"
                    status={errors.email ? { type: 'error', message: errors.email } : undefined}
                  />
                </LayoutContent>
              }
              footer={
                <LayoutFooter>
                  <HStack gap={2} hAlign="end">
                    <Button
                      label="Cancel"
                      variant="secondary"
                      onClick={() => setIsCreateOpen(false)}
                    />
                    <Button
                      type="submit"
                      label="Create invite"
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
    </VStack>
  )
}
