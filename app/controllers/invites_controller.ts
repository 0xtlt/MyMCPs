import Invite from '#models/invite'
import User from '#models/user'
import { acceptInviteValidator, createInviteValidator } from '#validators/user'
import type { HttpContext } from '@adonisjs/core/http'
import env from '#start/env'
import db from '@adonisjs/lucid/services/db'
import { DateTime } from 'luxon'

export default class InvitesController {
  async index({ inertia, auth, request }: HttpContext) {
    const invites = await Invite.query()
      .where('created_by', auth.user!.id)
      .orderBy('created_at', 'desc')

    const configured = env.get('APP_URL').replace(/\/$/, '')
    const requestOrigin = `${request.protocol()}://${request.host()}`

    return inertia.render('invites/index', {
      invites: invites.map((invite) => ({
        id: invite.id,
        email: invite.email,
        role: invite.role,
        token: invite.token,
        acceptedAt: invite.acceptedAt?.toISO() ?? null,
        expiresAt: invite.expiresAt.toISO(),
        createdAt: invite.createdAt.toISO(),
        isUsable: invite.isUsable,
      })),
      appUrl: request.host()?.includes('localhost') ? requestOrigin : configured,
    })
  }

  async store({ request, response, auth, session }: HttpContext) {
    const { email } = await request.validateUsing(createInviteValidator)

    const existing = await User.findBy('email', email)
    if (existing) {
      session.flash('error', 'A user with that email already exists')
      return response.redirect().toRoute('invites.index')
    }

    const pending = await Invite.query()
      .where('email', email)
      .whereNull('accepted_at')
      .where('expires_at', '>', DateTime.utc().toSQL())
      .first()

    if (pending) {
      session.flash('error', 'A pending invite already exists for that email')
      return response.redirect().toRoute('invites.index')
    }

    await Invite.create({
      email,
      role: 'member',
      token: Invite.generateToken(),
      createdBy: auth.user!.id,
      expiresAt: DateTime.utc().plus({ days: 7 }),
    })

    session.flash('success', 'Invite created')
    return response.redirect().toRoute('invites.index')
  }

  async show({ params, inertia, response, session }: HttpContext) {
    const invite = await Invite.findBy('token', params.token)

    if (!invite || !invite.isUsable) {
      session.flash('error', 'This invite is invalid or has expired')
      return response.redirect().toRoute('home')
    }

    return inertia.render('invites/accept', {
      token: invite.token,
      email: invite.email,
    })
  }

  async accept({ params, request, response, auth, session }: HttpContext) {
    const payload = await request.validateUsing(acceptInviteValidator)

    const result = await db.transaction(async (trx) => {
      const invite = await Invite.query({ client: trx })
        .where('token', params.token)
        .forUpdate()
        .first()

      if (!invite || !invite.isUsable) {
        return { status: 'invalid' as const }
      }

      const existing = await User.query({ client: trx }).where('email', invite.email).first()
      if (existing) {
        invite.useTransaction(trx)
        invite.acceptedAt = DateTime.utc()
        await invite.save()
        return { status: 'exists' as const }
      }

      const user = await User.create(
        {
          fullName: payload.fullName,
          email: invite.email,
          password: payload.password,
          role: invite.role,
        },
        { client: trx }
      )

      invite.useTransaction(trx)
      invite.acceptedAt = DateTime.utc()
      await invite.save()

      return { status: 'created' as const, user }
    })

    if (result.status === 'invalid') {
      session.flash('error', 'This invite is invalid or has expired')
      return response.redirect().toRoute('home')
    }

    if (result.status === 'exists') {
      session.flash('error', 'A user with that email already exists')
      return response.redirect().toRoute('session.create')
    }

    await auth.use('web').login(result.user)
    session.flash('success', 'Welcome to MyMCPs')
    return response.redirect().toRoute('home')
  }
}
