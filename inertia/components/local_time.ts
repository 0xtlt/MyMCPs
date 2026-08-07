export function browserTimeZone() {
  return Intl.DateTimeFormat().resolvedOptions().timeZone
}

const dayFirstDateOptions: Intl.DateTimeFormatOptions = {
  day: '2-digit',
  month: '2-digit',
  year: 'numeric',
}

export function formatLocalDate(value: string, timeZone?: string) {
  return new Intl.DateTimeFormat('en-GB', {
    ...dayFirstDateOptions,
    timeZone,
  }).format(new Date(value))
}

export function formatLocalDateTime(value: string, timeZone?: string) {
  return new Intl.DateTimeFormat('en-GB', {
    ...dayFirstDateOptions,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    timeZone,
  }).format(new Date(value))
}

export function formatTimeZoneLabel(timeZone: string) {
  const offset = new Intl.DateTimeFormat('en', {
    timeZone,
    timeZoneName: 'longOffset',
  })
    .formatToParts(new Date())
    .find((part) => part.type === 'timeZoneName')
    ?.value.replace('GMT', 'UTC')

  return offset ? `${timeZone} (${offset})` : timeZone
}
