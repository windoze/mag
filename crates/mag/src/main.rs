//! The `mag` binary: the single assembly point that wires a concrete
//! `mag-core::Engine` into an interface.
//!
//! Startup assembly (`docs/CLI.md` §4.2): the runtime configuration is read
//! from `--config <path>` (default `~/.config/mag/config.toml`, honoring
//! `XDG_CONFIG_HOME`) into a [`ConfigService`], and [`Engine::from_config`]
//! assembles the LLM client, tool registry, source registry, persistence, and
//! approval strategy from the current snapshot. A missing config file is
//! non-fatal — the built-in default configuration (no providers, no external
//! agents, default approval tiers) is used and an info line is logged —
//! while a corrupt or unassemblable configuration exits non-zero with the
//! diagnostic. The engine is then served as an `Arc<dyn MagService>`; by default
//! that is the terminal CLI via [`mag_cli::Cli`], while `--acp` keeps serving the
//! ACP agent interface over stdio via [`mag_acp::serve`].
//! Being the only crate that depends on both `mag-core` and `mag-acp` keeps
//! the `mag-acp` library's dependency boundary intact (`docs/ACP.md` §0/§2).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use agent_client_protocol::Stdio;
use mag_cli::{Cli as TerminalCli, CliOptions};
use mag_core::{ConfigService, Engine};
use mag_service::{MagService, SessionId};

/// Parsed command line.
struct Cli {
    /// Serve the Agent Client Protocol interface over stdio.
    acp: bool,
    /// Explicit `--config <path>` override.
    config: Option<PathBuf>,
    /// Existing session to resume when running the terminal CLI.
    resume: Option<SessionId>,
    /// `--help` / `-h` was passed.
    help: bool,
}

/// Parses the command line (everything after the program name).
fn parse_args(args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli {
        acp: false,
        config: None,
        resume: None,
        help: false,
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--acp" => cli.acp = true,
            "--help" | "-h" => cli.help = true,
            "--config" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--config expects a path argument".to_owned())?;
                cli.config = Some(PathBuf::from(value));
            }
            "--resume" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--resume expects a session id argument".to_owned())?;
                cli.resume = Some(parse_session_id(&value)?);
            }
            _ if arg.starts_with("--config=") => {
                let value = arg.trim_start_matches("--config=");
                if value.is_empty() {
                    return Err("--config expects a non-empty path".to_owned());
                }
                cli.config = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--resume=") => {
                let value = arg.trim_start_matches("--resume=");
                if value.is_empty() {
                    return Err("--resume expects a non-empty session id".to_owned());
                }
                cli.resume = Some(parse_session_id(value)?);
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(cli)
}

/// Parses a wire session id from a command-line argument.
fn parse_session_id(value: &str) -> Result<SessionId, String> {
    SessionId::parse_str(value).map_err(|error| format!("invalid --resume session id: {error}"))
}

/// The default configuration file path (`docs/CLI.md` §4.2, decision D4):
/// `$XDG_CONFIG_HOME/mag/config.toml` when `XDG_CONFIG_HOME` is set, else
/// `~/.config/mag/config.toml`; when neither variable is available the
/// relative `mag/config.toml` is used.
fn default_config_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("mag").join("config.toml");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(home)
            .join(".config")
            .join("mag")
            .join("config.toml");
    }
    PathBuf::from("mag").join("config.toml")
}

/// Prints the usage text.
fn usage(write: &mut dyn std::io::Write) {
    let _ = writeln!(
        write,
        "usage: mag [--config <path>] [--resume <session-id>] [--acp]"
    );
    let _ = writeln!(write, "  (no --acp)       run the terminal CLI");
    let _ = writeln!(
        write,
        "  --acp            serve the Agent Client Protocol interface over stdio"
    );
    let _ = writeln!(
        write,
        "  --config <path>  runtime configuration file (default: ~/.config/mag/config.toml)"
    );
    let _ = writeln!(
        write,
        "  --resume <id>    resume an existing CLI session at startup"
    );
    let _ = writeln!(write, "  --help, -h       show this help");
}

/// Assembles the configured engine: loads the configuration (a missing file
/// yields the built-in default configuration with an info log, never an
/// error) and builds the engine from it.
fn assemble_engine(config_path: &std::path::Path) -> Result<Engine, String> {
    if !config_path.exists() {
        eprintln!(
            "mag: info: no config file at {}; starting with built-in defaults",
            config_path.display()
        );
    }
    let config = ConfigService::load_or_default(config_path)
        .map_err(|error| format!("failed to load config {}: {error}", config_path.display()))?;
    Engine::from_config(Arc::new(config))
        .map_err(|error| format!("failed to assemble from {}: {error}", config_path.display()))
}

#[tokio::main]
async fn main() -> ExitCode {
    use std::io::Write as _;

    let cli = match parse_args(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("mag: {error}");
            usage(&mut std::io::stderr());
            return ExitCode::from(2);
        }
    };

    if cli.help {
        usage(&mut std::io::stdout());
        return ExitCode::SUCCESS;
    }

    if cli.acp && cli.resume.is_some() {
        eprintln!("mag: --resume is only supported by the terminal CLI, not --acp");
        usage(&mut std::io::stderr());
        return ExitCode::from(2);
    }

    let config_path = cli.config.unwrap_or_else(default_config_path);
    let engine = match assemble_engine(&config_path) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("mag: {error}");
            return ExitCode::FAILURE;
        }
    };
    let service: Arc<dyn MagService> = Arc::new(engine);

    if cli.acp {
        let _ = std::io::stderr().flush();
        match mag_acp::serve(service, Stdio::new()).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("mag --acp: {error}");
                ExitCode::FAILURE
            }
        }
    } else {
        let opts = CliOptions {
            resume: cli.resume,
            ..CliOptions::default()
        };
        match TerminalCli::run(service, opts).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("mag: {error}");
                ExitCode::FAILURE
            }
        }
    }
}
