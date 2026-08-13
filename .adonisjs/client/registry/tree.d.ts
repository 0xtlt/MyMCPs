/* eslint-disable prettier/prettier */
import type { routes } from './index.ts'

export interface ApiDefinition {
  health: typeof routes['health']
  oauth: {
    metadata: typeof routes['oauth.metadata']
    resourceMetadata: typeof routes['oauth.resourceMetadata']
    register: typeof routes['oauth.register']
    authorize: typeof routes['oauth.authorize']
    token: typeof routes['oauth.token']
    revoke: typeof routes['oauth.revoke']
  }
  oauthServer: {
    protectedResourceMetadata: typeof routes['oauth_server.protected_resource_metadata']
    authorize: typeof routes['oauth_server.authorize']
  }
  onboarding: {
    show: typeof routes['onboarding.show']
    store: typeof routes['onboarding.store']
  }
  gateway: {
    handle: typeof routes['gateway.handle']
    handleGet: typeof routes['gateway.handleGet']
  }
  session: {
    create: typeof routes['session.create']
    store: typeof routes['session.store']
    destroy: typeof routes['session.destroy']
  }
  invites: {
    show: typeof routes['invites.show']
    accept: typeof routes['invites.accept']
    index: typeof routes['invites.index']
    store: typeof routes['invites.store']
    destroy: typeof routes['invites.destroy']
  }
  home: typeof routes['home']
  settings: {
    index: typeof routes['settings.index']
    updateEmail: typeof routes['settings.updateEmail']
    updatePassword: typeof routes['settings.updatePassword']
    updateMcpLogging: typeof routes['settings.updateMcpLogging']
  }
  logs: {
    index: typeof routes['logs.index']
  }
  analytics: {
    index: typeof routes['analytics.index']
  }
  members: {
    destroy: typeof routes['members.destroy']
  }
  mcps: {
    index: typeof routes['mcps.index']
    store: typeof routes['mcps.store']
    oauthCallback: typeof routes['mcps.oauthCallback']
    show: typeof routes['mcps.show']
    update: typeof routes['mcps.update']
    destroy: typeof routes['mcps.destroy']
    probe: typeof routes['mcps.probe']
    updateNpm: typeof routes['mcps.updateNpm']
    oauthStart: typeof routes['mcps.oauthStart']
  }
  tokens: {
    index: typeof routes['tokens.index']
    store: typeof routes['tokens.store']
    update: typeof routes['tokens.update']
    revoke: typeof routes['tokens.revoke']
    destroy: typeof routes['tokens.destroy']
  }
}
