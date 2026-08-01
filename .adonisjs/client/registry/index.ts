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
  'gateway.handle': {
    methods: ["POST"],
    pattern: '/mcp',
    tokens: [{"old":"/mcp","type":0,"val":"mcp","end":""}],
    types: placeholder as Registry['gateway.handle']['types'],
  },
  'gateway.handleGet': {
    methods: ["GET","HEAD"],
    pattern: '/mcp',
    tokens: [{"old":"/mcp","type":0,"val":"mcp","end":""}],
    types: placeholder as Registry['gateway.handleGet']['types'],
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
  'invites.destroy': {
    methods: ["DELETE"],
    pattern: '/invites/:id',
    tokens: [{"old":"/invites/:id","type":0,"val":"invites","end":""},{"old":"/invites/:id","type":1,"val":"id","end":""}],
    types: placeholder as Registry['invites.destroy']['types'],
  },
  'members.destroy': {
    methods: ["DELETE"],
    pattern: '/members/:id',
    tokens: [{"old":"/members/:id","type":0,"val":"members","end":""},{"old":"/members/:id","type":1,"val":"id","end":""}],
    types: placeholder as Registry['members.destroy']['types'],
  },
  'mcps.index': {
    methods: ["GET","HEAD"],
    pattern: '/mcps',
    tokens: [{"old":"/mcps","type":0,"val":"mcps","end":""}],
    types: placeholder as Registry['mcps.index']['types'],
  },
  'mcps.store': {
    methods: ["POST"],
    pattern: '/mcps',
    tokens: [{"old":"/mcps","type":0,"val":"mcps","end":""}],
    types: placeholder as Registry['mcps.store']['types'],
  },
  'mcps.oauthCallback': {
    methods: ["GET","HEAD"],
    pattern: '/mcps/oauth/callback',
    tokens: [{"old":"/mcps/oauth/callback","type":0,"val":"mcps","end":""},{"old":"/mcps/oauth/callback","type":0,"val":"oauth","end":""},{"old":"/mcps/oauth/callback","type":0,"val":"callback","end":""}],
    types: placeholder as Registry['mcps.oauthCallback']['types'],
  },
  'mcps.show': {
    methods: ["GET","HEAD"],
    pattern: '/mcps/:id',
    tokens: [{"old":"/mcps/:id","type":0,"val":"mcps","end":""},{"old":"/mcps/:id","type":1,"val":"id","end":""}],
    types: placeholder as Registry['mcps.show']['types'],
  },
  'mcps.update': {
    methods: ["PUT"],
    pattern: '/mcps/:id',
    tokens: [{"old":"/mcps/:id","type":0,"val":"mcps","end":""},{"old":"/mcps/:id","type":1,"val":"id","end":""}],
    types: placeholder as Registry['mcps.update']['types'],
  },
  'mcps.destroy': {
    methods: ["DELETE"],
    pattern: '/mcps/:id',
    tokens: [{"old":"/mcps/:id","type":0,"val":"mcps","end":""},{"old":"/mcps/:id","type":1,"val":"id","end":""}],
    types: placeholder as Registry['mcps.destroy']['types'],
  },
  'mcps.probe': {
    methods: ["POST"],
    pattern: '/mcps/:id/probe',
    tokens: [{"old":"/mcps/:id/probe","type":0,"val":"mcps","end":""},{"old":"/mcps/:id/probe","type":1,"val":"id","end":""},{"old":"/mcps/:id/probe","type":0,"val":"probe","end":""}],
    types: placeholder as Registry['mcps.probe']['types'],
  },
  'mcps.oauthStart': {
    methods: ["GET","HEAD"],
    pattern: '/mcps/:id/oauth/start',
    tokens: [{"old":"/mcps/:id/oauth/start","type":0,"val":"mcps","end":""},{"old":"/mcps/:id/oauth/start","type":1,"val":"id","end":""},{"old":"/mcps/:id/oauth/start","type":0,"val":"oauth","end":""},{"old":"/mcps/:id/oauth/start","type":0,"val":"start","end":""}],
    types: placeholder as Registry['mcps.oauthStart']['types'],
  },
  'tokens.index': {
    methods: ["GET","HEAD"],
    pattern: '/tokens',
    tokens: [{"old":"/tokens","type":0,"val":"tokens","end":""}],
    types: placeholder as Registry['tokens.index']['types'],
  },
  'tokens.store': {
    methods: ["POST"],
    pattern: '/tokens',
    tokens: [{"old":"/tokens","type":0,"val":"tokens","end":""}],
    types: placeholder as Registry['tokens.store']['types'],
  },
  'tokens.revoke': {
    methods: ["POST"],
    pattern: '/tokens/:id/revoke',
    tokens: [{"old":"/tokens/:id/revoke","type":0,"val":"tokens","end":""},{"old":"/tokens/:id/revoke","type":1,"val":"id","end":""},{"old":"/tokens/:id/revoke","type":0,"val":"revoke","end":""}],
    types: placeholder as Registry['tokens.revoke']['types'],
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
