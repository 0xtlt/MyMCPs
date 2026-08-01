import type { HttpContext } from '@adonisjs/core/http'
import db from '@adonisjs/lucid/services/db'
import { DateTime } from 'luxon'
import Invite from '#models/invite'
import User from '#models/user'
import Mcp from '#models/mcp'
import AccessToken from '#models/access_token'
import { acceptInviteValidator, createInviteValidator } from '#validators/user'
import { publicAppUrl } from '#services/public_url'

export default class InvitesController {
  async index({ inertia, auth, request }: HttpContext) {
    const invites = await Invite.query().orderBy('created_at', 'desc')
    const members = await User.query().orderBy('created_at', 'asc')

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
      members: members.map((member) => ({
        id: member.id,
        email: member.email,
        fullName: member.fullName,
        role: member.role,
        createdAt: member.createdAt.toISO(),
        isCurrentUser: member.id === auth.user!.id,
      })),
      appUrl: publicAppUrl(request),
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

  async destroy({ params, response, session }: HttpContext) {
    const invite = await Invite.find(params.id)
    if (!invite) {
      session.flash('error', 'Invite not found')
      return response.redirect().toRoute('invites.index')
    }

    await invite.delete()
    session.flash('success', 'Invite removed')
    return response.redirect().toRoute('invites.index')
  }

  async destroyMember({ params, auth, response, session }: HttpContext) {
    const member = await User.find(params.id)
    if (!member) {
      session.flash('error', 'Member not found')
      return response.redirect().toRoute('invites.index')
    }

    if (member.id === auth.user!.id) {
      session.flash('error', 'You cannot remove your own account')
      return response.redirect().toRoute('invites.index')
    }

    if (member.role === 'admin') {
      const adminCount = await User.query().where('role', 'admin').count('* as total')
      if (Number(adminCount[0].$extras.total) <= 1) {
        session.flash('error', 'Cannot remove the last admin')
        return response.redirect().toRoute('invites.index')
      }
    }

    const actorId = auth.user!.id
    await db.transaction(async (trx) => {
      await Mcp.query({ client: trx }).where('created_by', member.id).update({ createdBy: actorId })
      await AccessToken.query({ client: trx })
        .where('created_by', member.id)
        .update({ createdBy: actorId })
      await Invite.query({ client: trx }).where('created_by', member.id).update({ createdBy: actorId })
      member.useTransaction(trx)
      await member.delete()
    })

    session.flash('success', 'Member removed')
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
