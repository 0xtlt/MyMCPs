import type { ApiResponse } from '@japa/api-client'
import type { Assert } from '@japa/assert'

export function assertRedirectTo(assert: Assert, response: ApiResponse, path: string) {
  const location = response.header('location')

  assert.isDefined(location)
  assert.equal(new URL(location!, 'http://localhost').pathname, path)
}
