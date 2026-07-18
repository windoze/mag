//! The `mag` binary: the single assembly point that wires a concrete
//! `mag-core::Engine` into an interface.
//!
//! It constructs an [`Engine`] as an
//! `Arc<dyn MagService>` and, for the `--acp` subcommand, serves the ACP
//! agent interface over stdio via [`mag_acp::serve`]. Being the only crate that
//! depends on both `mag-core` and `mag-acp` keeps the `mag-acp` library's
//! dependency boundary intact (`docs/ACP.md` §0/§2).

use std::process::ExitCode;
use std::sync::Arc;

use agent_client_protocol::Stdio;
use mag_core::Engine;
use mag_service::MagService;

#[tokio::main]
async fn main() -> ExitCode {
    let run_acp = std::env::args().skip(1).any(|arg| arg == "--acp");

    if !run_acp {
        eprintln!("usage: mag --acp");
        eprintln!("  --acp   serve the Agent Client Protocol interface over stdio");
        return ExitCode::from(2);
    }

    // Assemble the transport-neutral engine as the service facade. Provider/model
    // wiring arrives with source configuration in a later milestone; for now the
    // engine manages sessions and serves the ACP handshake.
    let service: Arc<dyn MagService> = Arc::new(Engine::new());

    match mag_acp::serve(service, Stdio::new()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mag --acp: {error}");
            ExitCode::FAILURE
        }
    }
}
