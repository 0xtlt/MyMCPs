/**
 * Vine schemas for exporting and importing backups.
 */
import vine, { SimpleMessagesProvider, Vine } from '@vinejs/vine'

/**
 * The export form of the Settings page. The password protects the file; the
 * current password of the account confirms who asks for it.
 */
export const exportBackupValidator = vine.create({
  password: vine.string().minLength(8).maxLength(128).confirmed({
    confirmationField: 'passwordConfirmation',
  }),
  passwordConfirmation: vine.string(),
  currentPassword: vine.string().minLength(1),
})

/**
 * The import form of the setup screen, once its file has been received:
 * `backup` is the size of that file, nothing when none was sent.
 */
export const importBackupValidator = vine.create({
  backup: vine.number().min(1),
  password: vine.string(),
})

importBackupValidator.messagesProvider = new SimpleMessagesProvider({
  'backup.required': 'Choose a backup file',
  'backup.min': 'Choose a backup file',
  'password.required': 'Enter the password of the backup',
})

/**
 * The metadata of a backup is JSON written by an instance, not the fields of
 * a form, so it gets a Vine of its own: the one the pages use turns an empty
 * string into null.
 */
const metadataVine = new Vine()

/**
 * What a backup says about itself, read the way the Rust rewrite reads it.
 * Only what an import needs is checked: `createdAt` is any string, and the
 * other members, `app` included, are informational. The key becomes an
 * encryption key, which takes 16 characters at least.
 */
export const backupMetadataValidator = metadataVine.create({
  createdAt: metadataVine.string(),
  appKey: metadataVine.string().minLength(16).maxLength(512),
})
