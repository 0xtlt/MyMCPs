import type { ApplicationService } from '@adonisjs/core/types'

/**
 * Characters an HTML parser (`<`, `>`, `&`) or a script context (the two
 * Unicode line separators) could treat as markup inside a data script.
 */
const MARKUP_CHARACTERS = /[<>&\u2028\u2029]/g

/**
 * Serializes the Inertia page object for a `<script type="application/json">`
 * element. `@inertia()` only escapes `/`, so a prop containing `<!--<script`
 * keeps the following `</script>` from closing the element, which swallows the
 * mount element and blanks the page. Writing these characters as JSON
 * `\uXXXX` escapes leaves nothing for the HTML parser to act on, and
 * `JSON.parse` still returns the original strings.
 */
export function serializeInertiaPage(page: unknown): string {
  return JSON.stringify(page ?? {}).replace(
    MARKUP_CHARACTERS,
    (character) => `\\u${character.charCodeAt(0).toString(16).padStart(4, '0')}`
  )
}

/**
 * Exposes the serializer to `resources/views/inertia_layout.edge`.
 */
export default class InertiaPageProvider {
  constructor(protected app: ApplicationService) {}

  async boot() {
    const { default: edge } = await import('edge.js')
    edge.global('inertiaPageJson', serializeInertiaPage)
  }
}
