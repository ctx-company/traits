//! `ctx` binary entry point.

use std::process::ExitCode;

use ctx_traits_cli::app;

fn main() -> ExitCode {
    // First, before argument parsing and before anything spawns a thread: a
    // `ctx __ctx-setsid-exec <argv…>` invocation is the fork-free
    // session-detach shim and never returns (see
    // `ctx_traits_io::command::SETSID_EXEC_SENTINEL`).
    ctx_traits_io::command::maybe_setsid_exec_shim();
    app::entry::run(std::env::args_os())
}
