/* eslint-disable prettier/prettier */
/// <reference path="../manifest.d.ts" />

import type { ExtractBody, ExtractErrorResponse, ExtractQuery, ExtractQueryForGet, ExtractResponse } from '@tuyau/core/types'
import type { InferInput, SimpleError } from '@vinejs/vine/types'

export type ParamValue = string | number | bigint | boolean

export interface Registry {
  'health': {
    methods: ["GET","HEAD"]
    pattern: '/health'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: unknown
      errorResponse: unknown
    }
  }
  'oauth.metadata': {
    methods: ["GET","HEAD"]
    pattern: '/.well-known/oauth-authorization-server'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['authorizationMetadata']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['authorizationMetadata']>>>
    }
  }
  'oauth.resourceMetadata': {
    methods: ["GET","HEAD"]
    pattern: '/.well-known/oauth-protected-resource'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['protectedResourceMetadata']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['protectedResourceMetadata']>>>
    }
  }
  'oauth_server.protected_resource_metadata': {
    methods: ["GET","HEAD"]
    pattern: '/.well-known/oauth-protected-resource/mcp'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['protectedResourceMetadata']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['protectedResourceMetadata']>>>
    }
  }
  'oauth.register': {
    methods: ["POST"]
    pattern: '/register'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['register']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['register']>>>
    }
  }
  'oauth.authorize': {
    methods: ["GET","HEAD"]
    pattern: '/authorize'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['authorize']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['authorize']>>>
    }
  }
  'oauth_server.authorize': {
    methods: ["POST"]
    pattern: '/authorize'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['authorize']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['authorize']>>>
    }
  }
  'oauth.token': {
    methods: ["POST"]
    pattern: '/token'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['token']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['token']>>>
    }
  }
  'oauth.revoke': {
    methods: ["POST"]
    pattern: '/revoke'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['revoke']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/oauth_server_controller').default['revoke']>>>
    }
  }
  'onboarding.show': {
    methods: ["GET","HEAD"]
    pattern: '/onboarding'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/onboarding_controller').default['show']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/onboarding_controller').default['show']>>>
    }
  }
  'onboarding.store': {
    methods: ["POST"]
    pattern: '/onboarding'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/user').onboardingValidator)>>
      paramsTuple: []
      params: {}
      query: ExtractQuery<InferInput<(typeof import('#validators/user').onboardingValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/onboarding_controller').default['store']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/onboarding_controller').default['store']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'gateway.handle': {
    methods: ["POST"]
    pattern: '/mcp'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/gateway_controller').default['handle']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/gateway_controller').default['handle']>>>
    }
  }
  'gateway.handleGet': {
    methods: ["GET","HEAD"]
    pattern: '/mcp'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/gateway_controller').default['handle']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/gateway_controller').default['handle']>>>
    }
  }
  'session.create': {
    methods: ["GET","HEAD"]
    pattern: '/login'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/session_controller').default['create']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/session_controller').default['create']>>>
    }
  }
  'session.store': {
    methods: ["POST"]
    pattern: '/login'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/user').loginValidator)>>
      paramsTuple: []
      params: {}
      query: ExtractQuery<InferInput<(typeof import('#validators/user').loginValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/session_controller').default['store']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/session_controller').default['store']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'invites.show': {
    methods: ["GET","HEAD"]
    pattern: '/invite/:token'
    types: {
      body: {}
      paramsTuple: [ParamValue]
      params: { token: ParamValue }
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['show']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['show']>>>
    }
  }
  'invites.accept': {
    methods: ["POST"]
    pattern: '/invite/:token'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/user').acceptInviteValidator)>>
      paramsTuple: [ParamValue]
      params: { token: ParamValue }
      query: ExtractQuery<InferInput<(typeof import('#validators/user').acceptInviteValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['accept']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['accept']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'home': {
    methods: ["GET","HEAD"]
    pattern: '/'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: unknown
      errorResponse: unknown
    }
  }
  'session.destroy': {
    methods: ["POST"]
    pattern: '/logout'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/session_controller').default['destroy']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/session_controller').default['destroy']>>>
    }
  }
  'settings.index': {
    methods: ["GET","HEAD"]
    pattern: '/settings'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/settings_controller').default['index']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/settings_controller').default['index']>>>
    }
  }
  'settings.updateEmail': {
    methods: ["PATCH"]
    pattern: '/settings/email'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/user').updateEmailValidator)>>
      paramsTuple: []
      params: {}
      query: ExtractQuery<InferInput<(typeof import('#validators/user').updateEmailValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/settings_controller').default['updateEmail']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/settings_controller').default['updateEmail']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'settings.updatePassword': {
    methods: ["PATCH"]
    pattern: '/settings/password'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/user').updatePasswordValidator)>>
      paramsTuple: []
      params: {}
      query: ExtractQuery<InferInput<(typeof import('#validators/user').updatePasswordValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/settings_controller').default['updatePassword']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/settings_controller').default['updatePassword']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'settings.updateMcpLogging': {
    methods: ["PATCH"]
    pattern: '/settings/mcp-logging'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/user').updateMcpLoggingValidator)>>
      paramsTuple: []
      params: {}
      query: ExtractQuery<InferInput<(typeof import('#validators/user').updateMcpLoggingValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/settings_controller').default['updateMcpLogging']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/settings_controller').default['updateMcpLogging']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'logs.index': {
    methods: ["GET","HEAD"]
    pattern: '/logs'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: ExtractQueryForGet<InferInput<(typeof import('#validators/mcp_call_log').logsQueryValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/logs_controller').default['index']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/logs_controller').default['index']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'analytics.index': {
    methods: ["GET","HEAD"]
    pattern: '/analytics'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: ExtractQueryForGet<InferInput<(typeof import('#validators/mcp_call_log').analyticsQueryValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/analytics_controller').default['index']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/analytics_controller').default['index']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'invites.index': {
    methods: ["GET","HEAD"]
    pattern: '/invites'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['index']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['index']>>>
    }
  }
  'invites.store': {
    methods: ["POST"]
    pattern: '/invites'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/user').createInviteValidator)>>
      paramsTuple: []
      params: {}
      query: ExtractQuery<InferInput<(typeof import('#validators/user').createInviteValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['store']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['store']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'invites.destroy': {
    methods: ["DELETE"]
    pattern: '/invites/:id'
    types: {
      body: {}
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['destroy']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['destroy']>>>
    }
  }
  'members.destroy': {
    methods: ["DELETE"]
    pattern: '/members/:id'
    types: {
      body: {}
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['destroyMember']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/invites_controller').default['destroyMember']>>>
    }
  }
  'mcps.index': {
    methods: ["GET","HEAD"]
    pattern: '/mcps'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['index']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['index']>>>
    }
  }
  'mcps.store': {
    methods: ["POST"]
    pattern: '/mcps'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/mcp').createMcpValidator)>>
      paramsTuple: []
      params: {}
      query: ExtractQuery<InferInput<(typeof import('#validators/mcp').createMcpValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['store']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['store']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'mcps.oauthCallback': {
    methods: ["GET","HEAD"]
    pattern: '/mcps/oauth/callback'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: ExtractQueryForGet<InferInput<(typeof import('#validators/oauth').oauthCallbackValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['oauthCallback']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['oauthCallback']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'mcps.show': {
    methods: ["GET","HEAD"]
    pattern: '/mcps/:id'
    types: {
      body: {}
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['show']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['show']>>>
    }
  }
  'mcps.update': {
    methods: ["PUT"]
    pattern: '/mcps/:id'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/mcp').updateMcpValidator)>>
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: ExtractQuery<InferInput<(typeof import('#validators/mcp').updateMcpValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['update']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['update']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'mcps.destroy': {
    methods: ["DELETE"]
    pattern: '/mcps/:id'
    types: {
      body: {}
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['destroy']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['destroy']>>>
    }
  }
  'mcps.probe': {
    methods: ["POST"]
    pattern: '/mcps/:id/probe'
    types: {
      body: {}
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['probe']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['probe']>>>
    }
  }
  'mcps.oauthStart': {
    methods: ["GET","HEAD"]
    pattern: '/mcps/:id/oauth/start'
    types: {
      body: {}
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['oauthStart']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/mcps_controller').default['oauthStart']>>>
    }
  }
  'tokens.index': {
    methods: ["GET","HEAD"]
    pattern: '/tokens'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['index']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['index']>>>
    }
  }
  'tokens.store': {
    methods: ["POST"]
    pattern: '/tokens'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/mcp').createAccessTokenValidator)>>
      paramsTuple: []
      params: {}
      query: ExtractQuery<InferInput<(typeof import('#validators/mcp').createAccessTokenValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['store']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['store']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'tokens.update': {
    methods: ["PUT"]
    pattern: '/tokens/:id'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/mcp').updateAccessTokenValidator)>|InferInput<(typeof import('#validators/mcp').accessTokenParamsValidator)>>
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: ExtractQuery<InferInput<(typeof import('#validators/mcp').updateAccessTokenValidator)>|InferInput<(typeof import('#validators/mcp').accessTokenParamsValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['update']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['update']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
  'tokens.revoke': {
    methods: ["POST"]
    pattern: '/tokens/:id/revoke'
    types: {
      body: {}
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['revoke']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['revoke']>>>
    }
  }
  'tokens.destroy': {
    methods: ["DELETE"]
    pattern: '/tokens'
    types: {
      body: ExtractBody<InferInput<(typeof import('#validators/mcp').deleteAccessTokensValidator)>>
      paramsTuple: []
      params: {}
      query: ExtractQuery<InferInput<(typeof import('#validators/mcp').deleteAccessTokensValidator)>>
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['destroy']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/access_tokens_controller').default['destroy']>>> | { status: 422; response: { errors: SimpleError[] } }
    }
  }
}
