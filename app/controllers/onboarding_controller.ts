import User from '#models/user'
import { BackupImportError, beginBackupImport, importBackup } from '#services/backup/import'
import { receiveBackupUpload } from '#services/backup/upload'
import { createBackupWorkspace, removeBackupWorkspace } from '#services/backup/workspace'
import { rateLimitClientKey } from '#services/client_ip'
import { signIn } from '#services/session_stamp'
import { backupImportRateLimiter } from '#start/limiter'
import { importBackupValidator } from '#validators/backup'
import { onboardingValidator } from '#validators/user'
import type { HttpContext } from '@adonisjs/core/http'
import db from '@adonisjs/lucid/services/db'
import { errors } from '@vinejs/vine'

export default class OnboardingController {
  async show({ inertia }: HttpContext) {
    return inertia.render('onboarding/index', {})
  }

  async store(ctx: HttpContext) {
    const { request, response } = ctx
    const payload = await request.validateUsing(onboardingValidator)

    const user = await db.transaction(async (trx) => {
      const count = await User.query({ client: trx }).count('* as total')
      if (Number(count[0].$extras.total) > 0) {
        return null
      }

      return User.create(
        {
          fullName: payload.fullName,
          email: payload.email,
          password: payload.password,
          role: 'admin',
        },
        { client: trx }
      )
    })

    if (!user) {
      return response.redirect().toRoute('home')
    }

    await signIn(ctx, user)
    return response.redirect().toRoute('home')
  }

  async showImport({ inertia }: HttpContext) {
    return inertia.render('onboarding/import', {})
  }

  /**
   * Make this new instance the one of a backup. Whoever reaches the setup
   * screen can send one, so the request has cost nothing yet when it gets
   * here: its body is still unread, and the session, the CSRF token and the
   * absence of any account have been checked on the way.
   */
  async storeImport(ctx: HttpContext) {
    const { request, response, session, logger } = ctx
    const client = request.ip()

    // Counted before a byte of the body is read.
    await backupImportRateLimiter.consume(`backup-import:${rateLimitClientKey(client)}`)

    /** Back to the form with what to tell the person, and without what they typed. */
    const refuse = (refusal: InstanceType<typeof errors.E_VALIDATION_ERROR>, outcome: string) => {
      logger.warn({ client, outcome }, 'Backup import refused')
      session.flashValidationErrors(refusal, false)
      return response.redirect().toRoute('onboarding.showImport')
    }
    /** What is wrong with the file goes under its field, a wrong password under the other. */
    const refusal = (field: 'backup' | 'password' | 'import', message: string) =>
      new errors.E_VALIDATION_ERROR([{ field, message, rule: 'backup' }])

    const finish = beginBackupImport()
    if (!finish) {
      // Nothing is wrong with what was sent: said above the form, under no field.
      const busy = new BackupImportError('busy')
      return refuse(refusal('import', busy.message), busy.reason)
    }

    let workspace: string | undefined
    try {
      workspace = await createBackupWorkspace()
      const upload = await receiveBackupUpload(ctx, workspace)

      const [invalid, form] = await importBackupValidator.tryValidate({
        backup: upload.file?.size,
        password: upload.password,
      })
      if (invalid) {
        return refuse(invalid, 'incomplete_form')
      }

      await importBackup({ filePath: upload.file!.path, password: form.password, workspace })
    } catch (error) {
      if (!(error instanceof BackupImportError)) {
        // The client hung up halfway: nothing was kept, and nobody is left to answer.
        if (request.request.socket.destroyed) {
          logger.warn({ client, outcome: 'interrupted' }, 'Backup import refused')
          return
        }
        throw error
      }

      if (error.reason === 'already_set_up') {
        // Someone created the first account meanwhile: answer as the setup screen now does.
        logger.warn({ client, outcome: error.reason }, 'Backup import refused')
        return response
          .redirect()
          .toRoute((await ctx.auth.use('web').check()) ? 'home' : 'session.create')
      }
      return refuse(
        refusal(error.reason === 'wrong_password' ? 'password' : 'backup', error.message),
        error.reason
      )
    } finally {
      if (workspace) await removeBackupWorkspace(workspace)
      finish()
    }

    logger.info({ client, outcome: 'imported' }, 'Backup imported')
    session.flash('success', 'Backup imported. Sign in with an account of the imported instance.')
    return response.redirect().toRoute('session.create')
  }
}
