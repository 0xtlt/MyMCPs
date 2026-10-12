//! `mymcps-sandbox-exec <policy> [<program> [<argument>...]]`: what
//! `mymcps sandbox:exec` does, without the server around it.

use std::process::ExitCode;

fn main() -> ExitCode {
    mymcps_sandbox::launch(std::env::args_os().skip(1))
}
