import '@adonisjs/core/types/http'

type ParamValue = string | number | bigint | boolean

export type ScannedRoutes = {
  ALL: {
    'onboarding.show': { paramsTuple?: []; params?: {} }
    'onboarding.store': { paramsTuple?: []; params?: {} }
    'session.create': { paramsTuple?: []; params?: {} }
    'session.store': { paramsTuple?: []; params?: {} }
    'invites.show': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'invites.accept': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'home': { paramsTuple?: []; params?: {} }
    'session.destroy': { paramsTuple?: []; params?: {} }
    'invites.index': { paramsTuple?: []; params?: {} }
    'invites.store': { paramsTuple?: []; params?: {} }
  }
  GET: {
    'onboarding.show': { paramsTuple?: []; params?: {} }
    'session.create': { paramsTuple?: []; params?: {} }
    'invites.show': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'home': { paramsTuple?: []; params?: {} }
    'invites.index': { paramsTuple?: []; params?: {} }
  }
  HEAD: {
    'onboarding.show': { paramsTuple?: []; params?: {} }
    'session.create': { paramsTuple?: []; params?: {} }
    'invites.show': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'home': { paramsTuple?: []; params?: {} }
    'invites.index': { paramsTuple?: []; params?: {} }
  }
  POST: {
    'onboarding.store': { paramsTuple?: []; params?: {} }
    'session.store': { paramsTuple?: []; params?: {} }
    'invites.accept': { paramsTuple: [ParamValue]; params: {'token': ParamValue} }
    'session.destroy': { paramsTuple?: []; params?: {} }
    'invites.store': { paramsTuple?: []; params?: {} }
  }
}
declare module '@adonisjs/core/types/http' {
  export interface RoutesList extends ScannedRoutes {}
}