/**
 * Vine schema for the links to files of built-in MCPs.
 */
import vine from '@vinejs/vine'
import { decodeFileReference } from '#services/builtin/file_link'

/** Replace the path segment with what the tool put in the link. */
const fileReferenceRule = vine.createRule((value, _options, field) => {
  const reference = decodeFileReference(value as string)
  if (reference === undefined) {
    field.report('The {{ field }} field must be a file reference', 'fileReference', field)
    return
  }
  field.mutate(reference, field)
})

/**
 * Route parameters of a file link. The signature of the link covers them, so
 * the request is only validated once it has been checked. What a reference
 * must contain is for the provider to say.
 */
export const builtinFileValidator = vine.create({
  params: vine.object({
    id: vine.number().withoutDecimals().positive(),
    reference: vine
      .string()
      .use(fileReferenceRule())
      .transform((reference): unknown => reference),
  }),
})
