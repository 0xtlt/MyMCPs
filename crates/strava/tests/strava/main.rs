//! The built-in Strava MCP against a fake Strava: the ports of
//! `tests/unit/builtin_strava.spec.ts`, `tests/unit/vine_builtin_strava.spec.ts`,
//! `tests/unit/vine_builtin_input_schemas.spec.ts` for the tools of Strava, and
//! of what `tests/functional/builtin_strava_mcp.spec.ts` asks of the tools.
//! `differential` then holds the port to what the TypeScript itself answers.

mod builtin_strava;
mod differential;
mod support;
mod vine_builtin_input_schemas;
mod vine_builtin_strava;
