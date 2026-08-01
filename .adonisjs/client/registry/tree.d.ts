/* eslint-disable prettier/prettier */
import type { routes } from './index.ts'

export interface ApiDefinition {
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
    oauthStart: typeof routes['mcps.oauthStart']
  }
  tokens: {
    index: typeof routes['tokens.index']
    store: typeof routes['tokens.store']
    revoke: typeof routes['tokens.revoke']
  }
}
