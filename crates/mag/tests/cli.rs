//! Bin-level smoke tests for the `mag` binary (`TODO.md` M6-5).
//!
//! All offline: the binary is driven with `--config` pointing at a tempdir
//! sample or tempdir default, the CLI path is driven through piped stdio, the
//! ACP handshake is answered over piped stdio with a fake API key injected
//! through the child environment, and no network is touched (assembly builds the
//! adapter but never calls it).

use std::{
    fs,
    io::{BufRead, BufReader, ErrorKind, Write},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CHILD_TIMEOUT: Duration = Duration::from_secs(10);

/// Unique temp directory per test, removed on drop (same pattern as the
/// `TempConfigDir` helpers in mag-core).
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "mag-bin-{tag}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn config_path(&self) -> PathBuf {
        self.0.join("config.toml")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The binary under test (built by cargo alongside the integration test).
fn mag() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mag"))
}

/// Spawns `mag --acp --config <path>` with piped stdio and extra environment.
fn spawn_acp(config: &std::path::Path, envs: &[(&str, &str)]) -> Child {
    let mut command = mag();
    command
        .arg("--acp")
        .arg("--config")
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in envs {
        command.env(name, value);
    }
    command.spawn().expect("spawn mag --acp")
}

/// Runs the default terminal CLI with piped stdin/stdout.
fn run_cli(config: &std::path::Path, extra_args: &[String], input: &str) -> Output {
    run_cli_with_env(config, extra_args, input, &[])
}

/// Runs the default terminal CLI with additional environment variables.
fn run_cli_with_env(
    config: &std::path::Path,
    extra_args: &[String],
    input: &str,
    envs: &[(&str, &str)],
) -> Output {
    let mut command = mag();
    command.arg("--config").arg(config);
    for arg in extra_args {
        command.arg(arg);
    }
    for (name, value) in envs {
        command.env(name, value);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mag CLI");

    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        write_child_stdin(&mut stdin, input.as_bytes(), "CLI stdin");
    }

    wait_with_timeout(child, "mag CLI")
}

fn write_child_stdin(stdin: &mut impl Write, bytes: &[u8], context: &str) {
    match stdin.write_all(bytes) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::BrokenPipe => return,
        Err(error) => panic!("write {context}: {error}"),
    }
    match stdin.flush() {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::BrokenPipe => {}
        Err(error) => panic!("flush {context}: {error}"),
    }
}

fn wait_with_timeout(mut child: Child, context: &str) -> Output {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().expect("collect child output"),
            Ok(None) => {}
            Err(error) => panic!("wait for {context}: {error}"),
        }
        if start.elapsed() >= CHILD_TIMEOUT {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .expect("collect timed-out child output");
            panic!("{context} timed out after {CHILD_TIMEOUT:?}: {output:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Extracts the first `[session <id>]` line printed by the CLI.
fn first_session_id(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("[session ")
                .and_then(|rest| rest.strip_suffix(']'))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("stdout did not contain a session line: {stdout}"))
}

/// Performs the ACP `initialize` handshake against `child`, returning the
/// response line. Fails the test when no answer arrives within 10 seconds.
fn initialize_handshake(child: &mut Child) -> String {
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        let _ = tx.send(lines.next());
    });

    stdin
        .write_all(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#,
        )
        .expect("write initialize");
    stdin.write_all(b"\n").expect("write newline");
    stdin.flush().expect("flush initialize");

    rx.recv_timeout(Duration::from_secs(10))
        .expect("mag must answer initialize within 10s")
        .expect("stdout readable")
        .expect("a response line")
}

#[test]
fn help_exits_zero_and_documents_the_flags() {
    let output = mag().arg("--help").output().expect("run mag --help");

    assert!(output.status.success(), "--help exits 0: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("terminal CLI"),
        "help documents default CLI: {stdout}"
    );
    assert!(stdout.contains("--acp"), "help documents --acp: {stdout}");
    assert!(
        stdout.contains("--config"),
        "help documents --config: {stdout}"
    );
    assert!(
        stdout.contains("--resume"),
        "help documents --resume: {stdout}"
    );
}

#[test]
fn unknown_argument_is_rejected_with_usage() {
    let output = mag().arg("--bogus").output().expect("run mag --bogus");

    assert_eq!(output.status.code(), Some(2), "usage error exits 2");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown argument"), "got: {stderr}");
}

#[test]
fn default_invocation_runs_the_terminal_cli() {
    let dir = TempDir::new("default-cli");

    let output = run_cli(&dir.config_path(), &[], "/quit\n");

    assert!(output.status.success(), "default CLI exits 0: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[session "),
        "CLI creates a session: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("built-in defaults"),
        "missing config uses defaults with an info diagnostic: {stderr}"
    );
}

#[test]
fn default_cli_loads_and_rejects_a_corrupt_config() {
    let dir = TempDir::new("corrupt-cli");
    fs::write(dir.config_path(), "not toml = [").expect("write junk config");

    let output = run_cli(&dir.config_path(), &[], "/quit\n");

    assert!(!output.status.success(), "corrupt config exits non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to load config"), "got: {stderr}");
}

#[test]
fn resume_flag_resumes_a_persisted_cli_session() {
    let dir = TempDir::new("resume-cli");
    let persist = dir.0.join("sessions");
    fs::write(
        dir.config_path(),
        format!(
            r#"
[providers.openai]
wire = "openai"
api_key = {{ env = "MAG_BIN_RESUME_API_KEY" }}

[agents.default]
provider = "openai"
model = "gpt-5-codex"

[session]
persist_path = {:?}
"#,
            persist.to_string_lossy()
        ),
    )
    .expect("write config");

    let envs = [("MAG_BIN_RESUME_API_KEY", "sk-bin-resume")];
    let first = run_cli_with_env(&dir.config_path(), &[], "/quit\n", &envs);
    assert!(first.status.success(), "first CLI run exits 0: {first:?}");
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    let session_id = first_session_id(&first_stdout);

    let second = run_cli_with_env(
        &dir.config_path(),
        &["--resume".to_owned(), session_id.clone()],
        "/quit\n",
        &envs,
    );

    assert!(second.status.success(), "resumed CLI exits 0: {second:?}");
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        second_stdout.contains(&format!("[session {session_id} resumed]")),
        "CLI resumes the persisted session: {second_stdout}"
    );
}

#[test]
fn acp_handshake_with_a_sample_config_and_injected_secret() {
    let dir = TempDir::new("sample");
    fs::write(
        dir.config_path(),
        r#"
[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "MAG_BIN_TEST_API_KEY" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"

[tools.shell]
approval = "ask"
"#,
    )
    .expect("write sample config");

    let mut child = spawn_acp(
        &dir.config_path(),
        &[("MAG_BIN_TEST_API_KEY", "sk-bin-smoke")],
    );
    let response = initialize_handshake(&mut child);

    assert!(
        response.contains("agentCapabilities"),
        "initialize response carries agent capabilities: {response}"
    );
    child.kill().expect("kill mag subprocess");
    let _ = child.wait();
}

#[test]
fn acp_handshake_with_a_missing_config_file_uses_defaults() {
    let dir = TempDir::new("missing");
    // No file written: load_or_default yields the built-in default
    // configuration (clientless engine), and the handshake still works.

    let mut child = spawn_acp(&dir.config_path(), &[]);
    let response = initialize_handshake(&mut child);

    assert!(
        response.contains("agentCapabilities"),
        "initialize response carries agent capabilities: {response}"
    );
    child.kill().expect("kill mag subprocess");
    let _ = child.wait();
}

#[test]
fn acp_with_an_unresolvable_secret_exits_nonzero_naming_the_reference() {
    let dir = TempDir::new("secret");
    fs::write(
        dir.config_path(),
        r#"
[providers.anthropic]
wire = "anthropic"
api_key = { env = "MAG_BIN_TEST_DEFINITELY_MISSING_SECRET" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"
"#,
    )
    .expect("write config");

    let output = mag()
        .arg("--acp")
        .arg("--config")
        .arg(dir.config_path())
        .env_remove("MAG_BIN_TEST_DEFINITELY_MISSING_SECRET")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("run mag --acp");

    assert!(!output.status.success(), "assembly failure exits non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MAG_BIN_TEST_DEFINITELY_MISSING_SECRET"),
        "the error names the env reference: {stderr}"
    );
    assert!(
        stderr.contains("anthropic"),
        "the error names the provider: {stderr}"
    );
    assert!(!stderr.contains("sk-"), "no secret material: {stderr}");
}

#[test]
fn warn_level_diagnostics_reach_stderr_and_stay_off_stdout() {
    let dir = TempDir::new("tracing");
    fs::write(
        dir.config_path(),
        r#"
[providers.openai]
wire = "openai"
api_key = { env = "MAG_BIN_TRACING_API_KEY" }

[agents.default]
provider = "openai"
model = "gpt-5-codex"

[tools.no_such_tool]
approval = "ask"
"#,
    )
    .expect("write config with an unknown tool override");

    let envs = [("MAG_BIN_TRACING_API_KEY", "sk-bin-tracing")];
    let output = run_cli_with_env(&dir.config_path(), &[], "/quit\n", &envs);

    assert!(output.status.success(), "CLI exits 0: {output:?}");
    // mag-core warns that the tool override names no registered plugin; the
    // bin-installed tracing subscriber must surface it on stderr by default
    // (no MAG_LOG/RUST_LOG set) while stdout keeps only the CLI transcript.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("tool override names no registered tool plugin"),
        "warn diagnostic lands on stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("tool override names no registered tool plugin"),
        "stdout stays free of log output: {stdout}"
    );
}

#[test]
fn mag_log_env_overrides_the_default_level() {
    let dir = TempDir::new("tracing-env");
    fs::write(
        dir.config_path(),
        r#"
[tools.shell]
enabled = false
"#,
    )
    .expect("write config disabling a builtin tool");

    // `shell` disabled by config is an info-level diagnostic: invisible at the
    // default warn level, visible once MAG_LOG asks for info.
    let quiet = run_cli(&dir.config_path(), &[], "/quit\n");
    let quiet_stderr = String::from_utf8_lossy(&quiet.stderr);
    assert!(
        !quiet_stderr.contains("tool disabled by configuration"),
        "info diagnostic hidden at the default level: {quiet_stderr}"
    );

    let verbose = run_cli_with_env(&dir.config_path(), &[], "/quit\n", &[("MAG_LOG", "info")]);
    let verbose_stderr = String::from_utf8_lossy(&verbose.stderr);
    assert!(
        verbose_stderr.contains("tool disabled by configuration"),
        "MAG_LOG=info surfaces the info diagnostic: {verbose_stderr}"
    );
}

/// Sends one HTTP/1.1 GET over a std loopback connection and reads the full
/// response after the server closes it.
fn http_get(port: u16, path: &str, token: Option<&str>) -> String {
    let auth = token
        .map(|value| format!("Authorization: Bearer {value}\r\n"))
        .unwrap_or_default();
    let mut stream = std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .expect("connect to the web smoke server");
    stream
        .write_all(
            format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .expect("write smoke request");
    let mut response = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut response).expect("read smoke response");
    String::from_utf8_lossy(&response).into_owned()
}

/// Bin-level web smoke (`TODO.md` W5-1): the real `mag --web` binary is spawned
/// fully offline (assembly never calls the provider) and probed over loopback
/// HTTP for the auth gate and the SPA placeholder. The browser half of the
/// smoke is the manual checklist in `ui/README.md` (Manual Web Smoke).
#[test]
fn web_binary_serves_placeholder_and_enforces_bearer_auth() {
    let dir = TempDir::new("web");
    fs::write(
        dir.config_path(),
        r#"
[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "MAG_BIN_WEB_API_KEY" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"
"#,
    )
    .expect("write web smoke config");

    // Bind on port 0 and read the actual port back from the startup line:
    // pre-picking a free port races other parallel test processes that may
    // grab it before the child binds (TOCTOU), so the child picks instead.
    let mut child = mag()
        .arg("--web")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg("0")
        .arg("--token")
        .arg("web-smoke-token")
        .arg("--config")
        .arg(dir.config_path())
        .env("MAG_BIN_WEB_API_KEY", "sk-bin-web")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mag --web");

    let stderr = child.stderr.take().expect("child stderr is piped");
    let (lines_tx, lines_rx) = std::sync::mpsc::channel::<String>();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + CHILD_TIMEOUT;
    let port = loop {
        match lines_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if let Some(rest) = line.strip_prefix("mag web listening on http://127.0.0.1:") {
                    break rest
                        .trim_end_matches('/')
                        .parse::<u16>()
                        .expect("listening URL carries a port");
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                assert!(
                    Instant::now() < deadline,
                    "mag --web did not print its listening URL"
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("mag --web exited before printing its listening URL");
            }
        }
    };

    let unauthorized = http_get(port, "/api/sessions", None);
    assert!(
        unauthorized.starts_with("HTTP/1.1 401"),
        "API requires the bearer token: {unauthorized}"
    );
    let wrong_token = http_get(port, "/api/sessions", Some("wrong-token"));
    assert!(
        wrong_token.starts_with("HTTP/1.1 401"),
        "a wrong token is rejected: {wrong_token}"
    );
    let authorized = http_get(port, "/api/sessions", Some("web-smoke-token"));
    assert!(
        authorized.starts_with("HTTP/1.1 200"),
        "the provided token unlocks the API: {authorized}"
    );
    let index = http_get(port, "/", None);
    assert!(
        index.starts_with("HTTP/1.1 200"),
        "the SPA (or its build placeholder) is served without auth: {index}"
    );

    child.kill().expect("kill mag --web");
    let _ = child.wait();
    let _ = reader.join();
}
