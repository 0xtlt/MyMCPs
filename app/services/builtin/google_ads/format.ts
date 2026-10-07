/**
 * How Google Ads writes accounts and money, and how people read them.
 */

const MICROS = 1_000_000

/** `1234567890` for `123-456-7890`: the form the API takes. */
export function customerNumber(value: string) {
  return value.replaceAll('-', '')
}

/** `123-456-7890`: the form Google Ads shows, and people recognize. */
export function customerLabel(customerId: string) {
  return customerId.replace(/^(\d{3})(\d{3})(\d{4})$/, '$1-$2-$3')
}

/** A language code as Google Ads writes it: `fr`, and with a region `pt_BR`. */
export function languageCode(value: string) {
  const [language, region] = value.trim().split(/[_-]/)
  return region ? `${language.toLowerCase()}_${region.toUpperCase()}` : language.toLowerCase()
}

/**
 * An amount in the account's currency as the API takes it. Rounded to the
 * cent, which is the billable unit of most currencies: the API refuses an
 * amount that is not a multiple of the currency's own.
 */
export function toMicros(amount: number) {
  return String(Math.round(amount * 100) * (MICROS / 100))
}

/** `undefined` for an amount the API left out. */
export function fromMicros(micros: string | number | null | undefined) {
  if (micros === null || micros === undefined || micros === '') return undefined
  const amount = Number(micros) / MICROS
  return Number.isFinite(amount) ? amount : undefined
}

/**
 * An amount as a person reads it, such as `€250.00`. The currency comes from
 * the account, never from the agent.
 */
export function money(amount: number, currencyCode: string) {
  try {
    return new Intl.NumberFormat('en', { style: 'currency', currency: currencyCode }).format(amount)
  } catch {
    // Not a currency `Intl` knows: say the code as Google gave it.
    return `${amount.toFixed(2)} ${currencyCode}`
  }
}

/** A share, such as a click-through rate, as a percentage with two decimals. */
export function percent(ratio: number | undefined) {
  return ratio === undefined ? undefined : Math.round(ratio * 10_000) / 100
}
