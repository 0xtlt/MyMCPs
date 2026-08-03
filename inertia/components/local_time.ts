export function browserTimeZone() {
  return Intl.DateTimeFormat().resolvedOptions().timeZone
}

export function formatLocalDateTime(value: string, timeZone: string) {
  return new Date(value).toLocaleString(undefined, { timeZone })
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
