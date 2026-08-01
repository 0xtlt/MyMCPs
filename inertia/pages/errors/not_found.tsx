import { EmptyState } from '@astryxdesign/core/EmptyState'
import { Button } from '@astryxdesign/core/Button'
import { Center } from '@astryxdesign/core/Center'
import { Icon } from '@astryxdesign/core/Icon'

export default function NotFound() {
  return (
    <Center axis="both" style={{ minHeight: '100%' }}>
      <EmptyState
        title="Page not found"
        description="This route does not exist. Head back home to continue."
        icon={<Icon icon="search" size="lg" color="secondary" />}
        headingLevel={1}
        actions={<Button label="Go home" variant="primary" href="/" />}
      />
    </Center>
  )
}
