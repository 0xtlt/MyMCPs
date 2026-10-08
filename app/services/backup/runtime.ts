import env from '#start/env'

/** The largest backup file an import takes: 4 GiB. */
const MAX_FILE_BYTES = 4 * 1024 ** 3

/** The `APP_KEY` of this instance, exactly as configured. */
function instanceAppKey() {
  return env.get('APP_KEY').release()
}

/**
 * Seam for tests. They run a single instance, so they play a second one by
 * changing the key it says it has, and they cannot send 4 GiB to find the
 * limit.
 */
export const backupRuntime = {
  appKey: instanceAppKey,
  maxFileBytes: MAX_FILE_BYTES,
}

export function resetBackupRuntime() {
  backupRuntime.appKey = instanceAppKey
  backupRuntime.maxFileBytes = MAX_FILE_BYTES
}
