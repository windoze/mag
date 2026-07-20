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
//! ACP agent interface over stdio via [`mag_acp::serve`] and `--web` serves the
//! browser interface through [`mag_web`].
//! Being the only crate that depends on both `mag-core` and `mag-acp` keeps
//! the `mag-acp` library's dependency boundary intact (`docs/ACP.md` §0/§2).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use std::io::Write as _;

use agent_client_protocol::Stdio;
use mag_cli::{Cli as TerminalCli, CliOptions};
use mag_core::{ConfigService, Engine};
use mag_service::{MagService, SessionId};
use tracing_subscriber::EnvFilter;

/// Installs the process-wide tracing subscriber.
///
/// All diagnostics (mag-core's warn-level tool/agent fallbacks, apply-step
/// rejections, listener panics) go to **stderr**; stdout stays reserved for
/// user output in the terminal CLI and, critically, for the ACP stdio
/// JSON-RPC stream under `--acp`, which any stdout log line would corrupt.
/// The level filter is `MAG_LOG` first, then `RUST_LOG`, defaulting to
/// `warn` — the prototype logs sparingly, so warn keeps the terminal quiet
/// while surfacing every diagnostic mag-core currently emits.
fn init_tracing() {
    let filter = EnvFilter::try_from_env("MAG_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// Parsed command line.
#[derive(Debug)]
struct Cli {
    /// Serve the Agent Client Protocol interface over stdio.
    acp: bool,
    /// Serve the browser web interface over HTTP.
    web: bool,
    /// Explicit `--config <path>` override.
    config: Option<PathBuf>,
    /// Existing session to resume when running the terminal CLI.
    resume: Option<SessionId>,
    /// Web bind host.
    web_host: IpAddr,
    /// Web bind port.
    web_port: u16,
    /// Web bearer token supplied externally.
    web_token: Option<String>,
    /// Disable web API auth when the bind policy allows it.
    web_no_auth: bool,
    /// Any web-only option was passed.
    web_option_seen: bool,
    /// `--help` / `-h` was passed.
    help: bool,
}

/// Parses the command line (everything after the program name).
fn parse_args(args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli {
        acp: false,
        web: false,
        config: None,
        resume: None,
        web_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        web_port: 8080,
        web_token: None,
        web_no_auth: false,
        web_option_seen: false,
        help: false,
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--acp" => cli.acp = true,
            "--web" => cli.web = true,
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
            "--host" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--host expects an IP address argument".to_owned())?;
                cli.web_option_seen = true;
                cli.web_host = parse_host(&value)?;
            }
            "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--port expects a TCP port argument".to_owned())?;
                cli.web_option_seen = true;
                cli.web_port = parse_port(&value)?;
            }
            "--token" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--token expects a non-empty token argument".to_owned())?;
                if value.is_empty() {
                    return Err("--token expects a non-empty token".to_owned());
                }
                cli.web_option_seen = true;
                cli.web_token = Some(value);
            }
            "--no-auth" => {
                cli.web_option_seen = true;
                cli.web_no_auth = true;
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
            _ if arg.starts_with("--host=") => {
                let value = arg.trim_start_matches("--host=");
                if value.is_empty() {
                    return Err("--host expects a non-empty IP address".to_owned());
                }
                cli.web_option_seen = true;
                cli.web_host = parse_host(value)?;
            }
            _ if arg.starts_with("--port=") => {
                let value = arg.trim_start_matches("--port=");
                if value.is_empty() {
                    return Err("--port expects a non-empty TCP port".to_owned());
                }
                cli.web_option_seen = true;
                cli.web_port = parse_port(value)?;
            }
            _ if arg.starts_with("--token=") => {
                let value = arg.trim_start_matches("--token=");
                if value.is_empty() {
                    return Err("--token expects a non-empty token".to_owned());
                }
                cli.web_option_seen = true;
                cli.web_token = Some(value.to_owned());
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }

    if cli.acp && cli.web {
        return Err("--acp and --web cannot be used together".to_owned());
    }
    if cli.web_option_seen && !cli.web {
        return Err("--host, --port, --token, and --no-auth require --web".to_owned());
    }
    if cli.web_token.is_some() && cli.web_no_auth {
        return Err("--token and --no-auth cannot be used together".to_owned());
    }

    Ok(cli)
}

/// Parses a wire session id from a command-line argument.
fn parse_session_id(value: &str) -> Result<SessionId, String> {
    SessionId::parse_str(value).map_err(|error| format!("invalid --resume session id: {error}"))
}

/// Parses a web bind host from a command-line argument.
fn parse_host(value: &str) -> Result<IpAddr, String> {
    value
        .parse()
        .map_err(|error| format!("invalid --host IP address: {error}"))
}

/// Parses a web bind port from a command-line argument.
fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse()
        .map_err(|error| format!("invalid --port TCP port: {error}"))
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
        "usage: mag [--config <path>] [--resume <session-id>] [--acp] [--web [--host <addr>] [--port <n>] [--token <t>] [--no-auth]]"
    );
    let _ = writeln!(write, "  (no --acp/--web) run the terminal CLI");
    let _ = writeln!(
        write,
        "  --acp            serve the Agent Client Protocol interface over stdio"
    );
    let _ = writeln!(write, "  --web            serve the browser web interface");
    let _ = writeln!(
        write,
        "  --host <addr>    web bind IP address (default: 127.0.0.1; requires --web)"
    );
    let _ = writeln!(
        write,
        "  --port <n>       web bind TCP port (default: 8080; requires --web)"
    );
    let _ = writeln!(
        write,
        "  --token <t>      web bearer token supplied by caller (requires --web)"
    );
    let _ = writeln!(
        write,
        "  --no-auth        disable web API auth on loopback binds only (requires --web)"
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

/// Converts parsed web CLI flags into `mag-web` serve options.
fn web_serve_options(cli: &Cli) -> mag_web::ServeOptions {
    mag_web::ServeOptions {
        host: cli.web_host,
        port: cli.web_port,
        token_policy: web_token_policy(cli),
        static_assets_dir: None,
        heartbeat_interval: None,
    }
}

/// Selects the web token strategy requested by the CLI.
fn web_token_policy(cli: &Cli) -> mag_web::TokenPolicy {
    if let Some(token) = &cli.web_token {
        mag_web::TokenPolicy::Provided(token.clone())
    } else if cli.web_no_auth {
        mag_web::TokenPolicy::Disabled
    } else {
        mag_web::TokenPolicy::Generate
    }
}

/// Runs the web interface and prints the access URL after the socket is bound.
async fn run_web(service: Arc<dyn MagService>, cli: &Cli) -> Result<(), String> {
    let prepared = mag_web::prepare_router(service, web_serve_options(cli))
        .map_err(|error| format!("failed to prepare web server: {error}"))?;
    let address = prepared.options().address();
    let auth_token = prepared.options().auth_token().map(str::to_owned);
    let externally_provided_token = cli.web_token.is_some();
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| format!("failed to bind web server at {address}: {error}"))?;
    let local_addr = listener
        .local_addr()
        .map_err(|error| format!("failed to read web server address: {error}"))?;

    print_web_startup(
        local_addr,
        auth_token.as_deref(),
        externally_provided_token,
        &mut std::io::stderr(),
    );

    mag_web::serve_prepared(listener, prepared)
        .await
        .map_err(|error| format!("web server exited with error: {error}"))
}

/// Prints startup details without leaking externally supplied bearer tokens.
fn print_web_startup(
    address: SocketAddr,
    auth_token: Option<&str>,
    externally_provided_token: bool,
    write: &mut dyn std::io::Write,
) {
    let base_url = web_base_url(address);
    match (auth_token, externally_provided_token) {
        (Some(_), true) => {
            let _ = writeln!(write, "mag web listening on {base_url}");
            let _ = writeln!(
                write,
                "mag web auth uses the externally supplied token; it is not printed"
            );
        }
        (Some(token), false) => {
            let _ = writeln!(write, "mag web listening on {base_url}#t={token}");
        }
        (None, _) => {
            let _ = writeln!(write, "mag web listening on {base_url}");
            let _ = writeln!(
                write,
                "warning: mag web auth is disabled (--no-auth); any local process can call the API"
            );
        }
    }
}

/// Formats a socket address as an HTTP base URL.
fn web_base_url(address: SocketAddr) -> String {
    match address.ip() {
        IpAddr::V4(ip) => format!("http://{ip}:{}/", address.port()),
        IpAddr::V6(ip) => format!("http://[{ip}]:{}/", address.port()),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

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

    if (cli.acp || cli.web) && cli.resume.is_some() {
        eprintln!("mag: --resume is only supported by the terminal CLI");
        usage(&mut std::io::stderr());
        return ExitCode::from(2);
    }

    let config_path = cli.config.clone().unwrap_or_else(default_config_path);
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
    } else if cli.web {
        match run_web(service, &cli).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("mag --web: {error}");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, String> {
        parse_args(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn parses_web_flags_into_serve_options() {
        let cli = parse(&[
            "--web",
            "--host",
            "0.0.0.0",
            "--port=9090",
            "--token",
            "external-token",
        ])
        .expect("web flags parse");

        assert!(cli.web);
        assert_eq!(cli.web_host, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(cli.web_port, 9090);
        assert_eq!(cli.web_token.as_deref(), Some("external-token"));
        assert_eq!(
            web_serve_options(&cli).token_policy,
            mag_web::TokenPolicy::Provided("external-token".to_owned())
        );
    }

    #[test]
    fn web_only_flags_require_web_mode() {
        let error =
            parse(&["--host", "127.0.0.1"]).expect_err("web-only flags without --web must fail");

        assert!(error.contains("require --web"));
    }

    #[test]
    fn acp_and_web_are_mutually_exclusive() {
        let error = parse(&["--acp", "--web"]).expect_err("--acp with --web must fail");

        assert!(error.contains("--acp and --web"));
    }

    #[test]
    fn token_and_no_auth_are_mutually_exclusive() {
        let error = parse(&["--web", "--token", "secret", "--no-auth"])
            .expect_err("conflicting auth flags must fail");

        assert!(error.contains("--token and --no-auth"));
    }

    #[test]
    fn web_startup_prints_generated_but_not_external_tokens() {
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let mut generated = Vec::new();
        print_web_startup(address, Some("generated-token"), false, &mut generated);
        let generated = String::from_utf8(generated).expect("utf-8 output");
        assert!(generated.contains("http://127.0.0.1:8080/#t=generated-token"));

        let mut external = Vec::new();
        print_web_startup(address, Some("external-token"), true, &mut external);
        let external = String::from_utf8(external).expect("utf-8 output");
        assert!(external.contains("http://127.0.0.1:8080/"));
        assert!(!external.contains("external-token"));
        assert!(external.contains("externally supplied token"));
    }

    #[test]
    fn web_startup_warns_when_auth_is_disabled() {
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let mut output = Vec::new();
        print_web_startup(address, None, false, &mut output);
        let output = String::from_utf8(output).expect("utf-8 output");
        assert!(output.contains("http://127.0.0.1:8080/"));
        assert!(output.contains("warning: mag web auth is disabled"));
    }
}
