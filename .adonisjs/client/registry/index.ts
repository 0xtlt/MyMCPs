/* eslint-disable prettier/prettier */
import type { AdonisEndpoint } from '@tuyau/core/types'
import type { Registry } from './schema.d.ts'
import type { ApiDefinition } from './tree.d.ts'

const placeholder: any = {}

const routes = {
  'onboarding.show': {
    methods: ["GET","HEAD"],
    pattern: '/onboarding',
    tokens: [{"old":"/onboarding","type":0,"val":"onboarding","end":""}],
    types: placeholder as Registry['onboarding.show']['types'],
  },
  'onboarding.store': {
    methods: ["POST"],
    pattern: '/onboarding',
    tokens: [{"old":"/onboarding","type":0,"val":"onboarding","end":""}],
    types: placeholder as Registry['onboarding.store']['types'],
  },
  'session.create': {
    methods: ["GET","HEAD"],
    pattern: '/login',
    tokens: [{"old":"/login","type":0,"val":"login","end":""}],
    types: placeholder as Registry['session.create']['types'],
  },
  'session.store': {
    methods: ["POST"],
    pattern: '/login',
    tokens: [{"old":"/login","type":0,"val":"login","end":""}],
    types: placeholder as Registry['session.store']['types'],
  },
  'invites.show': {
    methods: ["GET","HEAD"],
    pattern: '/invite/:token',
    tokens: [{"old":"/invite/:token","type":0,"val":"invite","end":""},{"old":"/invite/:token","type":1,"val":"token","end":""}],
    types: placeholder as Registry['invites.show']['types'],
  },
  'invites.accept': {
    methods: ["POST"],
    pattern: '/invite/:token',
    tokens: [{"old":"/invite/:token","type":0,"val":"invite","end":""},{"old":"/invite/:token","type":1,"val":"token","end":""}],
    types: placeholder as Registry['invites.accept']['types'],
  },
  'home': {
    methods: ["GET","HEAD"],
    pattern: '/',
    tokens: [{"old":"/","type":0,"val":"/","end":""}],
    types: placeholder as Registry['home']['types'],
  },
  'session.destroy': {
    methods: ["POST"],
    pattern: '/logout',
    tokens: [{"old":"/logout","type":0,"val":"logout","end":""}],
    types: placeholder as Registry['session.destroy']['types'],
  },
  'invites.index': {
    methods: ["GET","HEAD"],
    pattern: '/invites',
    tokens: [{"old":"/invites","type":0,"val":"invites","end":""}],
    types: placeholder as Registry['invites.index']['types'],
  },
  'invites.store': {
    methods: ["POST"],
    pattern: '/invites',
    tokens: [{"old":"/invites","type":0,"val":"invites","end":""}],
    types: placeholder as Registry['invites.store']['types'],
  },
} as const satisfies Record<string, AdonisEndpoint>

export { routes }

export const registry = {
  routes,
  $tree: {} as ApiDefinition,
}

declare module '@tuyau/core/types' {
  export interface UserRegistry {
    routes: typeof routes
    $tree: ApiDefinition
  }
}
