import '@adonisjs/core/types/http'

type ParamValue = string | number | bigint | boolean

export type ScannedRoutes = {
  ALL: {
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
    'mcps.oauthStart': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.index': { paramsTuple?: []; params?: {} }
    'tokens.store': { paramsTuple?: []; params?: {} }
    'tokens.revoke': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
  }
  GET: {
    'onboarding.show': { paramsTuple?: []; params?: {} }
    'gateway.handleGet': { paramsTuple?: []; params?: {} }
    'session.create': { paramsTuple?: []; params?: {} }
    'invites.show': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'home': { paramsTuple?: []; params?: {} }
    'invites.index': { paramsTuple?: []; params?: {} }
    'mcps.index': { paramsTuple?: []; params?: {} }
    'mcps.oauthCallback': { paramsTuple?: []; params?: {} }
    'mcps.show': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.oauthStart': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.index': { paramsTuple?: []; params?: {} }
  }
  HEAD: {
    'onboarding.show': { paramsTuple?: []; params?: {} }
    'gateway.handleGet': { paramsTuple?: []; params?: {} }
    'session.create': { paramsTuple?: []; params?: {} }
    'invites.show': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'home': { paramsTuple?: []; params?: {} }
    'invites.index': { paramsTuple?: []; params?: {} }
    'mcps.index': { paramsTuple?: []; params?: {} }
    'mcps.oauthCallback': { paramsTuple?: []; params?: {} }
    'mcps.show': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.oauthStart': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.index': { paramsTuple?: []; params?: {} }
  }
  POST: {
    'onboarding.store': { paramsTuple?: []; params?: {} }
    'gateway.handle': { paramsTuple?: []; params?: {} }
    'session.store': { paramsTuple?: []; params?: {} }
    'invites.accept': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'session.destroy': { paramsTuple?: []; params?: {} }
    'invites.store': { paramsTuple?: []; params?: {} }
    'mcps.store': { paramsTuple?: []; params?: {} }
    'mcps.probe': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'tokens.store': { paramsTuple?: []; params?: {} }
    'tokens.revoke': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
  }
  DELETE: {
    'invites.destroy': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'members.destroy': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'mcps.destroy': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
  }
  PUT: {
    'mcps.update': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
  }
}
declare module '@adonisjs/core/types/http' {
  export interface RoutesList extends ScannedRoutes {}
}