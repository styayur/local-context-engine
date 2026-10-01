//! `lce-index-service.exe` — the privileged index service.
//!
//! ```text
//! lce-index-service                     # run as a service (what the SCM invokes)
//! lce-index-service --console           # run in the foreground, for debugging
//! lce-index-service install             # register with the service control manager
//! lce-index-service start | stop | status | uninstall
//! ```
//!
//! Nothing here is reachable over the IPC boundary: the pipe protocol cannot
//! express "install", "start" or "stop", so a client cannot reconfigure the
//! service it talks to.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use index_protocol::DEFAULT_PIPE_NAME;
use index_service::server::{serve_loop, IndexServer};
use search_daemon::SearchService;

mod scm;

/// Global stop flag shared with the service control handler.
static STOP: AtomicBool = AtomicBool::new(false);

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command = arguments.first().map(String::as_str).unwrap_or("run");

    let exit = match command {
        "--help" | "-h" | "help" => {
            print_usage();
            0
        }
        "--console" | "console" | "run-console" => run_console(),
        "install" => scm::install(),
        "uninstall" | "remove" => scm::uninstall(),
        "start" => scm::start(),
        "stop" => scm::stop(),
        "status" => scm::status(),
        "run" => scm::run_as_service(),
        other => {
            eprintln!("unknown command `{other}`");
            print_usage();
            2
        }
    };
    std::process::exit(exit);
}

fn print_usage() {
    println!(
        "lce-index-service — privileged Local Context Engine index service\n\n\
         USAGE:\n  \
           lce-index-service                 run as a Windows service (SCM entry point)\n  \
           lce-index-service --console       run in the foreground on the default pipe\n  \
           lce-index-service install         register the service (needs administrator)\n  \
           lce-index-service start           start the registered service\n  \
           lce-index-service stop            stop the registered service\n  \
           lce-index-service status          report the service state\n  \
           lce-index-service uninstall       stop and remove the service\n\n\
         The service owns the NTFS MFT and the per-volume USN journals. Front ends\n\
         talk to it over the named pipe `{DEFAULT_PIPE_NAME}`; they never need\n\
         administrator rights of their own."
    );
}

fn run_console() -> i32 {
    init_tracing();
    println!("listening on \\\\.\\pipe\\{DEFAULT_PIPE_NAME} (terminate the process to stop)");
    run_server(DEFAULT_PIPE_NAME)
}

/// Run the pipe server until a stop is requested.
///
/// The stop flag is the process-wide one that the service control handler
/// flips, so an SCM `stop` and a console run share exactly one shutdown path.
pub fn run_server(pipe_name: &str) -> i32 {
    let service = Arc::new(SearchService::bootstrap());
    let server = IndexServer::new(service, pipe_name);
    match serve_loop(&server, pipe_name, &STOP) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("the index service could not start: {}", error.hint());
            1
        }
    }
}

/// Ask the serve loop to stop.
pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("LCE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
