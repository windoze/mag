//! Pipe-driven end-to-end tests for the minimal `mag-cli` REPL.
//!
//! The tests inject a scripted [`MagService`] and drive [`Cli::run_with_io`] with
//! in-memory stdin/stdout pipes. They cover the M6 CLI contracts without a
//! real terminal, network, credentials, or LLM.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use mag_cli::{Cli, CliError, CliOptions};
use mag_service::{
    AgentIdWire, ApprovalDecisionWire, ApprovalRequirementWire, ConfigDto, DelegationMessageWire,
    DelegationStatusWire, DelegationTrace, HistoryEntry, InteractionKindWire, InteractionOrigin,
    InteractionResponseWire, MagService, PermissionCategoryWire, PermissionDecisionWire,
    PermissionRiskWire, RequestId, RunId, RunOutput, ServiceError, ServiceEvent,
    SessionId, SessionInfo, SourceInfo, SourceKindWire, ToolCallIdWire, UsageInfo,
    UserInput,
};
use serde_json::json;
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
const REQ_PERMISSION: &str = "10000000-0000-0000-0000-000000000004";
const REQ_BACKGROUND: &str = "10000000-0000-0000-0000-000000000005";
const REQ_QUIT_LATE: &str = "10000000-0000-0000-0000-000000000006";
const CALL_APPROVAL: &str = "20000000-0000-0000-0000-000000000001";
const AGENT_PERMISSION: &str = "30000000-0000-0000-0000-000000000001";

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
    defer_cancel_terminal: Arc<AtomicBool>,
    resumes: Arc<Mutex<Vec<SessionId>>>,
    deletes: Arc<Mutex<Vec<SessionId>>>,
    list_sessions_calls: Arc<Mutex<usize>>,
    list_sources_calls: Arc<Mutex<usize>>,
    probe_calls: Arc<Mutex<usize>>,
    get_config_calls: Arc<Mutex<usize>>,
    reload_config_calls: Arc<Mutex<usize>>,
    apply_config_calls: Arc<Mutex<usize>>,
    create_count: Arc<Mutex<usize>>,
    create_agents: Arc<Mutex<Vec<Option<String>>>>,
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
            defer_cancel_terminal: Arc::new(AtomicBool::new(false)),
            resumes: Arc::new(Mutex::new(Vec::new())),
            deletes: Arc::new(Mutex::new(Vec::new())),
            list_sessions_calls: Arc::new(Mutex::new(0)),
            list_sources_calls: Arc::new(Mutex::new(0)),
            probe_calls: Arc::new(Mutex::new(0)),
            get_config_calls: Arc::new(Mutex::new(0)),
            reload_config_calls: Arc::new(Mutex::new(0)),
            apply_config_calls: Arc::new(Mutex::new(0)),
            create_count: Arc::new(Mutex::new(0)),
            create_agents: Arc::new(Mutex::new(Vec::new())),
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

    fn defer_cancel_terminal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.defer_cancel_terminal)
    }

    /// Emits the cancelled terminal a `cancel` call would normally produce;
    /// used together with `defer_cancel_terminal` to script the exact moment a
    /// cancelled run reaches its terminal state.
    fn emit_cancelled_terminal(&self, id: SessionId) {
        let _ = self.events.send(ServiceEvent::RunError {
            id,
            message: "cancelled by user".to_owned(),
            kind: mag_service::RunErrorKind::Cancelled,
        });
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

    fn create_agents(&self) -> Arc<Mutex<Vec<Option<String>>>> {
        Arc::clone(&self.create_agents)
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
        let _ = self.events.send(ServiceEvent::InteractionRequested {
            id,
            request_id: RequestId::parse_str(REQ_PERMISSION).expect("valid request id"),
            kind: InteractionKindWire::Permission {
                action_id: "perm-shell-1".to_owned(),
                actor: AgentIdWire::parse_str(AGENT_PERMISSION).expect("valid agent id"),
                category: PermissionCategoryWire::Shell,
                risk: PermissionRiskWire::High,
                summary: "run privileged command".to_owned(),
                subject: json!({ "command": "rm -rf target/tmp" }),
                reason: Some("cleanup requested by model".to_owned()),
            },
            origin: InteractionOrigin {
                delegate: Some("ops".to_owned()),
                depth: 2,
            },
        });
    }

    fn emit_delegation_script(&self, id: SessionId) {
        let run_id = Some(RunId::parse_str(RUN_A).expect("valid run id"));
        let _ = self.events.send(ServiceEvent::DelegationStarted {
            id,
            trace: DelegationTrace {
                run_id,
                delegate: "researcher".to_owned(),
                status: DelegationStatusWire::Started,
                task: Some("summarize docs".to_owned()),
                output: None,
                message: None,
                usage: None,
            },
        });
        let _ = self.events.send(ServiceEvent::DelegationMessage {
            id,
            message: DelegationMessageWire {
                run_id,
                delegate: "researcher".to_owned(),
                text: "working on summary".to_owned(),
            },
        });
        let _ = self.events.send(ServiceEvent::DelegationFinished {
            id,
            trace: DelegationTrace {
                run_id,
                delegate: "researcher".to_owned(),
                status: DelegationStatusWire::Finished,
                task: Some("summarize docs".to_owned()),
                output: Some("summary ready".to_owned()),
                message: None,
                usage: None,
            },
        });
        let _ = self.events.send(ServiceEvent::DelegationFailed {
            id,
            trace: DelegationTrace {
                run_id,
                delegate: "peer".to_owned(),
                status: DelegationStatusWire::Failed,
                task: Some("check external".to_owned()),
                output: None,
                message: Some("external process exited".to_owned()),
                usage: None,
            },
        });
    }

    fn emit_background_question(&self, id: SessionId) {
        let _ = self.events.send(ServiceEvent::InteractionRequested {
            id,
            request_id: RequestId::parse_str(REQ_BACKGROUND).expect("valid request id"),
            kind: InteractionKindWire::Question {
                prompt: "Background question?".to_owned(),
            },
            origin: InteractionOrigin::default(),
        });
    }

    fn emit_late_approval(&self, id: SessionId) {
        let call_id = ToolCallIdWire::parse_str(CALL_APPROVAL).expect("valid call id");
        let _ = self.events.send(ServiceEvent::InteractionRequested {
            id,
            request_id: RequestId::parse_str(REQ_QUIT_LATE).expect("valid request id"),
            kind: InteractionKindWire::Approval {
                call_id,
                requirement: ApprovalRequirementWire::RequireApproval {
                    reason: Some("late interaction during quit".to_owned()),
                },
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

#[async_trait]
impl MagService for ScriptedService {
    async fn create_session(
        &self,
        _cwd: Option<PathBuf>,
        agent: Option<String>,
    ) -> Result<SessionId, ServiceError> {
        self.create_agents.lock().expect("lock").push(agent);
        let mut count = self.create_count.lock().expect("lock");
        *count += 1;
        let id = if *count == 1 { SESSION_A } else { SESSION_B };
        Ok(SessionId::parse_str(id).expect("valid session id"))
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError> {
        *self.list_sessions_calls.lock().expect("lock") += 1;
        Ok(vec![
            SessionInfo::new(
                SessionId::parse_str(SESSION_A).expect("valid session id"),
                "openai".to_owned(),
                None,
            ),
            SessionInfo::new(
                SessionId::parse_str(SESSION_B).expect("valid session id"),
                "anthropic".to_owned(),
                None,
            ),
        ])
    }

    async fn resume_session(&self, id: SessionId) -> Result<(), ServiceError> {
        self.resumes.lock().expect("lock").push(id);
        Ok(())
    }

    async fn get_session_history(&self, _id: SessionId) -> Result<Vec<HistoryEntry>, ServiceError> {
        Ok(Vec::new())
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
        if input.text == "delegate events" {
            self.emit_delegation_script(id);
            let _ = self.events.send(ServiceEvent::RunFinished {
                id,
                output: RunOutput {
                    text: "delegation script done".to_owned(),
                    usage: None,
                },
            });
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
        if !self.defer_cancel_terminal.load(Ordering::SeqCst) {
            self.emit_cancelled_terminal(id);
        }
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
        if reply_count == 4 {
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
        agent: Some("openai".to_owned()),
        cwd: None,
        resume: None,
        prompt: "mag> ".to_owned(),
    }
}

async fn drive(input: &str, service: Arc<ScriptedService>) -> String {
    drive_dyn(input, service).await
}

async fn drive_dyn(input: &str, service: Arc<dyn MagService>) -> String {
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
    spawn_cli_dyn(service)
}

fn spawn_cli_dyn(
    service: Arc<dyn MagService>,
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
async fn slash_new_with_agent_passes_the_agent_binding() {
    let service = Arc::new(ScriptedService::new());
    let create_agents = service.create_agents();

    let output = drive("/new reviewer\n/quit\n", service).await;

    assert!(output.contains(SESSION_A), "{output}");
    assert!(output.contains(SESSION_B), "{output}");
    let create_agents = create_agents.lock().expect("lock").clone();
    assert_eq!(create_agents.len(), 2);
    assert_eq!(create_agents[0].as_deref(), Some("openai"));
    assert_eq!(create_agents[1].as_deref(), Some("reviewer"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_session_interaction_is_answered_after_resume() {
    let service = Arc::new(ScriptedService::new());
    let replies = service.replies();
    let (mut stdin_writer, mut stdout_reader, run) = spawn_cli(service.clone());
    let mut output = String::new();
    let session_a = SessionId::parse_str(SESSION_A).unwrap();

    read_until(&mut stdout_reader, &mut output, SESSION_A).await;
    stdin_writer
        .write_all(b"/new\n")
        .await
        .expect("create second session");
    read_until(&mut stdout_reader, &mut output, SESSION_B).await;

    service.emit_background_question(session_a);
    read_until(
        &mut stdout_reader,
        &mut output,
        &format!("[interaction pending for session {SESSION_A}"),
    )
    .await;

    stdin_writer
        .write_all(format!("/resume {SESSION_A}\n").as_bytes())
        .await
        .expect("resume session with pending interaction");
    read_until(
        &mut stdout_reader,
        &mut output,
        &format!("[session {SESSION_A} resumed]"),
    )
    .await;
    read_until(
        &mut stdout_reader,
        &mut output,
        "[question] Background question?",
    )
    .await;
    stdin_writer
        .write_all(b"background answer\n")
        .await
        .expect("answer background interaction");
    stdin_writer.write_all(b"/quit\n").await.expect("quit CLI");
    stdin_writer.shutdown().await.expect("close scripted stdin");

    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run must not hang")
        .expect("CLI task must join");
    result.expect("CLI run must succeed");

    let replies = replies.lock().expect("lock").clone();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].session_id, session_a);
    assert_eq!(
        replies[0].request_id,
        RequestId::parse_str(REQ_BACKGROUND).unwrap()
    );
    assert_eq!(
        replies[0].response,
        InteractionResponseWire::Answer {
            text: "background answer".to_owned()
        }
    );
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

    read_until(
        &mut stdout_reader,
        &mut output,
        "[from ops@depth2] [permission] run privileged command",
    )
    .await;
    assert!(output.contains("category=shell risk=high"), "{output}");
    assert!(output.contains("action=perm-shell-1"), "{output}");
    assert!(output.contains("cleanup requested by model"), "{output}");
    stdin_writer
        .write_all(b"allow\n")
        .await
        .expect("answer permission");

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
    assert_eq!(
        output
            .matches("[from researcher@depth1] [approval]")
            .count(),
        1,
        "the active approval prompt is printed once before it is answered: {output}"
    );
    assert_eq!(replies.len(), 4);
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
    assert_eq!(
        replies[3].request_id,
        RequestId::parse_str(REQ_PERMISSION).unwrap()
    );
    assert_eq!(
        replies[3].response,
        InteractionResponseWire::Permission {
            action_id: "perm-shell-1".to_owned(),
            decision: PermissionDecisionWire::Approve,
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_on_question_cancels_the_run_without_answering() {
    let service = Arc::new(ScriptedService::new());
    let replies = service.replies();
    let cancels = service.cancels();
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
    stdin_writer.write_all(b"y\n").await.expect("approve");
    read_until(
        &mut stdout_reader,
        &mut output,
        "[question] What should I say?",
    )
    .await;
    stdin_writer
        .write_all("\u{3}\n".as_bytes())
        .await
        .expect("interrupt question");
    read_until(&mut stdout_reader, &mut output, "[cancel requested").await;
    read_until(&mut stdout_reader, &mut output, "[error cancelled]").await;
    stdin_writer.write_all(b"/quit\n").await.expect("quit CLI");
    stdin_writer.shutdown().await.expect("close scripted stdin");

    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run must not hang")
        .expect("CLI task must join");
    result.expect("CLI run must succeed");

    let replies = replies.lock().expect("lock").clone();
    assert!(replies.iter().any(|reply| {
        reply.request_id == RequestId::parse_str(REQ_APPROVAL).unwrap()
            && matches!(
                &reply.response,
                InteractionResponseWire::Approval {
                    decision,
                    ..
                } if *decision == ApprovalDecisionWire::Approve
            )
    }));
    assert!(
        !replies
            .iter()
            .any(|reply| reply.request_id == RequestId::parse_str(REQ_QUESTION).unwrap()),
        "question cancellation must not send an empty answer: {replies:?}"
    );
    let cancels = cancels.lock().expect("lock").clone();
    assert_eq!(cancels, vec![SessionId::parse_str(SESSION_A).unwrap()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_on_choice_cancels_the_run_without_selecting_default() {
    let service = Arc::new(ScriptedService::new());
    let replies = service.replies();
    let cancels = service.cancels();
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
    stdin_writer.write_all(b"y\n").await.expect("approve");
    read_until(
        &mut stdout_reader,
        &mut output,
        "[question] What should I say?",
    )
    .await;
    stdin_writer
        .write_all(b"hello before choice\n")
        .await
        .expect("answer question");
    read_until(&mut stdout_reader, &mut output, "[choice] Pick a path").await;
    stdin_writer
        .write_all("\u{3}\n".as_bytes())
        .await
        .expect("interrupt choice");
    read_until(&mut stdout_reader, &mut output, "[cancel requested").await;
    read_until(&mut stdout_reader, &mut output, "[error cancelled]").await;
    stdin_writer.write_all(b"/quit\n").await.expect("quit CLI");
    stdin_writer.shutdown().await.expect("close scripted stdin");

    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run must not hang")
        .expect("CLI task must join");
    result.expect("CLI run must succeed");

    let replies = replies.lock().expect("lock").clone();
    assert!(replies.iter().any(|reply| {
        reply.request_id == RequestId::parse_str(REQ_QUESTION).unwrap()
            && reply.response
                == InteractionResponseWire::Answer {
                    text: "hello before choice".to_owned(),
                }
    }));
    assert!(
        !replies
            .iter()
            .any(|reply| reply.request_id == RequestId::parse_str(REQ_CHOICE).unwrap()),
        "choice cancellation must not send a default choice: {replies:?}"
    );
    let cancels = cancels.lock().expect("lock").clone();
    assert_eq!(cancels, vec![SessionId::parse_str(SESSION_A).unwrap()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegation_events_are_rendered() {
    let service = Arc::new(ScriptedService::new());

    let output = drive("delegate events\n/quit\n", service).await;

    assert!(
        output.contains("[delegation started"),
        "delegation start is visible: {output}"
    );
    assert!(
        output.contains("delegate=researcher task=summarize docs"),
        "delegation trace carries delegate and task: {output}"
    );
    assert!(
        output.contains("[delegation message"),
        "delegation message is visible: {output}"
    );
    assert!(
        output.contains("delegate=researcher text=working on summary"),
        "delegation message carries text: {output}"
    );
    assert!(
        output.contains("[delegation finished"),
        "delegation finish is visible: {output}"
    );
    assert!(
        output.contains("output=summary ready"),
        "delegation finish carries output: {output}"
    );
    assert!(
        output.contains("[delegation failed"),
        "delegation failure is visible: {output}"
    );
    assert!(
        output.contains("delegate=peer task=check external message=external process exited"),
        "delegation failure carries message: {output}"
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
async fn rendering_tracks_streamed_text_per_session() {
    let service = Arc::new(ScriptedService::new());
    let (mut stdin_writer, mut stdout_reader, run) = spawn_cli(service.clone());
    let mut output = String::new();
    let session_a = SessionId::parse_str(SESSION_A).unwrap();
    let session_b = SessionId::parse_str(SESSION_B).unwrap();
    let run_a = RunId::parse_str(RUN_A).unwrap();
    let run_b = RunId::parse_str(RUN_B).unwrap();

    read_until(&mut stdout_reader, &mut output, SESSION_A).await;
    let _ = service.events.send(ServiceEvent::RunStarted {
        id: session_a,
        run_id: run_a,
    });
    let _ = service.events.send(ServiceEvent::RunStarted {
        id: session_b,
        run_id: run_b,
    });
    let _ = service.events.send(ServiceEvent::TextDelta {
        id: session_b,
        text: "B streamed".to_owned(),
    });
    let _ = service.events.send(ServiceEvent::RunFinished {
        id: session_a,
        output: RunOutput {
            text: "A final only".to_owned(),
            usage: None,
        },
    });
    let _ = service.events.send(ServiceEvent::RunFinished {
        id: session_b,
        output: RunOutput {
            text: "B streamed".to_owned(),
            usage: None,
        },
    });

    read_until(&mut stdout_reader, &mut output, "A final only").await;
    read_until(&mut stdout_reader, &mut output, "[finished]").await;
    stdin_writer.write_all(b"/quit\n").await.expect("quit CLI");
    stdin_writer.shutdown().await.expect("close scripted stdin");

    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run must not hang")
        .expect("CLI task must join");
    result.expect("CLI run must succeed");

    assert_eq!(output.matches("A final only").count(), 1, "{output}");
    assert_eq!(output.matches("B streamed").count(), 1, "{output}");
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
        output.contains(&format!("* {SESSION_A} agent=openai")),
        "{output}"
    );
    assert!(
        output.contains(&format!("  {SESSION_B} agent=anthropic")),
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
    assert!(
        output.contains("commands: /new [agent], /sessions"),
        "{output}"
    );

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
/// Scripted service whose event stream ends immediately, simulating a service
/// that closes the subscription while the CLI is still running (B6 regression).
struct EndingStreamService {
    inner: ScriptedService,
}

impl EndingStreamService {
    fn new() -> Self {
        Self {
            inner: ScriptedService::new(),
        }
    }
}

#[async_trait]
impl MagService for EndingStreamService {
    async fn create_session(
        &self,
        cwd: Option<PathBuf>,
        agent: Option<String>,
    ) -> Result<SessionId, ServiceError> {
        self.inner.create_session(cwd, agent).await
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, ServiceError> {
        self.inner.list_sessions().await
    }

    async fn resume_session(&self, id: SessionId) -> Result<(), ServiceError> {
        self.inner.resume_session(id).await
    }

    async fn get_session_history(&self, id: SessionId) -> Result<Vec<HistoryEntry>, ServiceError> {
        self.inner.get_session_history(id).await
    }

    async fn delete_session(&self, id: SessionId) -> Result<(), ServiceError> {
        self.inner.delete_session(id).await
    }

    async fn send_message(&self, id: SessionId, input: UserInput) -> Result<RunId, ServiceError> {
        self.inner.send_message(id, input).await
    }

    async fn cancel(&self, id: SessionId) -> Result<(), ServiceError> {
        self.inner.cancel(id).await
    }

    async fn pivot_message(&self, id: SessionId, input: UserInput) -> Result<(), ServiceError> {
        self.inner.pivot_message(id, input).await
    }

    async fn respond_interaction(
        &self,
        id: SessionId,
        request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        self.inner
            .respond_interaction(id, request_id, response)
            .await
    }

    fn subscribe(&self, _id: Option<SessionId>) -> BoxStream<'static, ServiceEvent> {
        stream::empty().boxed()
    }

    async fn list_sources(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        self.inner.list_sources().await
    }

    async fn probe_local_agents(&self) -> Result<Vec<SourceInfo>, ServiceError> {
        self.inner.probe_local_agents().await
    }

    async fn get_config(&self) -> Result<ConfigDto, ServiceError> {
        self.inner.get_config().await
    }

    async fn update_config(&self, config: ConfigDto) -> Result<(), ServiceError> {
        self.inner.update_config(config).await
    }

    async fn reload_config(&self) -> Result<(), ServiceError> {
        self.inner.reload_config().await
    }

    async fn apply_config(&self) -> Result<(), ServiceError> {
        self.inner.apply_config().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quit_with_in_flight_run_cancels_and_auto_answers_late_interaction() {
    let service = Arc::new(ScriptedService::new());
    let replies = service.replies();
    let cancels = service.cancels();
    // Hold back the cancelled run's terminal so the wind-down window stays
    // open: the late interaction then deterministically arrives while the CLI
    // is quitting but the run is still active.
    service
        .defer_cancel_terminal()
        .store(true, Ordering::SeqCst);
    let (mut stdin_writer, mut stdout_reader, run) = spawn_cli(service.clone());
    let mut output = String::new();
    let session_a = SessionId::parse_str(SESSION_A).unwrap();

    read_until(&mut stdout_reader, &mut output, SESSION_A).await;
    // `hold` starts a run that never finishes on its own.
    stdin_writer
        .write_all(b"hold\n")
        .await
        .expect("start in-flight run");
    stdin_writer.write_all(b"/quit\n").await.expect("quit CLI");
    // The quit path proactively cancels the in-flight run; once that line is
    // visible the CLI is already winding down.
    read_until(&mut stdout_reader, &mut output, "[cancel requested").await;
    // A driver blocked on a fresh interaction during wind-down must not hang
    // the CLI: the interaction is answered with a cancel decision.
    service.emit_late_approval(session_a);
    read_until(&mut stdout_reader, &mut output, "cancelled during quit").await;
    service.emit_cancelled_terminal(session_a);
    stdin_writer.shutdown().await.expect("close scripted stdin");

    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run must not hang")
        .expect("CLI task must join");
    result.expect("CLI run must succeed");

    let cancels = cancels.lock().expect("lock").clone();
    assert_eq!(cancels, vec![session_a]);

    let replies = replies.lock().expect("lock").clone();
    let late_reply = replies
        .iter()
        .find(|reply| reply.request_id == RequestId::parse_str(REQ_QUIT_LATE).unwrap())
        .expect("late interaction must be answered with a cancel decision");
    assert_eq!(late_reply.session_id, session_a);
    match &late_reply.response {
        InteractionResponseWire::Approval { decision, .. } => {
            assert_eq!(*decision, ApprovalDecisionWire::Cancel);
        }
        other => panic!("expected approval cancel response, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_still_exits_after_the_event_stream_ends() {
    // The subscription closes right away; the CLI must keep servicing input
    // (instead of busy-spinning on the dead notice channel) and exit normally
    // on /quit. The join timeout is the backstop against a hang regression.
    let service = Arc::new(EndingStreamService::new());
    let (mut stdin_writer, mut stdout_reader, run) = spawn_cli_dyn(service);
    let mut output = String::new();

    read_until(&mut stdout_reader, &mut output, "[session").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    stdin_writer.write_all(b"/quit\n").await.expect("quit CLI");
    stdin_writer.shutdown().await.expect("close scripted stdin");

    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("CLI run must not hang after the event stream ended")
        .expect("CLI task must join");
    result.expect("CLI run must succeed");
}
