import app from '@adonisjs/core/services/app'
import { type HttpContext, ExceptionHandler } from '@adonisjs/core/http'
import type { HttpError, StatusPageRange, StatusPageRenderer } from '@adonisjs/core/types/http'
import { errors as shieldErrors } from '@adonisjs/shield'

/**
 * What API clients are told about an unexpected error in production.
 */
const SERVER_ERROR_MESSAGE = 'Internal server error'

export default class HttpExceptionHandler extends ExceptionHandler {
  /**
   * In debug mode, the exception handler will display verbose errors
   * with pretty printed stack traces.
   */
  protected debug = !app.inProduction

  /**
   * Status pages are used to display a custom HTML pages for certain error
   * codes. You might want to enable them in production only, but feel
   * free to enable them in development as well.
   */
  protected renderStatusPages = app.inProduction

  /**
   * Status pages is a collection of error code range and a callback
   * to return the HTML contents to send as a response.
   */
  protected statusPages: Record<StatusPageRange, StatusPageRenderer> = {
    '404': (_, { inertia }) => inertia.render('errors/not_found', {}),
    '500..599': (_, { inertia }) => inertia.render('errors/server_error', {}),
  }

  /**
   * The message of an unexpected error can expose internals, such as the SQL
   * of a failed query. Outside debug mode, API clients only learn that the
   * server failed; the error itself is still logged by "report".
   */
  async renderErrorAsJSON(error: HttpError, ctx: HttpContext) {
    if (error.status >= 500 && !this.isDebuggingEnabled(ctx)) {
      ctx.response.status(error.status).send({ message: SERVER_ERROR_MESSAGE })
      return
    }
    return super.renderErrorAsJSON(error, ctx)
  }

  async renderErrorAsJSONAPI(error: HttpError, ctx: HttpContext) {
    if (error.status >= 500 && !this.isDebuggingEnabled(ctx)) {
      ctx.response
        .status(error.status)
        .send({ errors: [{ title: SERVER_ERROR_MESSAGE, status: error.status }] })
      return
    }
    return super.renderErrorAsJSONAPI(error, ctx)
  }

  /**
   * The method is used for handling errors and returning
   * response to the client
   */
  async handle(error: unknown, ctx: HttpContext) {
    /**
     * Shield sends a form posted without a valid CSRF token back where it
     * came from, with its fields copied into the session for a template to
     * fill the form again. No page reads them here, and the one form the
     * browser posts itself, the export of a backup, is made of passwords:
     * the refusal is flashed, and nothing of the form.
     */
    if (error instanceof shieldErrors.E_BAD_CSRF_TOKEN && 'session' in ctx) {
      const message = error.getResponseMessage(error, ctx)
      ctx.session.flash('error', message)
      ctx.session.flashErrors({ [error.code]: message })
      ctx.response.redirect().back()
      return
    }

    return super.handle(error, ctx)
  }

  /**
   * The method is used to report error to the logging service or
   * the a third party error monitoring service.
   *
   * @note You should not attempt to send a response from this method.
   */
  async report(error: unknown, ctx: HttpContext) {
    return super.report(error, ctx)
  }
}
