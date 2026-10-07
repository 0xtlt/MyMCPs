import { Head } from '@inertiajs/react'
import { Form } from '@adonisjs/inertia/react'
import { Banner } from '@astryxdesign/core/Banner'
import { BreadcrumbItem, Breadcrumbs } from '@astryxdesign/core/Breadcrumbs'
import { Button } from '@astryxdesign/core/Button'
import { CodeBlock } from '@astryxdesign/core/CodeBlock'
import { Collapsible } from '@astryxdesign/core/Collapsible'
import { HStack, VStack } from '@astryxdesign/core/Layout'
import { MetadataList, MetadataListItem } from '@astryxdesign/core/MetadataList'
import { Heading, Text } from '@astryxdesign/core/Text'
import { ApprovalStateToken, type ApprovalRow } from '~/components/approval_state'
import { formatLocalDateTime } from '~/components/local_time'

type Summary = {
  /** Whether MyMCPs knows the tool and read the call itself. */
  interpreted: boolean
  title: string
  details: Array<{ label: string; value: string; before?: string }>
  warnings?: string[]
  toolDescription: string | null
}

type Props = {
  approval: ApprovalRow
  summary: Summary | null
  /** The exact arguments, as JSON. `null` when they can no longer be decrypted. */
  arguments: string | null
  runnable: boolean
}

function decision(approval: ApprovalRow) {
  const by = approval.decidedBy ?? 'a member who has since left'
  const on = approval.decidedAt ? formatLocalDateTime(approval.decidedAt) : ''
  return `${by} on ${on}`
}

function StateBanner({ approval }: { approval: ApprovalRow }) {
  switch (approval.state) {
    case 'pending':
      return null
    case 'approved':
      return (
        <Banner
          status="success"
          title="Approved"
          description={`Approved by ${decision(approval)}. The agent can run this call once, with these exact arguments, until ${formatLocalDateTime(approval.expiresAt)}.`}
          container="card"
        />
      )
    case 'used':
      return (
        <Banner
          status="success"
          title="Approved and run"
          description={`Approved by ${decision(approval)}. The agent ran the call on ${approval.consumedAt ? formatLocalDateTime(approval.consumedAt) : 'an unknown date'}.`}
          container="card"
        />
      )
    case 'denied':
      return (
        <Banner
          status="error"
          title="Denied"
          description={`Denied by ${decision(approval)}. The call was not run.`}
          container="card"
        />
      )
    case 'expired':
      return (
        <Banner
          status="warning"
          title="Expired"
          description={
            approval.decidedAt
              ? `Approved by ${decision(approval)}, but the agent did not run the call in time. It was not run.`
              : 'Nobody decided in time. The call was not run, and the agent has to ask again.'
          }
          container="card"
        />
      )
  }
}

export default function ApprovalShow({
  approval,
  summary,
  arguments: callArguments,
  runnable,
}: Props) {
  const isPending = approval.state === 'pending'

  return (
    <VStack gap={6} maxWidth={720} width="100%">
      <Head title="Approval request" />

      <VStack gap={3}>
        <Breadcrumbs variant="supporting">
          <BreadcrumbItem href="/approvals">Approvals</BreadcrumbItem>
          <BreadcrumbItem>Request</BreadcrumbItem>
        </Breadcrumbs>
        <HStack gap={3} vAlign="center" wrap="wrap">
          <ApprovalStateToken state={approval.state} />
          <Text type="supporting" color="secondary">
            Asked on {formatLocalDateTime(approval.createdAt)}
          </Text>
        </HStack>
        <Heading level={1}>{summary?.title ?? `Run the tool "${approval.toolName}"`}</Heading>
        <Text type="body" color="secondary">
          {isPending
            ? `An agent using the access token “${approval.accessToken.name}” wants to do this on ${approval.mcp.name}. Nothing has been done yet.`
            : `An agent using the access token “${approval.accessToken.name}” asked to do this on ${approval.mcp.name}.`}{' '}
          MyMCPs wrote this page from the call itself: the agent cannot change what it says.
        </Text>
      </VStack>

      <StateBanner approval={approval} />

      {isPending && !runnable ? (
        <Banner
          status="warning"
          title="This call can no longer run"
          description="Its MCP is disabled, or its access token was revoked or has expired. Approving it changes nothing."
          container="card"
        />
      ) : null}

      {summary?.warnings?.map((warning) => (
        <Banner key={warning} status="warning" title={warning} container="card" />
      ))}

      {summary === null ? (
        <Banner
          status="error"
          title="This request can no longer be read"
          description="It was encrypted with another APP_KEY. Deny it and have the agent ask again."
          container="card"
        />
      ) : (
        <VStack gap={4} hAlign="stretch">
          {summary.interpreted ? null : (
            <Banner
              status="info"
              title="MyMCPs does not know what this tool does"
              description={
                summary.toolDescription
                  ? `It lists the arguments exactly as the agent sent them. ${approval.mcp.name} describes the tool as: ${summary.toolDescription}`
                  : `It lists the arguments exactly as the agent sent them. ${approval.mcp.name} does not describe the tool.`
              }
              container="card"
            />
          )}
          {summary.details.length > 0 ? (
            <MetadataList
              title={summary.interpreted ? 'What it changes' : 'Arguments'}
              label={{ position: 'start', width: 200 }}
            >
              {summary.details.map((detail, index) => (
                // Two rows may carry the same label.
                <MetadataListItem key={`${index}:${detail.label}`} label={detail.label}>
                  {detail.before === undefined ? (
                    <Text type="body" className="approval-value">
                      {detail.value}
                    </Text>
                  ) : (
                    <VStack gap={0}>
                      <Text type="body" weight="bold" className="approval-value">
                        {detail.value}
                      </Text>
                      <Text type="supporting" color="secondary" className="approval-value">
                        Now: {detail.before}
                      </Text>
                    </VStack>
                  )}
                </MetadataListItem>
              ))}
            </MetadataList>
          ) : (
            <Text type="body" color="secondary">
              The call has no arguments.
            </Text>
          )}
        </VStack>
      )}

      <MetadataList title="Request" label={{ position: 'start', width: 200 }}>
        <MetadataListItem label="MCP">
          {approval.mcp.name} ({approval.mcp.slug})
        </MetadataListItem>
        <MetadataListItem label="Tool">{approval.toolName}</MetadataListItem>
        <MetadataListItem label="Access token">
          {approval.accessToken.name} ({approval.accessToken.prefix}…)
        </MetadataListItem>
        {isPending ? (
          <MetadataListItem label="Waits until">
            {formatLocalDateTime(approval.expiresAt)}
          </MetadataListItem>
        ) : null}
      </MetadataList>

      {callArguments !== null ? (
        <Collapsible trigger="Exact arguments sent by the agent" defaultIsOpen={false}>
          <CodeBlock
            code={callArguments}
            language="json"
            width="100%"
            size="sm"
            maxHeight={360}
            isWrapped
          />
        </Collapsible>
      ) : null}

      {isPending ? (
        <Form route="approvals.decide" routeParams={{ id: approval.id }}>
          {({ processing }) => (
            <VStack gap={3} hAlign="stretch">
              <Text type="supporting" color="secondary">
                Approving lets the agent run this call once, with these exact arguments. It then has
                to call the tool again: tell it once you have decided.
              </Text>
              <HStack gap={2} hAlign="end" wrap="wrap">
                <Button
                  type="submit"
                  name="decision"
                  value="deny"
                  label="Deny"
                  variant="secondary"
                  isDisabled={processing}
                />
                <Button
                  type="submit"
                  name="decision"
                  value="approve"
                  label="Approve"
                  variant="primary"
                  isLoading={processing}
                />
              </HStack>
            </VStack>
          )}
        </Form>
      ) : null}
    </VStack>
  )
}
