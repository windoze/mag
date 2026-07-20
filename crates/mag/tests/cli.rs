//! Bin-level smoke tests for the `mag` binary (`TODO.md` M3-6).
//!
//! All offline: the binary is driven with `--config` pointing at a tempdir
//! sample, the ACP handshake is answered over piped stdio with a fake API key
//! injected through the child environment, and no network is touched
//! (assembly builds the adapter but never calls it).

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
    assert!(stdout.contains("--acp"), "help documents --acp: {stdout}");
    assert!(
        stdout.contains("--config"),
        "help documents --config: {stdout}"
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
fn invocation_without_acp_prints_usage_and_does_not_load_config() {
    // `--config` pointing at a tempdir sample parses fine but is only loaded
    // on the `--acp` startup path; the usage path must not touch it.
    let dir = TempDir::new("usage");
    fs::write(dir.config_path(), "not toml = [").expect("write junk config");
    let output = mag()
        .arg("--config")
        .arg(dir.config_path())
        .output()
        .expect("run mag without --acp");

    assert_eq!(output.status.code(), Some(2), "usage error exits 2");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage: mag"), "got: {stderr}");
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
