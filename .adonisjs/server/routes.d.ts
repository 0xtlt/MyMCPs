import '@adonisjs/core/types/http'

type ParamValue = string | number | bigint | boolean

export type ScannedRoutes = {
  ALL: {
    'health': { paramsTuple?: []; params?: {} }
    'oauth.metadata': { paramsTuple?: []; params?: {} }
    'oauth.resourceMetadata': { paramsTuple?: []; params?: {} }
    'oauth_server.protected_resource_metadata': { paramsTuple?: []; params?: {} }
    'oauth.register': { paramsTuple?: []; params?: {} }
    'oauth.authorize': { paramsTuple?: []; params?: {} }
    'oauth_server.authorize': { paramsTuple?: []; params?: {} }
    'oauth.token': { paramsTuple?: []; params?: {} }
    'oauth.revoke': { paramsTuple?: []; params?: {} }
    'onboarding.show': { paramsTuple?: []; params?: {} }
    'onboarding.store': { paramsTuple?: []; params?: {} }
    'gateway.handle': { paramsTuple?: []; params?: {} }
    'gateway.handleGet': { paramsTuple?: []; params?: {} }
    'session.create': { paramsTuple?: []; params?: {} }
    'session.store': { paramsTuple?: []; params?: {} }
    'invites.show': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'invites.accept': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'home': { paramsTuple?: []; params?: {} }
    'session.destroy': { paramsTuple?: []; params?: {} }
    'settings.index': { paramsTuple?: []; params?: {} }
    'settings.updateEmail': { paramsTuple?: []; params?: {} }
    'settings.updatePassword': { paramsTuple?: []; params?: {} }
    'settings.updateMcpLogging': { paramsTuple?: []; params?: {} }
    'logs.index': { paramsTuple?: []; params?: {} }
    'analytics.index': { paramsTuple?: []; params?: {} }
    'invites.index': { paramsTuple?: []; params?: {} }
    'invites.store': { paramsTuple?: []; params?: {} }
    'invites.destroy': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'members.destroy': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.index': { paramsTuple?: []; params?: {} }
    'mcps.store': { paramsTuple?: []; params?: {} }
    'mcps.oauthCallback': { paramsTuple?: []; params?: {} }
    'mcps.show': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.update': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.destroy': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.probe': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.updateNpm': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.oauthStart': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.index': { paramsTuple?: []; params?: {} }
    'tokens.store': { paramsTuple?: []; params?: {} }
    'tokens.update': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.revoke': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.destroy': { paramsTuple?: []; params?: {} }
  }
  GET: {
    'health': { paramsTuple?: []; params?: {} }
    'oauth.metadata': { paramsTuple?: []; params?: {} }
    'oauth.resourceMetadata': { paramsTuple?: []; params?: {} }
    'oauth_server.protected_resource_metadata': { paramsTuple?: []; params?: {} }
    'oauth.authorize': { paramsTuple?: []; params?: {} }
    'onboarding.show': { paramsTuple?: []; params?: {} }
    'gateway.handleGet': { paramsTuple?: []; params?: {} }
    'session.create': { paramsTuple?: []; params?: {} }
    'invites.show': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'home': { paramsTuple?: []; params?: {} }
    'settings.index': { paramsTuple?: []; params?: {} }
    'logs.index': { paramsTuple?: []; params?: {} }
    'analytics.index': { paramsTuple?: []; params?: {} }
    'invites.index': { paramsTuple?: []; params?: {} }
    'mcps.index': { paramsTuple?: []; params?: {} }
    'mcps.oauthCallback': { paramsTuple?: []; params?: {} }
    'mcps.show': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.oauthStart': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.index': { paramsTuple?: []; params?: {} }
  }
  HEAD: {
    'health': { paramsTuple?: []; params?: {} }
    'oauth.metadata': { paramsTuple?: []; params?: {} }
    'oauth.resourceMetadata': { paramsTuple?: []; params?: {} }
    'oauth_server.protected_resource_metadata': { paramsTuple?: []; params?: {} }
    'oauth.authorize': { paramsTuple?: []; params?: {} }
    'onboarding.show': { paramsTuple?: []; params?: {} }
    'gateway.handleGet': { paramsTuple?: []; params?: {} }
    'session.create': { paramsTuple?: []; params?: {} }
    'invites.show': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'home': { paramsTuple?: []; params?: {} }
    'settings.index': { paramsTuple?: []; params?: {} }
    'logs.index': { paramsTuple?: []; params?: {} }
    'analytics.index': { paramsTuple?: []; params?: {} }
    'invites.index': { paramsTuple?: []; params?: {} }
    'mcps.index': { paramsTuple?: []; params?: {} }
    'mcps.oauthCallback': { paramsTuple?: []; params?: {} }
    'mcps.show': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.oauthStart': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.index': { paramsTuple?: []; params?: {} }
  }
  POST: {
    'oauth.register': { paramsTuple?: []; params?: {} }
    'oauth_server.authorize': { paramsTuple?: []; params?: {} }
    'oauth.token': { paramsTuple?: []; params?: {} }
    'oauth.revoke': { paramsTuple?: []; params?: {} }
    'onboarding.store': { paramsTuple?: []; params?: {} }
    'gateway.handle': { paramsTuple?: []; params?: {} }
    'session.store': { paramsTuple?: []; params?: {} }
    'invites.accept': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'session.destroy': { paramsTuple?: []; params?: {} }
    'invites.store': { paramsTuple?: []; params?: {} }
    'mcps.store': { paramsTuple?: []; params?: {} }
    'mcps.probe': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.updateNpm': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.store': { paramsTuple?: []; params?: {} }
    'tokens.revoke': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
  }
  PATCH: {
    'settings.updateEmail': { paramsTuple?: []; params?: {} }
    'settings.updatePassword': { paramsTuple?: []; params?: {} }
    'settings.updateMcpLogging': { paramsTuple?: []; params?: {} }
  }
  DELETE: {
    'invites.destroy': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'members.destroy': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.destroy': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.destroy': { paramsTuple?: []; params?: {} }
  }
  PUT: {
    'mcps.update': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.update': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
  }
}
declare module '@adonisjs/core/types/http' {
  export interface RoutesList extends ScannedRoutes {}
}