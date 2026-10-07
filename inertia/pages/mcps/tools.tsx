import { useMemo, useState } from 'react'
import { Head, router } from '@inertiajs/react'
import { Banner } from '@astryxdesign/core/Banner'
import { BreadcrumbItem, Breadcrumbs } from '@astryxdesign/core/Breadcrumbs'
import { Button } from '@astryxdesign/core/Button'
import { HStack, StackItem, VStack } from '@astryxdesign/core/Layout'
import { List, ListItem } from '@astryxdesign/core/List'
import { SegmentedControl, SegmentedControlItem } from '@astryxdesign/core/SegmentedControl'
import { Heading, Text } from '@astryxdesign/core/Text'
import { TextInput } from '@astryxdesign/core/TextInput'
import { Token } from '@astryxdesign/core/Token'

type Mode = 'auto' | 'ask'

type ToolRow = {
  name: string
  description: string | null
  mode: Mode
  /** What the tool does until someone chooses. Built-in tools that spend money ask. */
  defaultMode: Mode
  /** False for a tool with a saved choice that its MCP no longer lists. */
  isListed: boolean
}

type Props = {
  mcp: { id: number; name: string; slug: string; isBuiltin: boolean }
  tools: ToolRow[]
  listError: string | null
  savedUnreadable: boolean
}

export default function McpTools({ mcp, tools, listError, savedUnreadable }: Props) {
  const saved = useMemo(
    () => Object.fromEntries(tools.map((tool) => [tool.name, tool.mode])),
    [tools]
  )
  const [changes, setChanges] = useState<Record<string, Mode>>({})
  const [search, setSearch] = useState('')
  const [isSaving, setIsSaving] = useState(false)

  const modeOf = (tool: ToolRow) => changes[tool.name] ?? tool.mode
  const changed = tools.filter((tool) => modeOf(tool) !== saved[tool.name])
  const asking = tools.filter((tool) => modeOf(tool) === 'ask').length

  const query = search.trim().toLowerCase()
  const shown = query
    ? tools.filter(
        (tool) =>
          tool.name.toLowerCase().includes(query) ||
          (tool.description ?? '').toLowerCase().includes(query)
      )
    : tools

  function setAll(mode: Mode) {
    // Only the tools in view: a choice made on a tool the search hides is kept.
    setChanges((current) => ({
      ...current,
      ...Object.fromEntries(shown.map((tool) => [tool.name, mode])),
    }))
  }

  function save() {
    router.put(
      `/mcps/${mcp.id}/tools`,
      { tools: tools.map((tool) => ({ name: tool.name, mode: modeOf(tool) })) },
      {
        preserveScroll: true,
        onStart: () => setIsSaving(true),
        onFinish: () => setIsSaving(false),
        onSuccess: () => setChanges({}),
      }
    )
  }

  return (
    <VStack gap={6} maxWidth={960} width="100%">
      <Head title={`Tool approvals · ${mcp.name}`} />

      <VStack gap={3}>
        <Breadcrumbs variant="supporting">
          <BreadcrumbItem href="/mcps">MCPs</BreadcrumbItem>
          <BreadcrumbItem href={`/mcps/${mcp.id}`}>{mcp.name}</BreadcrumbItem>
          <BreadcrumbItem>Tool approvals</BreadcrumbItem>
        </Breadcrumbs>
        <Heading level={1}>Tool approvals</Heading>
        <Text type="body" color="secondary">
          Choose what happens when an agent calls a tool of {mcp.name}. A tool that asks is not run:
          the agent gets a link to give you, and the call runs once you have signed in and approved
          it. The page you approve on is written by MyMCPs from the call itself, never by the agent.
        </Text>
      </VStack>

      {listError ? (
        <Banner
          status="error"
          title={`${mcp.name} did not list its tools`}
          description={`${listError} Only the tools with a saved choice are shown.`}
          container="card"
        />
      ) : null}

      {savedUnreadable ? (
        <Banner
          status="warning"
          title="The saved choices could not be read"
          description="Every tool of this MCP asks for approval until you save this page again."
          container="card"
        />
      ) : null}

      {tools.length === 0 ? (
        <Banner
          status="info"
          title="No tools to set up"
          description={
            listError
              ? 'Fix the connection from the MCPs page, then come back.'
              : 'This MCP has no tools.'
          }
          container="card"
        />
      ) : (
        <>
          <HStack gap={3} vAlign="end" wrap="wrap">
            <StackItem size="fill">
              <TextInput
                label="Find a tool"
                value={search}
                onChange={setSearch}
                placeholder="Name or description"
                hasClear
                width="100%"
              />
            </StackItem>
            <Button label="All run" variant="secondary" onClick={() => setAll('auto')} />
            <Button label="All ask" variant="secondary" onClick={() => setAll('ask')} />
          </HStack>

          <List
            header={`${asking} of ${tools.length} tools ask for approval`}
            density="compact"
            hasDividers
          >
            {shown.map((tool) => (
              <ListItem
                key={tool.name}
                label={tool.name}
                description={
                  <VStack gap={1}>
                    {tool.description ? (
                      <Text type="supporting" color="secondary" maxLines={3}>
                        {tool.description}
                      </Text>
                    ) : null}
                    {tool.isListed && tool.defaultMode !== 'ask' ? null : (
                      <HStack gap={1} wrap="wrap">
                        {tool.isListed ? null : (
                          <Token label="No longer listed by the MCP" color="gray" size="sm" />
                        )}
                        {tool.defaultMode === 'ask' ? (
                          <Token label="Asks by default" color="yellow" size="sm" />
                        ) : null}
                      </HStack>
                    )}
                  </VStack>
                }
                endContent={
                  <SegmentedControl
                    label={`When an agent calls ${tool.name}`}
                    size="sm"
                    value={modeOf(tool)}
                    onChange={(mode) =>
                      setChanges((current) => ({ ...current, [tool.name]: mode as Mode }))
                    }
                  >
                    <SegmentedControlItem value="auto" label="Runs" />
                    <SegmentedControlItem value="ask" label="Asks" />
                  </SegmentedControl>
                }
              />
            ))}
          </List>

          {shown.length === 0 ? (
            <Text type="body" color="secondary">
              No tool matches “{search}”.
            </Text>
          ) : null}

          <HStack gap={3} hAlign="end" vAlign="center" wrap="wrap">
            <Text type="supporting" color="secondary">
              {changed.length === 0
                ? 'No unsaved changes'
                : `${changed.length} unsaved ${changed.length === 1 ? 'change' : 'changes'}`}
            </Text>
            <Button label="Back to MCPs" variant="secondary" href="/mcps" />
            <Button
              label="Save"
              variant="primary"
              isDisabled={changed.length === 0}
              isLoading={isSaving}
              onClick={save}
            />
          </HStack>
        </>
      )}
    </VStack>
  )
}
