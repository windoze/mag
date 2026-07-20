#![warn(missing_docs)]

//! Minimal terminal CLI adapter for mag (`docs/CLI.md` §1/§2).
//!
//! `mag-cli` is intentionally an interface crate: it talks only to an injected
//! [`Arc<dyn MagService>`](mag_service::MagService), owns no agent logic, and
//! keeps the production assembly point in the top-level `mag` binary. It follows
//! the minimal REPL shape from `docs/CLI.md` §1.2: an input task reads lines, a
//! render task prints service events, and the coordinator turns user text into
//! [`MagService::send_message`] calls or resolves queued interaction prompts.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io::IsTerminal;
use std::sync::Arc;

use futures::{Stream, StreamExt};
use mag_service::{
    ApprovalDecisionWire, ApprovalRequirementWire, ConfigDto, DelegationMessageWire,
    DelegationTrace, InteractionKindWire, InteractionOrigin, InteractionResponseWire, MagService,
    PermissionCategoryWire, PermissionDecisionWire, PermissionRiskWire, RequestId, RoutingMode,
    RunErrorKind, ServiceError, ServiceEvent, SessionConfig, SessionId, SessionInfo, SourceInfo,
    SourceKindWire, StepIdWire, ToolCallIdWire, ToolStatusWire, ToolTrace, UserInput,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Stdout};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

/// Default provider used when the CLI creates a session without a user-supplied
/// agent selection.
pub const DEFAULT_PROVIDER: &str = "openai";

/// Default model used when the CLI creates a session without a user-supplied
/// agent selection.
pub const DEFAULT_MODEL: &str = "gpt-5-codex";

type SharedOutput<W> = Arc<Mutex<W>>;

/// Runtime options for [`Cli::run`].
#[derive(Clone, Debug)]
pub struct CliOptions {
    /// Session configuration used at startup and by `/new`.
    pub session: SessionConfig,
    /// Existing session to resume at startup instead of creating a new session.
    pub resume: Option<SessionId>,
    /// Prompt displayed before each input line in the pipe-friendly reader.
    pub prompt: String,
}

impl Default for CliOptions {
    fn default() -> Self {
        Self {
            session: SessionConfig {
                provider: DEFAULT_PROVIDER.to_owned(),
                model: DEFAULT_MODEL.to_owned(),
                tool_profile: None,
                cwd: None,
                routing: RoutingMode::default(),
                budget: None,
            },
            resume: None,
            prompt: "mag> ".to_owned(),
        }
    }
}

/// Error returned by the CLI adapter.
#[derive(Debug)]
pub enum CliError {
    /// A [`MagService`] call failed.
    Service(ServiceError),
    /// Reading stdin or writing stdout failed.
    Io(std::io::Error),
    /// The terminal line editor failed.
    Readline(String),
    /// A spawned input or render task failed to join.
    Join(tokio::task::JoinError),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Service(error) => write!(formatter, "service error: {error}"),
            Self::Io(error) => write!(formatter, "io error: {error}"),
            Self::Readline(error) => write!(formatter, "readline error: {error}"),
            Self::Join(error) => write!(formatter, "task join error: {error}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<ServiceError> for CliError {
    fn from(error: ServiceError) -> Self {
        Self::Service(error)
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<tokio::task::JoinError> for CliError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::Join(error)
    }
}

/// Terminal CLI entry point.
pub struct Cli;

impl Cli {
    /// Runs the CLI on process stdin/stdout.
    ///
    /// Interactive terminals use `rustyline` for line editing and history;
    /// non-terminal stdin falls back to the same pipe-friendly line reader used
    /// by tests so scripted e2e runs can drive the REPL through stdin/stdout.
    ///
    /// # Errors
    ///
    /// Returns [`CliError`] if session creation, service calls, line editing, or
    /// stdio operations fail.
    pub async fn run(service: Arc<dyn MagService>, opts: CliOptions) -> Result<(), CliError> {
        if std::io::stdin().is_terminal() {
            run_with_rustyline(service, opts).await
        } else {
            Self::run_with_io(service, opts, tokio::io::stdin(), tokio::io::stdout()).await
        }
    }

    /// Runs the CLI against caller-supplied input and output streams.
    ///
    /// This is the headless path used by offline e2e tests. It keeps the same
    /// coordinator and render task as [`Cli::run`], but reads plain newline-
    /// delimited input instead of invoking terminal line editing.
    ///
    /// # Errors
    ///
    /// Returns [`CliError`] if session creation, service calls, or stream I/O
    /// fails.
    pub async fn run_with_io<R, W>(
        service: Arc<dyn MagService>,
        opts: CliOptions,
        input: R,
        output: W,
    ) -> Result<(), CliError>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        run_event_loop(service, opts, output, move |output, prompt, tx| {
            tokio::spawn(read_pipe_lines(input, output, prompt, tx))
        })
        .await
    }
}

enum InputCommand {
    Line(String),
    Interrupted,
    Eof,
    Io(std::io::Error),
    Readline(String),
}

enum RenderNotice {
    RunStarted(SessionId),
    Terminal(SessionId),
    Interaction(Box<PendingInteraction>),
    StreamEnded,
    Io(std::io::Error),
}

enum LineOutcome {
    Continue,
    StartedRun {
        id: SessionId,
        skip_next_terminal: bool,
    },
    Quit,
}

#[derive(Clone, Debug)]
struct PendingInteraction {
    session_id: SessionId,
    request_id: RequestId,
    kind: InteractionKindWire,
    origin: InteractionOrigin,
}

#[derive(Default)]
struct PromptCoordinator {
    queue: VecDeque<PendingInteraction>,
    active: Option<PendingInteraction>,
}

impl PromptCoordinator {
    fn has_pending(&self) -> bool {
        self.active.is_some() || !self.queue.is_empty()
    }

    async fn enqueue<W>(
        &mut self,
        output: &SharedOutput<W>,
        current_session: SessionId,
        interaction: PendingInteraction,
    ) -> Result<(), CliError>
    where
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let interaction_session = interaction.session_id;
        self.queue.push_back(interaction);

        if interaction_session != current_session {
            write_line(
                output,
                &format!(
                    "\n[interaction pending for session {}; switch sessions to answer it]\n",
                    interaction_session
                ),
            )
            .await?;
            return Ok(());
        }

        self.prompt_next(output, current_session).await
    }

    async fn answer<W>(
        &mut self,
        service: &Arc<dyn MagService>,
        output: &SharedOutput<W>,
        line: String,
    ) -> Result<bool, CliError>
    where
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let Some(active) = self.active.clone() else {
            return Ok(false);
        };

        let response = match response_from_line(&active.kind, &line) {
            Ok(response) => response,
            Err(message) => {
                write_line(output, &format!("[error] {message}\n")).await?;
                write_interaction_prompt(output, &active).await?;
                return Ok(true);
            }
        };

        self.respond(service, output, response).await?;
        Ok(true)
    }

    async fn cancel_active<W>(
        &mut self,
        service: &Arc<dyn MagService>,
        output: &SharedOutput<W>,
    ) -> Result<bool, CliError>
    where
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let Some(active) = self.active.clone() else {
            return Ok(false);
        };
        let response = cancellation_response(&active.kind);
        self.respond(service, output, response).await?;
        Ok(true)
    }

    async fn cancel_all<W>(
        &mut self,
        service: &Arc<dyn MagService>,
        _output: &SharedOutput<W>,
    ) -> Result<(), CliError>
    where
        W: AsyncWrite + Send + Unpin + 'static,
    {
        while self.active.is_some() || !self.queue.is_empty() {
            if self.active.is_none() {
                self.active = self.queue.pop_front();
            }
            let Some(active) = self.active.take() else {
                continue;
            };
            service
                .respond_interaction(
                    active.session_id,
                    active.request_id,
                    cancellation_response(&active.kind),
                )
                .await?;
        }
        Ok(())
    }

    async fn respond<W>(
        &mut self,
        service: &Arc<dyn MagService>,
        output: &SharedOutput<W>,
        response: InteractionResponseWire,
    ) -> Result<(), CliError>
    where
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let Some(active) = self.active.take() else {
            return Ok(());
        };
        let session_id = active.session_id;
        service
            .respond_interaction(active.session_id, active.request_id, response)
            .await?;
        self.prompt_next(output, session_id).await
    }

    async fn prompt_next<W>(
        &mut self,
        output: &SharedOutput<W>,
        current_session: SessionId,
    ) -> Result<(), CliError>
    where
        W: AsyncWrite + Send + Unpin + 'static,
    {
        if self.active.is_none()
            && let Some(position) = self
                .queue
                .iter()
                .position(|interaction| interaction.session_id == current_session)
        {
            self.active = self.queue.remove(position);
        }
        if let Some(active) = &self.active {
            write_interaction_prompt(output, active).await?;
        }
        Ok(())
    }
}

async fn run_with_rustyline(
    service: Arc<dyn MagService>,
    opts: CliOptions,
) -> Result<(), CliError> {
    run_event_loop(
        service,
        opts,
        tokio::io::stdout(),
        |output: SharedOutput<Stdout>, prompt, tx| spawn_rustyline(output, prompt, tx),
    )
    .await
}

async fn run_event_loop<W, SpawnInput>(
    service: Arc<dyn MagService>,
    opts: CliOptions,
    output: W,
    spawn_input: SpawnInput,
) -> Result<(), CliError>
where
    W: AsyncWrite + Send + Unpin + 'static,
    SpawnInput: FnOnce(SharedOutput<W>, String, mpsc::Sender<InputCommand>) -> JoinHandle<()>,
{
    let output = Arc::new(Mutex::new(output));
    let mut session_id = if let Some(id) = opts.resume {
        service.resume_session(id).await?;
        write_line(&output, &format!("[session {id} resumed]\n")).await?;
        id
    } else {
        let id = service.create_session(opts.session.clone()).await?;
        write_line(&output, &format!("[session {id}]\n")).await?;
        id
    };

    let (input_tx, mut input_rx) = mpsc::channel(8);
    let (notice_tx, mut notice_rx) = mpsc::channel(8);

    let render_output = Arc::clone(&output);
    let events = service.subscribe(None);
    let render_handle = tokio::spawn(render_events(events, render_output, notice_tx));
    let input_handle = spawn_input(Arc::clone(&output), opts.prompt.clone(), input_tx);

    let mut active_runs = HashSet::new();
    let mut skipped_terminals = HashMap::<SessionId, usize>::new();
    let mut quitting = false;
    let mut prompts = PromptCoordinator::default();

    loop {
        if quitting && active_runs.is_empty() {
            break;
        }

        tokio::select! {
            command = input_rx.recv(), if !quitting => {
                match command {
                    Some(InputCommand::Line(line)) => {
                        if prompts.answer(&service, &output, line.clone()).await? {
                            continue;
                        }
                        let current_session_running = active_runs.contains(&session_id);
                        match handle_line(
                            &service,
                            &opts,
                            &output,
                            &mut session_id,
                            line,
                            current_session_running,
                        ).await? {
                            LineOutcome::Continue => {}
                            LineOutcome::StartedRun {
                                id,
                                skip_next_terminal,
                            } => {
                                active_runs.insert(id);
                                if skip_next_terminal {
                                    *skipped_terminals.entry(id).or_default() += 1;
                                }
                            }
                            LineOutcome::Quit => {
                                if prompts.has_pending() {
                                    prompts.cancel_all(&service, &output).await?;
                                }
                                quitting = true;
                            }
                        }
                        if !quitting {
                            prompts.prompt_next(&output, session_id).await?;
                        }
                    }
                    Some(InputCommand::Interrupted) => {
                        if prompts.cancel_active(&service, &output).await? {
                            continue;
                        }
                        if active_runs.contains(&session_id) {
                            request_cancel(&service, &output, session_id).await?;
                        }
                    }
                    Some(InputCommand::Eof) | None => {
                        if prompts.has_pending() {
                            prompts.cancel_all(&service, &output).await?;
                        }
                        quitting = true;
                    }
                    Some(InputCommand::Io(error)) => return Err(CliError::Io(error)),
                    Some(InputCommand::Readline(error)) => return Err(CliError::Readline(error)),
                }
            }
            notice = notice_rx.recv() => {
                match notice {
                    Some(RenderNotice::Interaction(interaction)) => {
                        prompts.enqueue(&output, session_id, *interaction).await?;
                    }
                    Some(RenderNotice::RunStarted(id)) => {
                        active_runs.insert(id);
                    }
                    Some(RenderNotice::Terminal(id)) => {
                        if should_skip_terminal(&mut skipped_terminals, id) {
                            continue;
                        }
                        active_runs.remove(&id);
                    }
                    Some(RenderNotice::StreamEnded) | None => {
                        active_runs.clear();
                        skipped_terminals.clear();
                        if quitting {
                            break;
                        }
                    }
                    Some(RenderNotice::Io(error)) => return Err(CliError::Io(error)),
                }
            }
        }
    }

    input_handle.abort();
    render_handle.abort();
    let _ = input_handle.await;
    let _ = render_handle.await;
    output.lock().await.flush().await?;
    Ok(())
}

async fn handle_line<W>(
    service: &Arc<dyn MagService>,
    opts: &CliOptions,
    output: &SharedOutput<W>,
    session_id: &mut SessionId,
    line: String,
    current_session_running: bool,
) -> Result<LineOutcome, CliError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(LineOutcome::Continue);
    }

    if trimmed.starts_with('/') {
        return handle_slash_command(service, opts, output, session_id, trimmed).await;
    }

    if current_session_running {
        pivot_or_send_message(service, output, *session_id, line).await
    } else {
        send_message(service, output, *session_id, line).await
    }
}

async fn handle_slash_command<W>(
    service: &Arc<dyn MagService>,
    opts: &CliOptions,
    output: &SharedOutput<W>,
    session_id: &mut SessionId,
    line: &str,
) -> Result<LineOutcome, CliError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut parts = line.split_whitespace();
    let command = parts.next().unwrap_or_default();
    match command {
        "/quit" => Ok(LineOutcome::Quit),
        "/new" => {
            if parts.next().is_some() {
                write_line(output, "[error] usage: /new\n").await?;
                return Ok(LineOutcome::Continue);
            }
            match service.create_session(opts.session.clone()).await {
                Ok(new_session) => {
                    *session_id = new_session;
                    write_line(output, &format!("[session {new_session}]\n")).await?;
                }
                Err(error) => write_line(output, &format!("[error] {error}\n")).await?,
            }
            Ok(LineOutcome::Continue)
        }
        "/sessions" => {
            if parts.next().is_some() {
                write_line(output, "[error] usage: /sessions\n").await?;
                return Ok(LineOutcome::Continue);
            }
            match service.list_sessions().await {
                Ok(sessions) => render_sessions(output, &sessions, *session_id).await?,
                Err(error) => write_line(output, &format!("[error] {error}\n")).await?,
            }
            Ok(LineOutcome::Continue)
        }
        "/resume" => {
            let Some(raw_id) = parts.next() else {
                write_line(output, "[error] usage: /resume <session-id>\n").await?;
                return Ok(LineOutcome::Continue);
            };
            if parts.next().is_some() {
                write_line(output, "[error] usage: /resume <session-id>\n").await?;
                return Ok(LineOutcome::Continue);
            }
            let id = match SessionId::parse_str(raw_id) {
                Ok(id) => id,
                Err(error) => {
                    write_line(output, &format!("[error] invalid session id: {error}\n")).await?;
                    return Ok(LineOutcome::Continue);
                }
            };
            match service.resume_session(id).await {
                Ok(()) => {
                    *session_id = id;
                    write_line(output, &format!("[session {id} resumed]\n")).await?;
                }
                Err(error) => write_line(output, &format!("[error] {error}\n")).await?,
            }
            Ok(LineOutcome::Continue)
        }
        "/delete" => {
            let Some(raw_id) = parts.next() else {
                write_line(output, "[error] usage: /delete <session-id>\n").await?;
                return Ok(LineOutcome::Continue);
            };
            if parts.next().is_some() {
                write_line(output, "[error] usage: /delete <session-id>\n").await?;
                return Ok(LineOutcome::Continue);
            }
            let id = match SessionId::parse_str(raw_id) {
                Ok(id) => id,
                Err(error) => {
                    write_line(output, &format!("[error] invalid session id: {error}\n")).await?;
                    return Ok(LineOutcome::Continue);
                }
            };
            match service.delete_session(id).await {
                Ok(()) => write_line(output, &format!("[session {id} deleted]\n")).await?,
                Err(error) => write_line(output, &format!("[error] {error}\n")).await?,
            }
            Ok(LineOutcome::Continue)
        }
        "/cancel" => {
            if parts.next().is_some() {
                write_line(output, "[error] usage: /cancel\n").await?;
                return Ok(LineOutcome::Continue);
            }
            request_cancel(service, output, *session_id).await?;
            Ok(LineOutcome::Continue)
        }
        "/sources" => {
            if parts.next().is_some() {
                write_line(output, "[error] usage: /sources\n").await?;
                return Ok(LineOutcome::Continue);
            }
            match service.list_sources().await {
                Ok(sources) => render_sources(output, "sources", &sources).await?,
                Err(error) => {
                    write_line(output, &format!("[error] list_sources: {error}\n")).await?
                }
            }
            match service.probe_local_agents().await {
                Ok(sources) => render_sources(output, "probed sources", &sources).await?,
                Err(error) => {
                    write_line(output, &format!("[error] probe_local_agents: {error}\n")).await?
                }
            }
            Ok(LineOutcome::Continue)
        }
        "/config" => handle_config_command(service, output, parts).await,
        "/help" => {
            if parts.next().is_some() {
                write_line(output, "[error] usage: /help\n").await?;
                return Ok(LineOutcome::Continue);
            }
            write_line(
                output,
                "commands: /new, /sessions, /resume <id>, /delete <id>, /cancel, /sources, /config <show|reload|apply>, /help, /quit\n",
            )
            .await?;
            Ok(LineOutcome::Continue)
        }
        _ => {
            write_line(output, &format!("[error] unknown command `{command}`\n")).await?;
            Ok(LineOutcome::Continue)
        }
    }
}

async fn handle_config_command<W>(
    service: &Arc<dyn MagService>,
    output: &SharedOutput<W>,
    mut parts: std::str::SplitWhitespace<'_>,
) -> Result<LineOutcome, CliError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    let Some(subcommand) = parts.next() else {
        write_line(output, "[error] usage: /config <show|reload|apply>\n").await?;
        return Ok(LineOutcome::Continue);
    };
    if parts.next().is_some() {
        write_line(output, "[error] usage: /config <show|reload|apply>\n").await?;
        return Ok(LineOutcome::Continue);
    }

    match subcommand {
        "show" => match service.get_config().await {
            Ok(config) => render_config(output, &config).await?,
            Err(error) => write_line(output, &format!("[error] {error}\n")).await?,
        },
        "reload" => match service.reload_config().await {
            Ok(()) => write_line(output, "[config reloaded]\n").await?,
            Err(error) => write_line(output, &format!("[error] {error}\n")).await?,
        },
        "apply" => match service.apply_config().await {
            Ok(()) => {
                write_line(
                    output,
                    "[config apply requested; changes will apply at each session's next turn boundary]\n",
                )
                .await?
            }
            Err(error) => write_line(output, &format!("[error] {error}\n")).await?,
        },
        _ => write_line(output, "[error] usage: /config <show|reload|apply>\n").await?,
    }
    Ok(LineOutcome::Continue)
}

async fn render_config<W>(
    output: &SharedOutput<W>,
    config: &ConfigDto,
) -> Result<(), std::io::Error>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    match config.to_string_pretty() {
        Ok(mut toml) => {
            if toml.is_empty() {
                toml.push_str("# empty config\n");
            } else if !toml.ends_with('\n') {
                toml.push('\n');
            }
            write_line(output, &toml).await
        }
        Err(error) => write_line(output, &format!("[error] config serialization: {error}\n")).await,
    }
}

async fn pivot_or_send_message<W>(
    service: &Arc<dyn MagService>,
    output: &SharedOutput<W>,
    session_id: SessionId,
    line: String,
) -> Result<LineOutcome, CliError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    match service
        .pivot_message(session_id, UserInput::text(line.clone()))
        .await
    {
        Ok(()) => Ok(LineOutcome::Continue),
        Err(ServiceError::NotPivotable { .. }) => {
            send_message_with_terminal_skip(service, output, session_id, line).await
        }
        Err(error) => {
            write_line(output, &format!("[error] {error}\n")).await?;
            Ok(LineOutcome::Continue)
        }
    }
}

async fn send_message<W>(
    service: &Arc<dyn MagService>,
    output: &SharedOutput<W>,
    session_id: SessionId,
    line: String,
) -> Result<LineOutcome, CliError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    send_message_inner(service, output, session_id, line, false).await
}

async fn send_message_with_terminal_skip<W>(
    service: &Arc<dyn MagService>,
    output: &SharedOutput<W>,
    session_id: SessionId,
    line: String,
) -> Result<LineOutcome, CliError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    send_message_inner(service, output, session_id, line, true).await
}

async fn send_message_inner<W>(
    service: &Arc<dyn MagService>,
    output: &SharedOutput<W>,
    session_id: SessionId,
    line: String,
    skip_next_terminal: bool,
) -> Result<LineOutcome, CliError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    match service
        .send_message(session_id, UserInput::text(line))
        .await
    {
        Ok(_) => Ok(LineOutcome::StartedRun {
            id: session_id,
            skip_next_terminal,
        }),
        Err(error) => {
            write_line(output, &format!("[error] {error}\n")).await?;
            Ok(LineOutcome::Continue)
        }
    }
}

async fn request_cancel<W>(
    service: &Arc<dyn MagService>,
    output: &SharedOutput<W>,
    session_id: SessionId,
) -> Result<(), CliError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    match service.cancel(session_id).await {
        Ok(()) => write_line(output, &format!("[cancel requested {session_id}]\n")).await?,
        Err(error) => write_line(output, &format!("[error] {error}\n")).await?,
    }
    Ok(())
}

fn should_skip_terminal(skipped_terminals: &mut HashMap<SessionId, usize>, id: SessionId) -> bool {
    let Some(skips) = skipped_terminals.get_mut(&id) else {
        return false;
    };
    *skips -= 1;
    if *skips == 0 {
        skipped_terminals.remove(&id);
    }
    true
}

async fn render_sessions<W>(
    output: &SharedOutput<W>,
    sessions: &[SessionInfo],
    current_session: SessionId,
) -> Result<(), std::io::Error>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut rendered = "[sessions]\n".to_owned();
    if sessions.is_empty() {
        rendered.push_str("(none)\n");
    } else {
        for session in sessions {
            let marker = if session.id == current_session {
                "*"
            } else {
                " "
            };
            rendered.push_str(&format!(
                "{marker} {} provider={} model={}\n",
                session.id, session.config.provider, session.config.model
            ));
        }
    }
    write_line(output, &rendered).await
}

async fn render_sources<W>(
    output: &SharedOutput<W>,
    label: &str,
    sources: &[SourceInfo],
) -> Result<(), std::io::Error>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut rendered = format!("[{label}]\n");
    if sources.is_empty() {
        rendered.push_str("(none)\n");
    } else {
        for source in sources {
            rendered.push_str(&format!(
                "- {} name={} kind={} available={}",
                source.id,
                source.name,
                source_kind(&source.kind),
                source.available
            ));
            if let Some(version) = &source.version {
                rendered.push_str(&format!(" version={version}"));
            }
            if let Some(path) = &source.path {
                rendered.push_str(&format!(" path={path}"));
            }
            if !source.capabilities.is_empty() {
                rendered.push_str(&format!(" capabilities={}", source.capabilities.join(",")));
            }
            rendered.push('\n');
        }
    }
    write_line(output, &rendered).await
}

async fn read_pipe_lines<R, W>(
    input: R,
    output: SharedOutput<W>,
    prompt: String,
    tx: mpsc::Sender<InputCommand>,
) where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut lines = BufReader::new(input).lines();
    loop {
        if let Err(error) = write_prompt(&output, &prompt).await {
            let _ = tx.send(InputCommand::Io(error)).await;
            break;
        }

        match lines.next_line().await {
            Ok(Some(line)) => {
                if line == "\u{3}" {
                    if tx.send(InputCommand::Interrupted).await.is_err() {
                        break;
                    }
                    continue;
                }
                let quit = line.trim() == "/quit";
                if tx.send(InputCommand::Line(line)).await.is_err() || quit {
                    break;
                }
            }
            Ok(None) => {
                let _ = tx.send(InputCommand::Eof).await;
                break;
            }
            Err(error) => {
                let _ = tx.send(InputCommand::Io(error)).await;
                break;
            }
        }
    }
}

fn spawn_rustyline(
    output: SharedOutput<Stdout>,
    prompt: String,
    tx: mpsc::Sender<InputCommand>,
) -> JoinHandle<()> {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut editor = match rustyline::DefaultEditor::new() {
            Ok(editor) => editor,
            Err(error) => {
                let _ = tx.blocking_send(InputCommand::Readline(error.to_string()));
                return;
            }
        };

        loop {
            if let Err(error) = handle.block_on(write_prompt(&output, &prompt)) {
                let _ = tx.blocking_send(InputCommand::Io(error));
                break;
            }

            match editor.readline("") {
                Ok(line) => {
                    if !line.trim().is_empty() {
                        let _ = editor.add_history_entry(line.as_str());
                    }
                    let quit = line.trim() == "/quit";
                    if tx.blocking_send(InputCommand::Line(line)).is_err() || quit {
                        break;
                    }
                }
                Err(rustyline::error::ReadlineError::Interrupted) => {
                    if tx.blocking_send(InputCommand::Interrupted).is_err() {
                        break;
                    }
                }
                Err(rustyline::error::ReadlineError::Eof) => {
                    let _ = tx.blocking_send(InputCommand::Eof);
                    break;
                }
                Err(error) => {
                    let _ = tx.blocking_send(InputCommand::Readline(error.to_string()));
                    break;
                }
            }
        }
    })
}

async fn render_events<S, W>(
    mut events: S,
    output: SharedOutput<W>,
    notice_tx: mpsc::Sender<RenderNotice>,
) where
    S: Stream<Item = ServiceEvent> + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut state = RenderState::default();
    while let Some(event) = events.next().await {
        if let ServiceEvent::InteractionRequested {
            id,
            request_id,
            kind,
            origin,
        } = event
        {
            let notice = RenderNotice::Interaction(Box::new(PendingInteraction {
                session_id: id,
                request_id,
                kind,
                origin,
            }));
            if notice_tx.send(notice).await.is_err() {
                return;
            }
            continue;
        }

        let started = match &event {
            ServiceEvent::RunStarted { id, .. } => Some(*id),
            _ => None,
        };
        let terminal = match &event {
            ServiceEvent::RunFinished { id, .. } | ServiceEvent::RunError { id, .. } => Some(*id),
            _ => None,
        };
        if let Err(error) = render_event(&output, &mut state, event).await {
            let _ = notice_tx.send(RenderNotice::Io(error)).await;
            return;
        }
        if let Some(id) = started
            && notice_tx.send(RenderNotice::RunStarted(id)).await.is_err()
        {
            return;
        }
        if let Some(id) = terminal
            && notice_tx.send(RenderNotice::Terminal(id)).await.is_err()
        {
            return;
        }
    }
    let _ = notice_tx.send(RenderNotice::StreamEnded).await;
}

#[derive(Default)]
struct RenderState {
    streamed_text: bool,
}

async fn render_event<W>(
    output: &SharedOutput<W>,
    state: &mut RenderState,
    event: ServiceEvent,
) -> Result<(), std::io::Error>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    match event {
        ServiceEvent::RunStarted { .. } => {
            state.streamed_text = false;
        }
        ServiceEvent::TextDelta { text, .. } => {
            state.streamed_text = true;
            write_line(output, &text).await?;
        }
        ServiceEvent::RunFinished { output: run, .. } => {
            if !state.streamed_text && !run.text.is_empty() {
                write_line(output, &run.text).await?;
            }
            let mut line = "\n[finished".to_owned();
            if let Some(usage) = run.usage {
                line.push_str(&format!(
                    " usage input={} output={} total={}",
                    usage.input_tokens, usage.output_tokens, usage.total_tokens
                ));
            }
            line.push_str("]\n");
            write_line(output, &line).await?;
            state.streamed_text = false;
        }
        ServiceEvent::RunError { message, kind, .. } => {
            write_line(
                output,
                &format!("\n[error {}] {message}\n", run_error_kind(&kind)),
            )
            .await?;
            state.streamed_text = false;
        }
        ServiceEvent::ToolStarted { id, trace } => {
            write_line(output, &render_tool_trace("started", id, &trace)).await?;
        }
        ServiceEvent::ToolFinished { id, trace } => {
            write_line(output, &render_tool_trace("finished", id, &trace)).await?;
        }
        ServiceEvent::DelegationStarted { id, trace } => {
            write_line(output, &render_delegation_trace("started", id, &trace)).await?;
        }
        ServiceEvent::DelegationFinished { id, trace } => {
            write_line(output, &render_delegation_trace("finished", id, &trace)).await?;
        }
        ServiceEvent::DelegationFailed { id, trace } => {
            write_line(output, &render_delegation_trace("failed", id, &trace)).await?;
        }
        ServiceEvent::DelegationMessage { id, message } => {
            write_line(output, &render_delegation_message(id, &message)).await?;
        }
        ServiceEvent::PivotQueued { id } => {
            write_line(output, &format!("\n[pivot queued {id}]\n")).await?;
        }
        ServiceEvent::PivotApplied { id } => {
            write_line(output, &format!("\n[pivot applied {id}]\n")).await?;
        }
        ServiceEvent::PivotDropped { id, reason } => {
            write_line(output, &format!("\n[pivot dropped {id}] {reason}\n")).await?;
        }
        ServiceEvent::ConfigChanged { revision } => {
            write_line(output, &format!("\n[config changed revision={revision}]\n")).await?;
        }
        _ => {}
    }
    Ok(())
}

fn render_tool_trace(label: &str, id: SessionId, trace: &ToolTrace) -> String {
    let mut line = format!(
        "\n[tool {label} {id}] name={} call={} status={}",
        trace.name,
        trace.call_id,
        tool_status(&trace.status)
    );
    if let Some(input) = &trace.input {
        line.push_str(&format!(" input={}", single_line(&input.to_string())));
    }
    if let Some(output) = &trace.output {
        line.push_str(&format!(" output={}", single_line(&output.to_string())));
    }
    if let Some(message) = &trace.message {
        line.push_str(&format!(" message={}", single_line(message)));
    }
    line.push('\n');
    line
}

fn render_delegation_trace(label: &str, id: SessionId, trace: &DelegationTrace) -> String {
    let mut line = format!("\n[delegation {label} {id}] delegate={}", trace.delegate);
    if let Some(task) = &trace.task {
        line.push_str(&format!(" task={}", single_line(task)));
    }
    if let Some(output) = &trace.output {
        line.push_str(&format!(" output={}", single_line(output)));
    }
    if let Some(message) = &trace.message {
        line.push_str(&format!(" message={}", single_line(message)));
    }
    line.push('\n');
    line
}

fn render_delegation_message(id: SessionId, message: &DelegationMessageWire) -> String {
    format!(
        "\n[delegation message {id}] delegate={} text={}\n",
        message.delegate,
        single_line(&message.text)
    )
}

async fn write_interaction_prompt<W>(
    output: &SharedOutput<W>,
    interaction: &PendingInteraction,
) -> Result<(), std::io::Error>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    write_line(output, &interaction_prompt(interaction)).await
}

fn interaction_prompt(interaction: &PendingInteraction) -> String {
    let origin = origin_prefix(&interaction.origin);
    match &interaction.kind {
        InteractionKindWire::Approval {
            call_id,
            requirement,
        } => {
            let mut prompt = format!("\n{origin}[approval] tool_call={call_id}");
            if let ApprovalRequirementWire::RequireApproval {
                reason: Some(reason),
            } = requirement
            {
                prompt.push_str(&format!(" reason: {}", single_line(reason)));
            }
            prompt.push_str("\nApprove? [y/n/cancel] ");
            prompt
        }
        InteractionKindWire::Question { prompt } => {
            format!("\n{origin}[question] {prompt}\nanswer> ")
        }
        InteractionKindWire::Choice { prompt, options } => {
            let mut rendered = format!("\n{origin}[choice] {prompt}\n");
            for (index, option) in options.iter().enumerate() {
                rendered.push_str(&format!("{}. {option}\n", index + 1));
            }
            rendered.push_str("choice> ");
            rendered
        }
        InteractionKindWire::Permission {
            action_id,
            category,
            risk,
            summary,
            subject,
            reason,
            ..
        } => {
            let mut prompt = format!(
                "\n{origin}[permission] {} category={} risk={} action={}\nsubject: {}",
                single_line(summary),
                permission_category(category),
                permission_risk(risk),
                action_id,
                subject
            );
            if let Some(reason) = reason {
                prompt.push_str(&format!("\nreason: {}", single_line(reason)));
            }
            prompt.push_str("\nAllow? [y/n/cancel] ");
            prompt
        }
        _ => "\n[interaction] unsupported interaction kind; press Ctrl-C to cancel\n".to_owned(),
    }
}

fn response_from_line(
    kind: &InteractionKindWire,
    line: &str,
) -> Result<InteractionResponseWire, String> {
    match kind {
        InteractionKindWire::Approval { call_id, .. } => approval_response_from_line(call_id, line),
        InteractionKindWire::Question { .. } => Ok(InteractionResponseWire::Answer {
            text: line.to_owned(),
        }),
        InteractionKindWire::Choice { options, .. } => choice_response_from_line(options, line),
        InteractionKindWire::Permission { action_id, .. } => {
            permission_response_from_line(action_id, line)
        }
        _ => Err("unsupported interaction kind".to_owned()),
    }
}

fn approval_response_from_line(
    call_id: &ToolCallIdWire,
    line: &str,
) -> Result<InteractionResponseWire, String> {
    let decision = match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" | "approve" => ApprovalDecisionWire::Approve,
        "n" | "no" | "deny" => ApprovalDecisionWire::Deny,
        "c" | "cancel" => ApprovalDecisionWire::Cancel,
        _ => return Err("enter y, n, or cancel".to_owned()),
    };
    Ok(approval_response(call_id, decision))
}

fn choice_response_from_line(
    options: &[String],
    line: &str,
) -> Result<InteractionResponseWire, String> {
    let selection = line
        .trim()
        .parse::<usize>()
        .map_err(|_| "enter a choice number".to_owned())?;
    if selection == 0 || selection > options.len() {
        return Err(format!("enter a number from 1 to {}", options.len()));
    }
    Ok(InteractionResponseWire::Choice {
        index: selection - 1,
    })
}

fn permission_response_from_line(
    action_id: &str,
    line: &str,
) -> Result<InteractionResponseWire, String> {
    let decision = match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" | "approve" | "allow" => PermissionDecisionWire::Approve,
        "n" | "no" | "deny" => PermissionDecisionWire::Deny { reason: None },
        "c" | "cancel" => PermissionDecisionWire::Cancel,
        _ => return Err("enter y, n, or cancel".to_owned()),
    };
    Ok(InteractionResponseWire::Permission {
        action_id: action_id.to_owned(),
        decision,
    })
}

fn cancellation_response(kind: &InteractionKindWire) -> InteractionResponseWire {
    match kind {
        InteractionKindWire::Approval { call_id, .. } => {
            approval_response(call_id, ApprovalDecisionWire::Cancel)
        }
        InteractionKindWire::Question { .. } => InteractionResponseWire::Answer {
            text: String::new(),
        },
        InteractionKindWire::Choice { .. } => InteractionResponseWire::Choice { index: 0 },
        InteractionKindWire::Permission { action_id, .. } => InteractionResponseWire::Permission {
            action_id: action_id.to_owned(),
            decision: PermissionDecisionWire::Cancel,
        },
        _ => InteractionResponseWire::Answer {
            text: String::new(),
        },
    }
}

fn approval_response(
    call_id: &ToolCallIdWire,
    decision: ApprovalDecisionWire,
) -> InteractionResponseWire {
    let message = if decision == ApprovalDecisionWire::Cancel {
        Some("interaction cancelled".to_owned())
    } else {
        None
    };
    InteractionResponseWire::Approval {
        step_id: placeholder_step_id(call_id),
        call_id: *call_id,
        decision,
        message,
    }
}

fn placeholder_step_id(call_id: &ToolCallIdWire) -> StepIdWire {
    StepIdWire::new(*call_id.as_uuid())
}

fn origin_prefix(origin: &InteractionOrigin) -> String {
    match origin.delegate.as_deref() {
        Some(delegate) => format!("[from {delegate}@depth{}] ", origin.depth),
        None => String::new(),
    }
}

fn single_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

fn permission_category(category: &PermissionCategoryWire) -> &'static str {
    match category {
        PermissionCategoryWire::Shell => "shell",
        PermissionCategoryWire::FileRead => "file_read",
        PermissionCategoryWire::FileWrite => "file_write",
        PermissionCategoryWire::Network => "network",
        PermissionCategoryWire::SpawnAgent => "spawn_agent",
        PermissionCategoryWire::Mcp => "mcp",
        PermissionCategoryWire::Other => "other",
        _ => "unknown",
    }
}

fn permission_risk(risk: &PermissionRiskWire) -> &'static str {
    match risk {
        PermissionRiskWire::Low => "low",
        PermissionRiskWire::Medium => "medium",
        PermissionRiskWire::High => "high",
        PermissionRiskWire::Critical => "critical",
        _ => "unknown",
    }
}

fn source_kind(kind: &SourceKindWire) -> &'static str {
    match kind {
        SourceKindWire::LlmProvider => "llm_provider",
        SourceKindWire::LocalAgent => "local_agent",
        SourceKindWire::ToolRuntime => "tool_runtime",
        SourceKindWire::Other => "other",
        _ => "unknown",
    }
}

fn run_error_kind(kind: &RunErrorKind) -> &'static str {
    match kind {
        RunErrorKind::Other => "other",
        RunErrorKind::Cancelled => "cancelled",
        RunErrorKind::LoopLimitExceeded => "loop_limit_exceeded",
        RunErrorKind::BudgetExhausted => "budget_exhausted",
        _ => "unknown",
    }
}

fn tool_status(status: &ToolStatusWire) -> &'static str {
    match status {
        ToolStatusWire::Started => "started",
        ToolStatusWire::Finished => "finished",
        ToolStatusWire::Denied => "denied",
        ToolStatusWire::Cancelled => "cancelled",
        ToolStatusWire::Failed => "failed",
        _ => "unknown",
    }
}

async fn write_prompt<W>(output: &SharedOutput<W>, prompt: &str) -> Result<(), std::io::Error>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    write_line(output, prompt).await
}

async fn write_line<W>(output: &SharedOutput<W>, text: &str) -> Result<(), std::io::Error>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut output = output.lock().await;
    output.write_all(text.as_bytes()).await?;
    output.flush().await
}
