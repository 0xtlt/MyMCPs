//! The schedule npm MCPs are refreshed on (`app/services/mcp_auto_update_cron.ts`).
//! The scheduler that runs it owns the reading of the expression: what the
//! Settings page accepts is what the scheduler can run.

pub use mymcps_gateway::auto_update::cron::{is_valid_five_field_cron, parse_five_field_cron};
