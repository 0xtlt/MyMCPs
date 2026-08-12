/* eslint-disable prettier/prettier */
import type { AdonisEndpoint } from '@tuyau/core/types'
import type { Registry } from './schema.d.ts'
import type { ApiDefinition } from './tree.d.ts'

const placeholder: any = {}

const routes = {
  'health': {
    methods: ["GET","HEAD"],
    pattern: '/health',
    tokens: [{"old":"/health","type":0,"val":"health","end":""}],
    types: placeholder as Registry['health']['types'],
  },
  'oauth.metadata': {
    methods: ["GET","HEAD"],
    pattern: '/.well-known/oauth-authorization-server',
    tokens: [{"old":"/.well-known/oauth-authorization-server","type":0,"val":".well-known","end":""},{"old":"/.well-known/oauth-authorization-server","type":0,"val":"oauth-authorization-server","end":""}],
    types: placeholder as Registry['oauth.metadata']['types'],
  },
  'oauth.resourceMetadata': {
    methods: ["GET","HEAD"],
    pattern: '/.well-known/oauth-protected-resource',
    tokens: [{"old":"/.well-known/oauth-protected-resource","type":0,"val":".well-known","end":""},{"old":"/.well-known/oauth-protected-resource","type":0,"val":"oauth-protected-resource","end":""}],
    types: placeholder as Registry['oauth.resourceMetadata']['types'],
  },
  'oauth_server.protected_resource_metadata': {
    methods: ["GET","HEAD"],
    pattern: '/.well-known/oauth-protected-resource/mcp',
    tokens: [{"old":"/.well-known/oauth-protected-resource/mcp","type":0,"val":".well-known","end":""},{"old":"/.well-known/oauth-protected-resource/mcp","type":0,"val":"oauth-protected-resource","end":""},{"old":"/.well-known/oauth-protected-resource/mcp","type":0,"val":"mcp","end":""}],
    types: placeholder as Registry['oauth_server.protected_resource_metadata']['types'],
  },
  'oauth.register': {
    methods: ["POST"],
    pattern: '/register',
    tokens: [{"old":"/register","type":0,"val":"register","end":""}],
    types: placeholder as Registry['oauth.register']['types'],
  },
  'oauth.authorize': {
    methods: ["GET","HEAD"],
    pattern: '/authorize',
    tokens: [{"old":"/authorize","type":0,"val":"authorize","end":""}],
    types: placeholder as Registry['oauth.authorize']['types'],
  },
  'oauth_server.authorize': {
    methods: ["POST"],
    pattern: '/authorize',
    tokens: [{"old":"/authorize","type":0,"val":"authorize","end":""}],
    types: placeholder as Registry['oauth_server.authorize']['types'],
  },
  'oauth.token': {
    methods: ["POST"],
    pattern: '/token',
    tokens: [{"old":"/token","type":0,"val":"token","end":""}],
    types: placeholder as Registry['oauth.token']['types'],
  },
  'oauth.revoke': {
    methods: ["POST"],
    pattern: '/revoke',
    tokens: [{"old":"/revoke","type":0,"val":"revoke","end":""}],
    types: placeholder as Registry['oauth.revoke']['types'],
  },
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
  'settings.index': {
    methods: ["GET","HEAD"],
    pattern: '/settings',
    tokens: [{"old":"/settings","type":0,"val":"settings","end":""}],
    types: placeholder as Registry['settings.index']['types'],
  },
  'settings.updateEmail': {
    methods: ["PATCH"],
    pattern: '/settings/email',
    tokens: [{"old":"/settings/email","type":0,"val":"settings","end":""},{"old":"/settings/email","type":0,"val":"email","end":""}],
    types: placeholder as Registry['settings.updateEmail']['types'],
  },
  'settings.updatePassword': {
    methods: ["PATCH"],
    pattern: '/settings/password',
    tokens: [{"old":"/settings/password","type":0,"val":"settings","end":""},{"old":"/settings/password","type":0,"val":"password","end":""}],
    types: placeholder as Registry['settings.updatePassword']['types'],
  },
  'settings.updateMcpLogging': {
    methods: ["PATCH"],
    pattern: '/settings/mcp-logging',
    tokens: [{"old":"/settings/mcp-logging","type":0,"val":"settings","end":""},{"old":"/settings/mcp-logging","type":0,"val":"mcp-logging","end":""}],
    types: placeholder as Registry['settings.updateMcpLogging']['types'],
  },
  'logs.index': {
    methods: ["GET","HEAD"],
    pattern: '/logs',
    tokens: [{"old":"/logs","type":0,"val":"logs","end":""}],
    types: placeholder as Registry['logs.index']['types'],
  },
  'analytics.index': {
    methods: ["GET","HEAD"],
    pattern: '/analytics',
    tokens: [{"old":"/analytics","type":0,"val":"analytics","end":""}],
    types: placeholder as Registry['analytics.index']['types'],
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
  'tokens.update': {
    methods: ["PUT"],
    pattern: '/tokens/:id',
    tokens: [{"old":"/tokens/:id","type":0,"val":"tokens","end":""},{"old":"/tokens/:id","type":1,"val":"id","end":""}],
    types: placeholder as Registry['tokens.update']['types'],
  },
  'tokens.revoke': {
    methods: ["POST"],
    pattern: '/tokens/:id/revoke',
    tokens: [{"old":"/tokens/:id/revoke","type":0,"val":"tokens","end":""},{"old":"/tokens/:id/revoke","type":1,"val":"id","end":""},{"old":"/tokens/:id/revoke","type":0,"val":"revoke","end":""}],
    types: placeholder as Registry['tokens.revoke']['types'],
  },
  'tokens.destroy': {
    methods: ["DELETE"],
    pattern: '/tokens',
    tokens: [{"old":"/tokens","type":0,"val":"tokens","end":""}],
    types: placeholder as Registry['tokens.destroy']['types'],
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
