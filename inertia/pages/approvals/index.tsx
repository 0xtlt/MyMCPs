import { Head } from '@inertiajs/react'
import { useAppShellMobile } from '@astryxdesign/core/AppShell'
import { Banner } from '@astryxdesign/core/Banner'
import { Button } from '@astryxdesign/core/Button'
import { VStack } from '@astryxdesign/core/Layout'
import { List, ListItem } from '@astryxdesign/core/List'
import { Table, pixel, proportional, type TableColumn } from '@astryxdesign/core/Table'
import { Heading, Text } from '@astryxdesign/core/Text'
import { ApprovalStateToken, type ApprovalRow } from '~/components/approval_state'
import { formatLocalDateTime } from '~/components/local_time'

function titleOf(approval: ApprovalRow) {
  return approval.title ?? `Run the tool "${approval.toolName}"`
}

function origin(approval: ApprovalRow) {
  return `${approval.toolName} · ${approval.mcp.name} · token ${approval.accessToken.name}`
}

function Requests({
  label,
  approvals,
  action,
}: {
  label: string
  approvals: ApprovalRow[]
  action: string
}) {
  const { isMobile } = useAppShellMobile()

  if (isMobile) {
    return (
      <List header={label} density="compact" hasDividers>
        {approvals.map((approval) => (
          <ListItem
            key={approval.id}
            label={titleOf(approval)}
            description={
              <VStack gap={1}>
                <Text type="supporting" color="secondary">
                  {origin(approval)}
                </Text>
                <ApprovalStateToken state={approval.state} />
              </VStack>
            }
            endContent={
              <Button
                label={`${action}: ${titleOf(approval)}`}
                variant="secondary"
                size="sm"
                href={`/approvals/${approval.id}`}
              >
                {action}
              </Button>
            }
          />
        ))}
      </List>
    )
  }

  const columns: TableColumn<ApprovalRow>[] = [
    {
      key: 'state',
      header: 'Status',
      width: pixel(180),
      renderCell: (approval) => <ApprovalStateToken state={approval.state} />,
    },
    {
      key: 'title',
      header: 'Call',
      width: proportional(3),
      renderCell: (approval) => (
        <VStack gap={0}>
          <Text type="body" weight="bold">
            {titleOf(approval)}
          </Text>
          <Text type="supporting" color="secondary">
            {origin(approval)}
          </Text>
        </VStack>
      ),
    },
    {
      key: 'createdAt',
      header: 'Asked',
      width: pixel(170),
      renderCell: (approval) => (
        <Text type="supporting" color="secondary">
          {formatLocalDateTime(approval.createdAt)}
        </Text>
      ),
    },
    {
      key: 'actions',
      header: 'Actions',
      width: pixel(100),
      align: 'end',
      renderCell: (approval) => (
        <Button
          label={`${action}: ${titleOf(approval)}`}
          variant="secondary"
          size="sm"
          href={`/approvals/${approval.id}`}
        >
          {action}
        </Button>
      ),
    },
  ]

  return <Table data={approvals} columns={columns} idKey="id" hasHover density="compact" />
}

export default function ApprovalsIndex({
  waiting,
  past,
}: {
  waiting: ApprovalRow[]
  past: ApprovalRow[]
}) {
  return (
    <VStack gap={6} maxWidth={960} width="100%">
      <Head title="Approvals" />
      <VStack gap={2}>
        <Heading level={1}>Approvals</Heading>
        <Text type="body" color="secondary">
          Tool calls that agents may not make on their own. Administrators see all of them, and
          members the ones made with their own access tokens. Choose which tools ask from the Tool
          approvals of each MCP.
        </Text>
      </VStack>

      <VStack gap={3} hAlign="stretch">
        <Heading level={2}>Waiting for a decision</Heading>
        {waiting.length === 0 ? (
          <Banner
            status="info"
            title="Nothing is waiting"
            description="When an agent calls a tool that asks for approval, it gets a link to give you, and the call is listed here."
            container="card"
          />
        ) : (
          <Requests label="Waiting for a decision" approvals={waiting} action="Review" />
        )}
      </VStack>

      {past.length > 0 ? (
        <VStack gap={3} hAlign="stretch">
          <Heading level={2}>Decided or expired</Heading>
          <Requests label="Decided or expired" approvals={past} action="Open" />
        </VStack>
      ) : null}
    </VStack>
  )
}
