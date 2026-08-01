/*
|--------------------------------------------------------------------------
| Validator file
|--------------------------------------------------------------------------
|
| Global VineJS configuration for this app.
|
*/

import vine, { VineDate } from '@vinejs/vine'
import { DateTime } from 'luxon'

declare module '@vinejs/vine/types' {
  interface VineGlobalTransforms {
    date: DateTime
  }
}

/**
 * HTML forms submit empty optional fields as "". Normalize them to null so
 * `.nullable()` / `.optional()` schemas match database nullability.
 * @see https://vinejs.dev/docs/html_forms_and_surprises
 */
vine.convertEmptyStringsToNull = true

/**
 * Validated dates become Luxon DateTime for Lucid and the rest of the app.
 */
VineDate.transform((value) => DateTime.fromJSDate(value))
