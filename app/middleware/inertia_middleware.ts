import type { HttpContext } from '@adonisjs/core/http'
import type { NextFn } from '@adonisjs/core/types/http'
import type { InferSharedProps } from '@adonisjs/inertia/types'
import User from '#models/user'
import UserTransformer from '#transformers/user_transformer'
import { publicOauthAppUrl } from '#services/public_url'
import BaseInertiaMiddleware from '@adonisjs/inertia/inertia_middleware'

export default class InertiaMiddleware extends BaseInertiaMiddleware {
  async share(ctx: HttpContext) {
    /**
     * The share method is called everytime an Inertia page is rendered. In
     * certain cases, a page may get rendered before the session middleware
     * or the auth middleware are executed. For example: During a 404 request.
     *
     * In that case, we must always assume that HttpContext is not fully hydrated
     * with all the properties
     */
    const { auth } = ctx as Partial<HttpContext>

    /**
     * Data shared with all Inertia pages. Make sure you are using
     * transformers for rich data-types like Models.
     */
    return {
      errors: ctx.inertia.always(this.getValidationErrors(ctx)),
      user: ctx.inertia.always(auth?.user ? UserTransformer.transform(auth.user) : undefined),
      setupComplete: ctx.inertia.always(await User.setupComplete()),
      appUrlConfigured: ctx.inertia.always(Boolean(publicOauthAppUrl())),
    }
  }

  flash(ctx: HttpContext) {
    const { session } = ctx as Partial<HttpContext>
    const errorRaw = session?.flashMessages.get('error')
    const successRaw = session?.flashMessages.get('success')

    return {
      error: typeof errorRaw === 'string' ? errorRaw : undefined,
      success: typeof successRaw === 'string' ? successRaw : undefined,
    }
  }

  async handle(ctx: HttpContext, next: NextFn) {
    await this.init(ctx)
    const output = await next()
    this.dispose(ctx)
    return output
  }
}

type MiddlewareSharedProps = InferSharedProps<InertiaMiddleware>

declare module '@adonisjs/inertia/types' {
  export interface SharedProps extends MiddlewareSharedProps {}
}
