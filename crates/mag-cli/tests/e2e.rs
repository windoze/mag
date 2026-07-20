//! Pipe-driven end-to-end tests for the minimal `mag-cli` REPL.
//!
//! The tests inject a scripted [`MagService`] and drive [`Cli::run_with_io`] with
//! in-memory stdin/stdout pipes. They cover the M6 CLI contracts without a
//! real terminal, network, credentials, or LLM.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use mag_cli::{Cli, CliError, CliOptions};
use mag_service::{
    ApprovalDecisionWire, ApprovalRequirementWire, ConfigDto, InteractionKindWire,
    InteractionOrigin, InteractionResponseWire, MagService, RequestId, RoutingMode, RunId,
    RunOutput, ServiceError, ServiceEvent, SessionConfig, SessionId, SessionInfo, SourceInfo,
    SourceKindWire, ToolCallIdWire, UsageInfo, UserInput,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

const SESSION_A: &str = "550e8400-e29b-41d4-a716-446655440000";
const SESSION_B: &str = "550e8400-e29b-41d4-a716-446655440001";
const RUN_A: &str = "00000000-0000-0000-0000-000000000001";
const RUN_B: &str = "00000000-0000-0000-0000-000000000002";
const REQ_APPROVAL: &str = "10000000-0000-0000-0000-000000000001";
const REQ_QUESTION: &str = "10000000-0000-0000-0000-000000000002";
const REQ_CHOICE: &str = "10000000-0000-0000-0000-000000000003";
const CALL_APPROVAL: &str = "20000000-0000-0000-0000-000000000001";

#[derive(Clone, Debug, PartialEq, Eq)]
struct SentMessage {
    session_id: SessionId,
    text: String,
}

#[derive(Clone, Debug, PartialEq)]
struct InteractionReply {
    session_id: SessionId,
    request_id: RequestId,
    response: InteractionResponseWire,
}

struct ScriptedService {
    events: broadcast::Sender<ServiceEvent>,
    sent: Arc<Mutex<Vec<SentMessage>>>,
    pivots: Arc<Mutex<Vec<SentMessage>>>,
    replies: Arc<Mutex<Vec<InteractionReply>>>,
    cancels: Arc<Mutex<Vec<SessionId>>>,
    resumes: Arc<Mutex<Vec<SessionId>>>,
    deletes: Arc<Mutex<Vec<SessionId>>>,
    list_sessions_calls: Arc<Mutex<usize>>,
    list_sources_calls: Arc<Mutex<usize>>,
    probe_calls: Arc<Mutex<usize>>,
    get_config_calls: Arc<Mutex<usize>>,
    reload_config_calls: Arc<Mutex<usize>>,
    apply_config_calls: Arc<Mutex<usize>>,
    create_count: Arc<Mutex<usize>>,
}

impl ScriptedService {
    fn new() -> Self {
        let (events, _) = broadcast::channel(32);
        Self {
            events,
            sent: Arc::new(Mutex::new(Vec::new())),
            pivots: Arc::new(Mutex::new(Vec::new())),
            replies: Arc::new(Mutex::new(Vec::new())),
            cancels: Arc::new(Mutex::new(Vec::new())),
            resumes: Arc::new(Mutex::new(Vec::new())),
            deletes: Arc::new(Mutex::new(Vec::new())),
            list_sessions_calls: Arc::new(Mutex::new(0)),
            list_sources_calls: Arc::new(Mutex::new(0)),
            probe_calls: Arc::new(Mutex::new(0)),
            get_config_calls: Arc::new(Mutex::new(0)),
            reload_config_calls: Arc::new(Mutex::new(0)),
            apply_config_calls: Arc::new(Mutex::new(0)),
            create_count: Arc::new(Mutex::new(0)),
        }
    }

    fn sent(&self) -> Arc<Mutex<Vec<SentMessage>>> {
        Arc::clone(&self.sent)
    }

    fn pivots(&self) -> Arc<Mutex<Vec<SentMessage>>> {
        Arc::clone(&self.pivots)
    }

    fn replies(&self) -> Arc<Mutex<Vec<InteractionReply>>> {
        Arc::clone(&self.replies)
    }

    fn cancels(&self) -> Arc<Mutex<Vec<SessionId>>> {
        Arc::clone(&self.cancels)
    }

    fn resumes(&self) -> Arc<Mutex<Vec<SessionId>>> {
        Arc::clone(&self.resumes)
    }

    fn deletes(&self) -> Arc<Mutex<Vec<SessionId>>> {
        Arc::clone(&self.deletes)
    }

    fn list_sessions_calls(&self) -> Arc<Mutex<usize>> {
        Arc::clone(&self.list_sessions_calls)
    }

    fn list_sources_calls(&self) -> Arc<Mutex<usize>> {
        Arc::clone(&self.list_sources_calls)
    }

    fn probe_calls(&self) -> Arc<Mutex<usize>> {
        Arc::clone(&self.probe_calls)
    }

    fn get_config_calls(&self) -> Arc<Mutex<usize>> {
        Arc::clone(&self.get_config_calls)
    }

    fn reload_config_calls(&self) -> Arc<Mutex<usize>> {
        Arc::clone(&self.reload_config_calls)
    }

    fn apply_config_calls(&self) -> Arc<Mutex<usize>> {
        Arc::clone(&self.apply_config_calls)
    }

    fn emit_interaction_script(&self, id: SessionId) {
        let call_id = ToolCallIdWire::parse_str(CALL_APPROVAL).expect("valid call id");
        let _ = self.events.send(ServiceEvent::InteractionRequested {
            id,
            request_id: RequestId::parse_str(REQ_APPROVAL).expect("valid request id"),
            kind: InteractionKindWire::Approval {
                call_id,
                requirement: ApprovalRequirementWire::RequireApproval {
                    reason: Some("run shell: git status".to_owned()),
                },
            },
            origin: InteractionOrigin {
                delegate: Some("researcher".to_owned()),
                depth: 1,
            },
        });
        let _ = self.events.send(ServiceEvent::InteractionRequested {
            id,
            request_id: RequestId::parse_str(REQ_QUESTION).expect("valid request id"),
            kind: InteractionKindWire::Question {
                prompt: "What should I say?".to_owned(),
            },
            origin: InteractionOrigin::default(),
        });
        let _ = self.events.send(ServiceEvent::InteractionRequested {
            id,
            request_id: RequestId::parse_str(REQ_CHOICE).expect("valid request id"),
            kind: InteractionKindWire::Choice {
                prompt: "Pick a path".to_owned(),
                options: vec!["left".to_owned(), "right".to_owned()],
            },
            origin: InteractionOrigin::default(),
        });
    }
}

fn scripted_config() -> ConfigDto {
    ConfigDto::parse_str(
        r#"
[providers.openai]
wire = "openai"

[agents.default]
provider = "openai"
model = "gpt-5-codex"

[tools.shell]
approval = "ask"
"#,
    )
    .expect("scripted config parses")
}

fn scripted_session_config(provider: &str, model: &str) -> SessionConfig {
    SessionConfig {
        provider: provider.to_owned(),
        model: model.to_owned(),
        tool_profile: None,
        cwd: None,
        routing: RoutingMode::default(),
        budget: None,
    }
}

#[async_trait]
impl MagService for ScriptedService {
    async fn create_session(&self, _config: SessionConfig) -> Result<SessionId, ServiceError> {
        let mut count = self.create_count.lock().expect("lock");
        *count += 1;
        let id = if *count == 1 { SESSION_A } else { SESSION_B };
        Ok(SessionId::parse_str(id).expect("valid session id"))
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError> {
        *self.list_sessions_calls.lock().expect("lock") += 1;
        Ok(vec![
            SessionInfo {
                id: SessionId::parse_str(SESSION_A).expect("valid session id"),
                config: scripted_session_config("openai", "gpt-5-codex"),
            },
            SessionInfo {
                id: SessionId::parse_str(SESSION_B).expect("valid session id"),
                config: scripted_session_config("anthropic", "claude-sonnet"),
            },
        ])
    }

    async fn resume_session(&self, id: SessionId) -> Result<(), ServiceError> {
        self.resumes.lock().expect("lock").push(id);
        Ok(())
    }

    async fn delete_session(&self, id: SessionId) -> Result<(), ServiceError> {
        self.deletes.lock().expect("lock").push(id);
        Ok(())
    }

    async fn send_message(&self, id: SessionId, input: UserInput) -> Result<RunId, ServiceError> {
        self.sent.lock().expect("lock").push(SentMessage {
            session_id: id,
            text: input.text.clone(),
        });
        let run_id = if input.text.contains("second") {
            RunId::parse_str(RUN_B).expect("valid run id")
        } else {
            RunId::parse_str(RUN_A).expect("valid run id")
        };
        let _ = self.events.send(ServiceEvent::RunStarted { id, run_id });
        if input.text == "hold" || input.text == "race-finish" {
            return Ok(run_id);
        }
        if input.text == "interact" {
            self.emit_interaction_script(id);
            return Ok(run_id);
        }
        let _ = self.events.send(ServiceEvent::TextDelta {
            id,
            text: format!("stream:{}", input.text),
        });
        let _ = self.events.send(ServiceEvent::TextDelta {
            id,
            text: ":done".to_owned(),
        });
        let _ = self.events.send(ServiceEvent::RunFinished {
            id,
            output: RunOutput {
                text: format!("stream:{}:done", input.text),
                usage: Some(UsageInfo {
                    input_tokens: 1,
                    output_tokens: 2,
                    total_tokens: 3,
                }),
            },
        });
        Ok(run_id)
    }

    async fn cancel(&self, id: SessionId) -> Result<(), ServiceError> {
        self.cancels.lock().expect("lock").push(id);
        let _ = self.events.send(ServiceEvent::RunError {
            id,
            message: "cancelled by user".to_owned(),
            kind: mag_service::RunErrorKind::Cancelled,
        });
        Ok(())
    }

    async fn pivot_message(&self, id: SessionId, input: UserInput) -> Result<(), ServiceError> {
        self.pivots.lock().expect("lock").push(SentMessage {
            session_id: id,
            text: input.text.clone(),
        });
        if input.text != "pivot now" {
            if input.text == "fallback after race" {
                let _ = self.events.send(ServiceEvent::RunFinished {
                    id,
                    output: RunOutput {
                        text: "race finished".to_owned(),
                        usage: None,
                    },
                });
            }
            return Err(ServiceError::NotPivotable {
                id,
                reason: "no in-progress run".to_owned(),
            });
        }

        let _ = self.events.send(ServiceEvent::PivotQueued { id });
        let _ = self.events.send(ServiceEvent::PivotApplied { id });
        let _ = self.events.send(ServiceEvent::RunFinished {
            id,
            output: RunOutput {
                text: "pivot landed".to_owned(),
                usage: None,
            },
        });
        Ok(())
    }

    async fn respond_interaction(
        &self,
        id: SessionId,
        request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        let reply_count = {
            let mut replies = self.replies.lock().expect("lock");
            replies.push(InteractionReply {
                session_id: id,
                request_id,
                response,
            });
            replies.len()
        };
        if reply_count == 3 {
            let _ = self.events.send(ServiceEvent::RunFinished {
                id,
                output: RunOutput {
                    text: "interactions done".to_owned(),
                    usage: None,
                },
            });
        }
        Ok(())
    }

    fn subscribe(&self, _id: Option<SessionId>) -> BoxStream<'static, ServiceEvent> {
        let receiver = self.events.subscribe();
        stream::unfold(receiver, |mut receiver| async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => return Some((event, receiver)),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
        .boxed()
    }

    async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        *self.list_sources_calls.lock().expect("lock") += 1;
        Ok(vec![SourceInfo {
            id: "openai".to_owned(),
            name: "OpenAI".to_owned(),
            kind: SourceKindWire::LlmProvider,
            available: true,
            version: Some("v1".to_owned()),
            path: None,
            capabilities: vec!["chat".to_owned()],
        }])
    }

    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        *self.probe_calls.lock().expect("lock") += 1;
        Ok(vec![SourceInfo {
            id: "local-coder".to_owned(),
            name: "Local Coder".to_owned(),
            kind: SourceKindWire::LocalAgent,
            available: false,
            version: None,
            path: Some("/missing/local-coder".to_owned()),
            capabilities: vec!["acp".to_owned()],
        }])
    }

    async fn get_config(&self) -> Result<ConfigDto, ServiceError> {
        *self.get_config_calls.lock().expect("lock") += 1;
        Ok(scripted_config())
    }

    async fn update_config(&self, _config: ConfigDto) -> Result<(), ServiceError> {
        Err(ServiceError::Unsupported {
            operation: "update_config".to_owned(),
        })
    }

    async fn reload_config(&self) -> Result<(), ServiceError> {
        *self.reload_config_calls.lock().expect("lock") += 1;
        let _ = self
            .events
            .send(ServiceEvent::ConfigChanged { revision: 42 });
        Ok(())
    }

    async fn apply_config(&self) -> Result<(), ServiceError> {
        *self.apply_config_calls.lock().expect("lock") += 1;
        Ok(())
    }
}

fn options() -> CliOptions {
    CliOptions {
        session: SessionConfig {
            provider: "openai".to_owned(),
            model: "gpt-5-codex".to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::default(),
            budget: None,
        },
        prompt: "mag> ".to_owned(),
    }
}

async fn drive(input: &str, service: Arc<ScriptedService>) -> String {
    let (mut stdin_writer, stdin_reader) = tokio::io::duplex(1024);
    let (stdout_writer, mut stdout_reader) = tokio::io::duplex(4096);
    let cli_service: Arc<dyn MagService> = service;

    let run = tokio::spawn(async move {
        Cli::run_with_io(cli_service, options(), stdin_reader, stdout_writer).await
    });

    stdin_writer
        .write_all(input.as_bytes())
        .await
        .expect("write scripted stdin");
    stdin_writer.shutdown().await.expect("close scripted stdin");

    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run must not hang")
        .expect("CLI task must join");
    result.expect("CLI run must succeed");

    let mut output = String::new();
    stdout_reader
        .read_to_string(&mut output)
        .await
        .expect("read scripted stdout");
    output
}

fn spawn_cli(
    service: Arc<ScriptedService>,
) -> (
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    JoinHandle<Result<(), CliError>>,
) {
    let (stdin_writer, stdin_reader) = tokio::io::duplex(1024);
    let (stdout_writer, stdout_reader) = tokio::io::duplex(8192);
    let cli_service: Arc<dyn MagService> = service;
    let run = tokio::spawn(async move {
        Cli::run_with_io(cli_service, options(), stdin_reader, stdout_writer).await
    });
    (stdin_writer, stdout_reader, run)
}

async fn read_until<R>(reader: &mut R, output: &mut String, needle: &str)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 256];
    while !output.contains(needle) {
        let read = tokio::time::timeout(Duration::from_secs(2), reader.read(&mut buffer))
            .await
            .expect("CLI output timed out")
            .expect("read CLI output");
        assert!(
            read > 0,
            "CLI output ended before `{needle}`; got {output:?}"
        );
        output.push_str(std::str::from_utf8(&buffer[..read]).expect("utf-8 CLI output"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repl_sends_two_messages_and_renders_streaming_output() {
    let service = Arc::new(ScriptedService::new());
    let sent = service.sent();

    let output = drive("first\nsecond\n/quit\n", service).await;

    assert!(output.contains("stream:first:done"), "{output}");
    assert!(output.contains("stream:second:done"), "{output}");
    assert!(
        output.contains("[finished usage input=1 output=2 total=3]"),
        "{output}"
    );

    let sent = sent.lock().expect("lock").clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].text, "first");
    assert_eq!(sent[1].text, "second");
    assert_eq!(sent[0].session_id, SessionId::parse_str(SESSION_A).unwrap());
    assert_eq!(sent[1].session_id, SessionId::parse_str(SESSION_A).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slash_new_creates_and_switches_to_a_new_session() {
    let service = Arc::new(ScriptedService::new());
    let sent = service.sent();

    let output = drive("before\n/new\nsecond after-new\n/quit\n", service).await;

    assert!(output.contains(SESSION_A), "{output}");
    assert!(output.contains(SESSION_B), "{output}");
    assert!(output.contains("stream:before:done"), "{output}");
    assert!(output.contains("stream:second after-new:done"), "{output}");

    let sent = sent.lock().expect("lock").clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].session_id, SessionId::parse_str(SESSION_A).unwrap());
    assert_eq!(sent[1].session_id, SessionId::parse_str(SESSION_B).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_coordinator_answers_queued_interactions_in_order() {
    let service = Arc::new(ScriptedService::new());
    let replies = service.replies();
    let (mut stdin_writer, mut stdout_reader, run) = spawn_cli(service);
    let mut output = String::new();

    stdin_writer
        .write_all(b"interact\n")
        .await
        .expect("send prompt-triggering input");

    read_until(
        &mut stdout_reader,
        &mut output,
        "[from researcher@depth1] [approval]",
    )
    .await;
    assert!(output.contains("tool_call="), "{output}");
    assert!(output.contains("reason: run shell: git status"), "{output}");
    stdin_writer
        .write_all(b"y\n")
        .await
        .expect("answer approval");

    read_until(
        &mut stdout_reader,
        &mut output,
        "[question] What should I say?",
    )
    .await;
    stdin_writer
        .write_all(b"hello from the user\n")
        .await
        .expect("answer question");

    read_until(&mut stdout_reader, &mut output, "[choice] Pick a path").await;
    assert!(output.contains("1. left"), "{output}");
    assert!(output.contains("2. right"), "{output}");
    stdin_writer.write_all(b"2\n").await.expect("answer choice");

    read_until(&mut stdout_reader, &mut output, "interactions done").await;
    read_until(&mut stdout_reader, &mut output, "[finished]").await;
    stdin_writer.write_all(b"/quit\n").await.expect("quit CLI");
    stdin_writer.shutdown().await.expect("close scripted stdin");

    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run must not hang")
        .expect("CLI task must join");
    result.expect("CLI run must succeed");

    let replies = replies.lock().expect("lock").clone();
    assert_eq!(replies.len(), 3);
    assert_eq!(
        replies[0].session_id,
        SessionId::parse_str(SESSION_A).unwrap()
    );
    assert_eq!(
        replies[0].request_id,
        RequestId::parse_str(REQ_APPROVAL).unwrap()
    );
    match &replies[0].response {
        InteractionResponseWire::Approval { decision, .. } => {
            assert_eq!(*decision, ApprovalDecisionWire::Approve);
        }
        other => panic!("expected approval response, got {other:?}"),
    }
    assert_eq!(
        replies[1].request_id,
        RequestId::parse_str(REQ_QUESTION).unwrap()
    );
    assert_eq!(
        replies[1].response,
        InteractionResponseWire::Answer {
            text: "hello from the user".to_owned()
        }
    );
    assert_eq!(
        replies[2].request_id,
        RequestId::parse_str(REQ_CHOICE).unwrap()
    );
    assert_eq!(
        replies[2].response,
        InteractionResponseWire::Choice { index: 1 }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_during_an_in_flight_run_uses_pivot_without_falling_back() {
    let service = Arc::new(ScriptedService::new());
    let sent = service.sent();
    let pivots = service.pivots();

    let output = drive("hold\npivot now\n/quit\n", service).await;

    assert!(output.contains("[pivot queued"), "{output}");
    assert!(output.contains("[pivot applied"), "{output}");
    assert!(output.contains("pivot landed"), "{output}");

    let sent = sent.lock().expect("lock").clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].text, "hold");

    let pivots = pivots.lock().expect("lock").clone();
    assert_eq!(pivots.len(), 1);
    assert_eq!(pivots[0].text, "pivot now");
    assert_eq!(
        pivots[0].session_id,
        SessionId::parse_str(SESSION_A).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_pivotable_falls_back_to_send_message() {
    let service = Arc::new(ScriptedService::new());
    let sent = service.sent();
    let pivots = service.pivots();

    let output = drive("race-finish\nfallback after race\n/quit\n", service).await;

    assert!(output.contains("race finished"), "{output}");
    assert!(
        output.contains("stream:fallback after race:done"),
        "{output}"
    );

    let pivots = pivots.lock().expect("lock").clone();
    assert_eq!(pivots.len(), 1);
    assert_eq!(pivots[0].text, "fallback after race");

    let sent = sent.lock().expect("lock").clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].text, "race-finish");
    assert_eq!(sent[1].text, "fallback after race");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_during_an_in_flight_run_cancels_the_current_session() {
    let service = Arc::new(ScriptedService::new());
    let cancels = service.cancels();

    let output = drive("hold\n\u{3}\n/quit\n", service).await;

    assert!(output.contains("[cancel requested"), "{output}");
    assert!(
        output.contains("[error cancelled] cancelled by user"),
        "{output}"
    );

    let cancels = cancels.lock().expect("lock").clone();
    assert_eq!(cancels, vec![SessionId::parse_str(SESSION_A).unwrap()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slash_commands_call_the_matching_service_methods() {
    let service = Arc::new(ScriptedService::new());
    let resumes = service.resumes();
    let deletes = service.deletes();
    let cancels = service.cancels();
    let list_sessions_calls = service.list_sessions_calls();
    let list_sources_calls = service.list_sources_calls();
    let probe_calls = service.probe_calls();

    let output = drive(
        &format!(
            "/sessions\n/resume {SESSION_B}\n/delete {SESSION_A}\n/cancel\n/sources\n/help\n/quit\n"
        ),
        service,
    )
    .await;

    assert!(
        output.contains(&format!("* {SESSION_A} provider=openai model=gpt-5-codex")),
        "{output}"
    );
    assert!(
        output.contains(&format!(
            "  {SESSION_B} provider=anthropic model=claude-sonnet"
        )),
        "{output}"
    );
    assert!(
        output.contains(&format!("[session {SESSION_B} resumed]")),
        "{output}"
    );
    assert!(
        output.contains(&format!("[session {SESSION_A} deleted]")),
        "{output}"
    );
    assert!(output.contains("[sources]"), "{output}");
    assert!(
        output.contains("- openai name=OpenAI kind=llm_provider available=true"),
        "{output}"
    );
    assert!(output.contains("[probed sources]"), "{output}");
    assert!(
        output.contains("- local-coder name=Local Coder kind=local_agent available=false"),
        "{output}"
    );
    assert!(output.contains("commands: /new, /sessions"), "{output}");

    assert_eq!(*list_sessions_calls.lock().expect("lock"), 1);
    assert_eq!(*list_sources_calls.lock().expect("lock"), 1);
    assert_eq!(*probe_calls.lock().expect("lock"), 1);
    assert_eq!(
        resumes.lock().expect("lock").clone(),
        vec![SessionId::parse_str(SESSION_B).unwrap()]
    );
    assert_eq!(
        deletes.lock().expect("lock").clone(),
        vec![SessionId::parse_str(SESSION_A).unwrap()]
    );
    assert_eq!(
        cancels.lock().expect("lock").clone(),
        vec![SessionId::parse_str(SESSION_B).unwrap()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_commands_call_service_methods_and_render_changes() {
    let service = Arc::new(ScriptedService::new());
    let get_config_calls = service.get_config_calls();
    let reload_config_calls = service.reload_config_calls();
    let apply_config_calls = service.apply_config_calls();
    let (mut stdin_writer, mut stdout_reader, run) = spawn_cli(service);
    let mut output = String::new();

    read_until(&mut stdout_reader, &mut output, "[session").await;
    stdin_writer
        .write_all(b"/config show\n")
        .await
        .expect("request config show");
    read_until(&mut stdout_reader, &mut output, "model = \"gpt-5-codex\"").await;
    assert!(output.contains("[providers.openai]"), "{output}");
    assert!(output.contains("wire = \"openai\""), "{output}");
    assert!(output.contains("[tools.shell]"), "{output}");
    assert!(output.contains("approval = \"ask\""), "{output}");

    stdin_writer
        .write_all(b"/config reload\n")
        .await
        .expect("request config reload");
    read_until(&mut stdout_reader, &mut output, "[config reloaded]").await;
    read_until(
        &mut stdout_reader,
        &mut output,
        "[config changed revision=42]",
    )
    .await;

    stdin_writer
        .write_all(b"/config apply\n")
        .await
        .expect("request config apply");
    read_until(&mut stdout_reader, &mut output, "next turn boundary").await;

    stdin_writer.write_all(b"/quit\n").await.expect("quit CLI");
    stdin_writer.shutdown().await.expect("close scripted stdin");

    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run must not hang")
        .expect("CLI task must join");
    result.expect("CLI run must succeed");

    assert_eq!(*get_config_calls.lock().expect("lock"), 1);
    assert_eq!(*reload_config_calls.lock().expect("lock"), 1);
    assert_eq!(*apply_config_calls.lock().expect("lock"), 1);
}
