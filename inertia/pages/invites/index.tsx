import { useState } from 'react'
import { Head, router } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Badge } from '@astryxdesign/core/Badge'
import { Button } from '@astryxdesign/core/Button'
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog'
import { DropdownMenu } from '@astryxdesign/core/DropdownMenu'
import {
  HStack,
  Layout,
  LayoutContent,
  LayoutFooter,
  StackItem,
  VStack,
} from '@astryxdesign/core/Layout'
import { List, ListItem } from '@astryxdesign/core/List'
import { SegmentedControl, SegmentedControlItem } from '@astryxdesign/core/SegmentedControl'
import { Table, pixel, proportional, type TableColumn } from '@astryxdesign/core/Table'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Heading, Text } from '@astryxdesign/core/Text'
import { formatLocalDate, formatLocalDateTime } from '~/components/local_time'

type TeamSection = 'members' | 'invites'

type InviteRow = {
  id: number
  email: string
  role: string
  token: string
  acceptedAt: string | null
  expiresAt: string | null
  createdAt: string | null
  isUsable: boolean
}

type MemberRow = {
  id: number
  email: string
  fullName: string | null
  role: string
  createdAt: string | null
  isCurrentUser: boolean
}

export default function InvitesIndex({
  invites,
  members,
  appUrl,
}: {
  invites: InviteRow[]
  members: MemberRow[]
  appUrl: string | null
}) {
  const { isMobile } = useAppShellMobile()
  const [isCreateOpen, setIsCreateOpen] = useState(false)
  const [email, setEmail] = useState('')
  const [section, setSection] = useState<TeamSection>('members')

  function inviteUrl(token: string) {
    if (!appUrl) return null
    return `${appUrl}/invite/${token}`
  }

  async function copyLink(token: string) {
    const url = inviteUrl(token)
    if (!url) return
    try {
      await navigator.clipboard.writeText(url)
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

  const inviteColumns: TableColumn<InviteRow>[] = [
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
          {invite.expiresAt ? formatLocalDateTime(invite.expiresAt) : '—'}
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
      width: pixel(200),
      align: 'end',
      renderCell: (invite) => (
        <HStack gap={2} hAlign="end">
          {invite.isUsable ? (
            <Button
              label="Copy link"
              variant="secondary"
              size="sm"
              isDisabled={!appUrl}
              tooltip={!appUrl ? 'Set APP_URL to enable public links' : undefined}
              onClick={() => copyLink(invite.token)}
            />
          ) : null}
          <Button
            label="Remove"
            variant="destructive"
            size="sm"
            onClick={() =>
              router.delete(`/invites/${invite.id}`, {
                preserveScroll: true,
              })
            }
          />
        </HStack>
      ),
    },
  ]

  const memberColumns: TableColumn<MemberRow>[] = [
    {
      key: 'name',
      header: 'Name',
      width: proportional(2),
      renderCell: (member) => (
        <VStack gap={0}>
          <Text type="body" weight="bold">
            {member.fullName || member.email}
          </Text>
          {member.fullName ? (
            <Text type="supporting" color="secondary">
              {member.email}
            </Text>
          ) : null}
        </VStack>
      ),
    },
    {
      key: 'role',
      header: 'Role',
      width: pixel(120),
      renderCell: (member) => (
        <Badge label={member.role} variant={member.role === 'admin' ? 'info' : 'neutral'} />
      ),
    },
    {
      key: 'createdAt',
      header: 'Joined',
      width: proportional(2),
      renderCell: (member) => (
        <Text type="supporting" color="secondary">
          {member.createdAt ? formatLocalDateTime(member.createdAt) : '—'}
        </Text>
      ),
    },
    {
      key: 'actions',
      header: 'Actions',
      width: pixel(120),
      align: 'end',
      renderCell: (member) =>
        member.isCurrentUser ? (
          <Text type="supporting" color="secondary">
            You
          </Text>
        ) : (
          <Button
            label="Remove"
            variant="destructive"
            size="sm"
            onClick={() =>
              router.delete(`/members/${member.id}`, {
                preserveScroll: true,
              })
            }
          />
        ),
    },
  ]

  return (
    <VStack gap={6} maxWidth={960} width="100%">
      <Head title="Invites" />
      <HStack gap={4} hAlign="between" vAlign="start" wrap="wrap">
        <StackItem size="fill">
          <VStack gap={2}>
            <Heading level={1}>Invites</Heading>
            <Text type="body" color="secondary">
              Invite teammates with a link. There is no public registration—only people you invite
              can join this instance.
            </Text>
          </VStack>
        </StackItem>
        <Button label="Create invite" variant="primary" onClick={openCreate} />
      </HStack>

      <VStack gap={4} hAlign="stretch">
        <StackItem size="static" crossAlignSelf="start">
          <SegmentedControl
            value={section}
            onChange={(value) => setSection(value as TeamSection)}
            label="Team section"
            layout="hug"
          >
            <SegmentedControlItem value="members" label={`Members (${members.length})`} />
            <SegmentedControlItem value="invites" label={`Invites (${invites.length})`} />
          </SegmentedControl>
        </StackItem>

        {section === 'members' ? (
          members.length === 0 ? (
            <Banner status="info" title="No members yet" container="card" />
          ) : isMobile ? (
            <List header="Members" density="compact" hasDividers>
              {members.map((member) => (
                <ListItem
                  key={member.id}
                  label={member.fullName || member.email}
                  description={member.fullName ? member.email : 'Member of this instance'}
                  endContent={
                    <VStack gap={1} hAlign="end">
                      <Badge
                        label={member.role}
                        variant={member.role === 'admin' ? 'info' : 'neutral'}
                      />
                      {member.isCurrentUser ? (
                        <Text type="supporting" color="secondary">
                          You
                        </Text>
                      ) : (
                        <DropdownMenu
                          button={{
                            label: `Actions for ${member.fullName || member.email}`,
                            children: 'Actions',
                            variant: 'secondary',
                            size: 'sm',
                          }}
                          items={[
                            {
                              label: 'Remove member',
                              onClick: () =>
                                router.delete(`/members/${member.id}`, {
                                  preserveScroll: true,
                                }),
                            },
                          ]}
                        />
                      )}
                    </VStack>
                  }
                />
              ))}
            </List>
          ) : (
            <Table
              data={members}
              columns={memberColumns}
              idKey="id"
              hasHover
              density="compact"
              textOverflow="truncate"
            />
          )
        ) : invites.length === 0 ? (
          <Banner
            status="info"
            title="No invites yet"
            description="Create an invite link to add a teammate."
            container="card"
          />
        ) : isMobile ? (
          <List header="Pending and past invites" density="compact" hasDividers>
            {invites.map((invite) => {
              const status = inviteStatus(invite)
              return (
                <ListItem
                  key={invite.id}
                  label={invite.email}
                  description={
                    invite.expiresAt
                      ? `Expires ${formatLocalDate(invite.expiresAt)}`
                      : 'No expiration date'
                  }
                  endContent={
                    <VStack gap={1} hAlign="end">
                      <Badge label={status.label} variant={status.variant} />
                      <DropdownMenu
                        button={{
                          label: `Actions for ${invite.email}`,
                          children: 'Actions',
                          variant: 'secondary',
                          size: 'sm',
                        }}
                        items={[
                          ...(invite.isUsable
                            ? [
                                {
                                  label: 'Copy link',
                                  onClick: () => copyLink(invite.token),
                                  isDisabled: !appUrl,
                                },
                              ]
                            : []),
                          {
                            label: 'Remove invite',
                            onClick: () =>
                              router.delete(`/invites/${invite.id}`, {
                                preserveScroll: true,
                              }),
                          },
                        ]}
                      />
                    </VStack>
                  }
                />
              )
            })}
          </List>
        ) : (
          <Table
            data={invites}
            columns={inviteColumns}
            idKey="id"
            hasHover
            density="compact"
            textOverflow="truncate"
          />
        )}
      </VStack>

      <Dialog isOpen={isCreateOpen} onOpenChange={setIsCreateOpen} purpose="form" width={480}>
        <Form
          route="invites.store"
          className="dialog-form-fill"
          onSuccess={() => setIsCreateOpen(false)}
        >
          {({ errors, processing }) => (
            <Layout
              header={
                <DialogHeader
                  title="Create invite"
                  subtitle="Send a one-time join link to a teammate"
                  onOpenChange={setIsCreateOpen}
                />
              }
              content={
                <LayoutContent isScrollable>
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
