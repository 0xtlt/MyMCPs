import BodyParserMiddleware from '@adonisjs/core/bodyparser_middleware'
import type { HttpContext } from '@adonisjs/core/http'
import type { NextFn } from '@adonisjs/core/types/http'

/**
 * Routes whose body is a file that the controller writes to disk as it
 * arrives. The parser would read a text or JSON file into memory, refuse it
 * over 1 MB, and leave nothing for the controller to read.
 */
const FILE_ROUTES = new Set(['/uploads/:id/:reference'])

/**
 * Parse the request body, as configured in `config/bodyparser.ts`, on every
 * route but the ones that take a file.
 */
export default class RequestBodyMiddleware {
  async handle(ctx: HttpContext, next: NextFn) {
    if (ctx.route && FILE_ROUTES.has(ctx.route.pattern)) {
      return next()
    }

    const bodyParser = await ctx.containerResolver.make(BodyParserMiddleware)
    return bodyParser.handle(ctx, next)
  }
}
