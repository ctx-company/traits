//! `ctx` binary entry point.

use std::process::ExitCode;

use ctx_traits_cli::app;

fn main() -> ExitCode {
    app::entry::run(std::env::args_os())
}
