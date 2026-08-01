/* eslint-disable prettier/prettier */
import type { routes } from './index.ts'

export interface ApiDefinition {
  onboarding: {
    show: typeof routes['onboarding.show']
    store: typeof routes['onboarding.store']
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
  }
  home: typeof routes['home']
}
