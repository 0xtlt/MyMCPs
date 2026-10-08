import { defineConfig } from '@adonisjs/core/bodyparser'

const bodyParserConfig = defineConfig({
  /**
   * Parse request bodies for these HTTP methods.
   * Keep this aligned with methods that receive payloads in your routes.
   */
  allowedMethods: ['POST', 'PUT', 'PATCH', 'DELETE'],

  /**
   * Config for the "application/x-www-form-urlencoded"
   * content-type parser.
   */
  form: {
    /**
     * Normalize empty string values to null.
     */
    convertEmptyStringsToNull: true,

    /**
     * Content types handled by the form parser.
     */
    types: ['application/x-www-form-urlencoded'],
  },

  /**
   * Config for the JSON parser.
   */
  json: {
    /**
     * Normalize empty string values to null.
     */
    convertEmptyStringsToNull: true,

    /**
     * Content types handled by the JSON parser.
     */
    types: [
      'application/json',
      'application/json-patch+json',
      'application/vnd.api+json',
      'application/csp-report',
    ],
  },

  /**
   * Config for the "multipart/form-data" content-type parser.
   * Multipart bodies are never parsed here: left on, any anonymous request
   * could have files streamed to the system tmp directory ahead of
   * authentication, where nothing removes them. The two routes that take a
   * file read the body themselves, once the request has passed their checks:
   * an upload link checks its signature first (see the request body
   * middleware), and the import of a backup, the one form with a file, has
   * a parser of its own (see app/services/backup/upload.ts).
   */
  multipart: {
    /**
     * Never write uploaded files to disk.
     */
    autoProcess: false,

    /**
     * Content types handled by the multipart parser.
     */
    types: [],
  },
})

export default bodyParserConfig
