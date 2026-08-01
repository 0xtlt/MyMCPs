import { EmptyState } from '@astryxdesign/core/EmptyState'
import { Button } from '@astryxdesign/core/Button'
import { Center } from '@astryxdesign/core/Center'
import { Icon } from '@astryxdesign/core/Icon'

export default function ServerError() {
  return (
    <Center axis="both" style={{ minHeight: '100%' }}>
      <EmptyState
        title="Something went wrong"
        description="An unexpected error occurred. Try again or return home."
        icon={<Icon icon="error" size="lg" color="error" />}
        headingLevel={1}
        actions={<Button label="Go home" variant="primary" href="/" />}
      />
    </Center>
  )
}
