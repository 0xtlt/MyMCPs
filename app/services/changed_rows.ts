/**
 * How many rows an update or a delete changed. Lucid's model query builder
 * answers with that number in an array and its database query builder with
 * the bare number, so the length of the answer says nothing. SQLite ignores
 * `returning`: do not ask for rows back on a query counted here.
 */
export function changedRows(result: unknown) {
  return Number(Array.isArray(result) ? result[0] : result)
}
