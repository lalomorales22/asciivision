use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

mod ai;
mod analytics;
mod client;
mod db;
mod effects;
mod memory;
mod message;
mod server;
mod shell;
mod sysmon;
mod tiling;
mod tools;
mod video;
mod webcam;

use ai::{AIClient, AIProvider, AIResponse, Message as ApiMessage, StreamChunk};
use analytics::AnalyticsPanel;
use client::VideoChatClient;
use db::Database;
use effects::EffectsEngine;
use memory::AgentMemory;
use server::VideoChatServer;
use shell::{format_outcome, run as run_shell, ShellOutcome};
use sysmon::SystemMonitor;
use tiling::{LayoutPreset, PanelKind, TilingManager};
use tools::{ToolCall, ToolResult, TrustLevel};
use video::VideoPlayer;
use webcam::WebcamCapture;

const BG_BASE: Color = Color::Rgb(3, 8, 12);
const BG_ALT: Color = Color::Rgb(10, 17, 24);
const PANEL_BG: Color = Color::Rgb(6, 13, 19);
const PANEL_ALT: Color = Color::Rgb(10, 19, 29);
const COPPER: Color = Color::Rgb(214, 153, 104);
const AMBER: Color = Color::Rgb(241, 189, 105);
const TEAL: Color = Color::Rgb(54, 154, 158);
const CYAN: Color = Color::Rgb(118, 214, 226);
const ICE: Color = Color::Rgb(207, 230, 232);
const DANGER: Color = Color::Rgb(225, 92, 84);
const MUTED: Color = Color::Rgb(101, 121, 134);
const INTRO_DURATION: Duration = Duration::from_millis(7600);

const LARGE_LOGO: &[&str] = &[
    "   █████╗ ███████╗  ██████╗██╗██╗██╗   ██╗██╗███████╗██╗ ██████╗ ███╗   ██╗",
    "  ██╔══██╗██╔════╝ ██╔════╝██║██║██║   ██║██║██╔════╝██║██╔═══██╗████╗  ██║",
    "  ███████║███████╗ ██║     ██║██║██║   ██║██║███████╗██║██║   ██║██╔██╗ ██║",
    "  ██╔══██║╚════██║ ██║     ██║╚██╗██╔╝ ██║██║╚════██║██║██║   ██║██║╚██╗██║",
    "  ██║  ██║███████║ ╚██████╗██║ ╚████╔╝  ██║███████║██║╚██████╔╝██║ ╚████║",
    "  ╚═╝  ╚═╝╚══════╝  ╚═════╝╚═╝  ╚═══╝   ╚═╝╚══════╝╚═╝ ╚═════╝ ╚═╝  ╚═══╝",
];

const SMALL_LOGO: &[&str] = &[
    "    ___   _____  _________   _  ______  _  ___  _   ",
    "   /   | / ___/ / ____/   | | |/ /  _/ / |/ / |/ |  ",
    "  / /| | \\__ \\ / /   / /| | |   // /  /    /|   /   ",
    " / ___ |___/ // /___/ ___ |/   |/ /  / /| |/   |    ",
    "/_/  |_/____/ \\____/_/  |_/_/|_/___//_/ |_/_/|_|    ",
];

const SCROLLER_TEXT: &str =
    " ASCIIVISION v2.0 // AI DEMOZONE // LIVE VIDEO CHAT // WEBCAM ASCII // 3D EFFECTS ENGINE // !bash !curl !brew // F2 MODEL // F3 VIDEO // F4 3D FX // F5 WEBCAM // F6 ANALYTICS // F7 CYCLE FX // CTRL+L PURGE // THIS TERMINAL HAS LEFT THE BUILDING ";

#[derive(Parser, Debug)]
#[command(
    name = "asciivision",
    about = "All-in-one terminal powerhouse: AI chat, live video, webcam streaming, 3D effects, analytics"
)]
struct Args {
    #[arg(long, default_value = "claude")]
    provider: String,

    #[arg(long)]
    background_video: Option<String>,

    #[arg(long)]
    intro_video: Option<String>,

    #[arg(long, default_value_t = false)]
    skip_intro: bool,

    #[arg(long, default_value_t = false)]
    no_video: bool,

    #[arg(long, default_value_t = false)]
    no_db: bool,

    /// Start WebSocket video chat server on this port
    #[arg(long)]
    serve: Option<u16>,

    /// Connect to a video chat server
    #[arg(long)]
    connect: Option<String>,

    /// Username for video chat
    #[arg(long, default_value = "anon")]
    username: String,

    /// Enable webcam on startup
    #[arg(long, default_value_t = false)]
    webcam: bool,

    /// Start with 3D effects active
    #[arg(long, default_value_t = false)]
    effects: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AppMode {
    Intro,
    Chat,
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MessageKind {
    User,
    Assistant,
    Shell,
    System,
}

struct ChatMessage {
    kind: MessageKind,
    label: String,
    content: String,
    accent: Color,
    include_in_context: bool,
    context_role: &'static str,
}

struct PendingApprovalState {
    tool_calls: Vec<ToolCall>,
    context: Vec<ApiMessage>,
    session_id: u64,
}

struct RevealJob {
    message_index: usize,
    full_text: Vec<char>,
    revealed: usize,
    speed: usize,
}

#[allow(dead_code)]
enum AppEvent {
    AiFinished {
        session_id: u64,
        result: std::result::Result<String, String>,
    },
    AiToolCalls {
        session_id: u64,
        tool_calls: Vec<ToolCall>,
        text: String,
        context: Vec<ApiMessage>,
    },
    ToolResultsReady {
        session_id: u64,
        tool_calls: Vec<ToolCall>,
        tool_results: Vec<ToolResult>,
        context: Vec<ApiMessage>,
    },
    StreamChunk {
        session_id: u64,
        chunk: StreamChunk,
    },
    ShellFinished {
        outcome: ShellOutcome,
    },
    PendingApproval {
        session_id: u64,
        tool_calls: Vec<ToolCall>,
        context: Vec<ApiMessage>,
    },
}

struct App {
    mode: AppMode,
    provider: AIProvider,
    ai_client: AIClient,
    video: Option<VideoPlayer>,
    video_enabled: bool,
    input: String,
    messages: Vec<ChatMessage>,
    reveal_queue: VecDeque<RevealJob>,
    show_help: bool,
    follow_tail: bool,
    scroll_lines: usize,
    pending_ai: bool,
    pending_shells: usize,
    session_id: u64,
    recent_commands: VecDeque<String>,
    last_shell_status: String,
    events_tx: mpsc::UnboundedSender<AppEvent>,
    events_rx: mpsc::UnboundedReceiver<AppEvent>,
    db: Option<Database>,
    last_tick: Instant,
    intro_started: Instant,
    status_note: String,

    // new modules
    effects: EffectsEngine,
    analytics: AnalyticsPanel,
    tiling: TilingManager,
    sysmon: SystemMonitor,
    webcam: Option<WebcamCapture>,
    webcam_frame: Option<video::AsciiFrame>,
    video_chat: Option<VideoChatClient>,
    username: String,
    /// cached body area for tiling direction calculations
    body_area: Rect,

    // Phase 1: Agentic features
    trust_level: TrustLevel,
    agent_memory: AgentMemory,
    pending_approval: Option<PendingApprovalState>,
    tool_loop_depth: usize,
    streaming_active: bool,
    last_esc_time: Instant,
    stream_buffer: String,
    stream_message_index: Option<usize>,
    pinned_messages: Vec<usize>,
    shell_output_history: VecDeque<String>,
}

impl ChatMessage {
    fn user(content: String) -> Self {
        Self {
            kind: MessageKind::User,
            label: "YOU".to_string(),
            content,
            accent: CYAN,
            include_in_context: true,
            context_role: "user",
        }
    }

    fn assistant(provider: &AIProvider) -> Self {
        Self {
            kind: MessageKind::Assistant,
            label: provider.name().to_string(),
            content: String::new(),
            accent: provider.color(),
            include_in_context: true,
            context_role: "assistant",
        }
    }

    fn shell(accent: Color) -> Self {
        Self {
            kind: MessageKind::Shell,
            label: "OPS".to_string(),
            content: String::new(),
            accent,
            include_in_context: true,
            context_role: "user",
        }
    }

    fn system(content: impl Into<String>) -> Self {
        Self {
            kind: MessageKind::System,
            label: "SYSTEM".to_string(),
            content: content.into(),
            accent: COPPER,
            include_in_context: false,
            context_role: "user",
        }
    }
}

impl RevealJob {
    fn new(message_index: usize, text: String, speed: usize) -> Self {
        Self {
            message_index,
            full_text: text.chars().collect(),
            revealed: 0,
            speed,
        }
    }
}

impl App {
    fn new(args: Args) -> Result<Self> {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let provider = AIProvider::from_input(&args.provider);
        let video_path = if args.no_video {
            None
        } else {
            resolve_video_path(args.background_video, args.intro_video)
        };
        let video = match video_path {
            Some(path) => Some(VideoPlayer::new(path, (132, 46), true)?),
            None => None,
        };

        let db = if args.no_db {
            None
        } else {
            Database::new().ok()
        };

        let mut effects = EffectsEngine::new();
        if args.effects {
            effects.active = true;
        }

        let webcam = if args.webcam {
            let config = webcam::WebcamConfig {
                width: 160,
                height: 48,
                ..webcam::WebcamConfig::default()
            };
            WebcamCapture::start(config).ok()
        } else {
            None
        };

        let mut agent_memory = AgentMemory::new();
        if let Some(ref db) = db {
            let _ = AgentMemory::init_table(db);
            agent_memory.load(db);
        }

        let mut app = Self {
            mode: if args.skip_intro {
                AppMode::Chat
            } else {
                AppMode::Intro
            },
            provider: provider.clone(),
            ai_client: AIClient::new(provider),
            video_enabled: true,
            video,
            input: String::new(),
            messages: Vec::new(),
            reveal_queue: VecDeque::new(),
            show_help: false,
            follow_tail: true,
            scroll_lines: 0,
            pending_ai: false,
            pending_shells: 0,
            session_id: 0,
            recent_commands: VecDeque::new(),
            last_shell_status: "shell bus idle".to_string(),
            events_tx,
            events_rx,
            db,
            last_tick: Instant::now(),
            intro_started: Instant::now(),
            status_note: "cold boot // intro online".to_string(),

            effects,
            analytics: AnalyticsPanel::new(),
            tiling: TilingManager::new(),
            sysmon: SystemMonitor::new(),
            webcam,
            webcam_frame: None,
            video_chat: None,
            username: args.username.clone(),
            body_area: Rect::default(),

            trust_level: TrustLevel::ConfirmDestructive,
            agent_memory,
            pending_approval: None,
            tool_loop_depth: 0,
            streaming_active: false,
            last_esc_time: Instant::now() - Duration::from_secs(10),
            stream_buffer: String::new(),
            stream_message_index: None,
            pinned_messages: Vec::new(),
            shell_output_history: VecDeque::new(),
        };

        app.add_system_message(
            "shell deck armed: use !<command> for bash, or /curl and /brew for shortcuts",
        );
        app.add_system_message(format!(
            "provider uplink live: {} // F2 rotate // F4 3D fx // F5 webcam // F7 cycle fx",
            app.provider.name()
        ));
        app.add_system_message(format!(
            "agentic mode online: tool-use loop active // trust level: {} // /trust to cycle",
            app.trust_level.name()
        ));
        app.add_system_message(
            "context: @<filepath> to inject file // /pin to pin messages // /remember <key>=<value> to store memory"
        );
        app.add_system_message(
            "video chat: /server <port> to host, /connect ws://<addr> to join, /chat <msg> to send"
        );
        app.add_system_message(
            "tiling: Ctrl+hjkl focus, Ctrl+Shift+hjkl swap, Ctrl+[/] resize, Ctrl+n cycle panel, /layout cycle preset"
        );

        if app.video.is_none() {
            app.add_system_message("video signal offline: no bundled mp4 found, falling back to synthetic raster field");
        }

        if app.db.is_none() && !args.no_db {
            app.add_system_message(
                "conversation archive offline: ~/.config/asciivision could not be initialized",
            );
        }

        if app.webcam.is_some() {
            app.add_system_message("webcam capture online: live ascii feed active");
        }

        Ok(app)
    }

    fn tick(&mut self) {
        if matches!(self.mode, AppMode::Intro) && self.intro_started.elapsed() >= INTRO_DURATION {
            self.mode = AppMode::Chat;
            self.status_note = "intro faded into live deck".to_string();
        }

        if let Some(video) = &mut self.video {
            if self.video_enabled || matches!(self.mode, AppMode::Intro) {
                video.tick();
            }
        }

        self.sysmon.refresh();

        // poll webcam -- drain all buffered frames to keep latency low
        if let Some(ref cam) = self.webcam {
            while let Some(frame) = cam.try_recv() {
                self.webcam_frame = Some(frame);
            }
            if self.webcam_frame.is_none() {
                if let Some(err) = cam.error() {
                    self.status_note = format!("webcam: {}", truncate(&err, 40));
                }
            }
        }

        while let Ok(event) = self.events_rx.try_recv() {
            match event {
                AppEvent::AiFinished { session_id, result } => {
                    if session_id != self.session_id {
                        continue;
                    }

                    self.pending_ai = false;
                    self.tool_loop_depth = 0;
                    self.streaming_active = false;
                    self.stream_buffer.clear();
                    self.stream_message_index = None;
                    match result {
                        Ok(text) => {
                            let message = ChatMessage::assistant(&self.provider);
                            let index = self.messages.len();
                            self.messages.push(message);
                            self.persist(&self.provider, "assistant", "chat", &text);
                            self.reveal_queue.push_back(RevealJob::new(index, text, 9));
                            self.follow_tail = true;
                            self.status_note =
                                format!("{} response injected", self.provider.badge());
                        }
                        Err(error) => {
                            self.add_system_message(format!("provider fault: {}", error));
                            self.status_note = "provider fault".to_string();
                        }
                    }
                }
                AppEvent::AiToolCalls {
                    session_id,
                    tool_calls,
                    text,
                    context,
                } => {
                    if session_id != self.session_id {
                        continue;
                    }

                    if !text.is_empty() {
                        let message = ChatMessage::assistant(&self.provider);
                        let index = self.messages.len();
                        self.messages.push(message);
                        self.persist(&self.provider, "assistant", "chat", &text);
                        self.reveal_queue.push_back(RevealJob::new(index, text, 12));
                    }

                    let needs_approval = match self.trust_level {
                        TrustLevel::FullAuto => false,
                        TrustLevel::ConfirmAll => true,
                        TrustLevel::ConfirmDestructive => tool_calls
                            .iter()
                            .any(|tc| tools::is_destructive(&tc.name, &tc.arguments)),
                    };

                    if needs_approval {
                        let summary: Vec<String> = tool_calls
                            .iter()
                            .map(|tc| format!("{}({})", tc.name, truncate(&tc.arguments.to_string(), 60)))
                            .collect();
                        self.add_system_message(format!(
                            "APPROVAL REQUIRED: agent wants to execute:\n  {}\nPress Enter to approve, Esc to reject",
                            summary.join("\n  ")
                        ));
                        self.pending_approval = Some(PendingApprovalState {
                            tool_calls,
                            context,
                            session_id,
                        });
                        self.status_note = "awaiting tool approval".to_string();
                    } else {
                        self.execute_tool_calls(tool_calls, context, session_id);
                    }
                }
                AppEvent::ToolResultsReady {
                    session_id,
                    tool_calls,
                    tool_results,
                    context,
                } => {
                    if session_id != self.session_id {
                        continue;
                    }

                    for tr in &tool_results {
                        let _accent = if tr.success { TEAL } else { DANGER };
                        let label = format!("[TOOL:{}]", tr.name);
                        let summary = truncate(&tr.content, 200);
                        self.add_system_message(format!(
                            "{} {} -> {}",
                            label,
                            if tr.success { "ok" } else { "err" },
                            summary
                        ));

                        self.shell_output_history.push_front(
                            format!("{}:\n{}", tr.name, truncate(&tr.content, 500))
                        );
                        while self.shell_output_history.len() > 5 {
                            self.shell_output_history.pop_back();
                        }
                    }

                    self.tool_loop_depth += 1;
                    if self.tool_loop_depth > 10 {
                        self.pending_ai = false;
                        self.tool_loop_depth = 0;
                        self.add_system_message("tool loop depth limit reached (10). stopping agent.");
                        self.status_note = "tool loop halted".to_string();
                        continue;
                    }

                    let client = self.ai_client.clone();
                    let tx = self.events_tx.clone();
                    let tool_defs = tools::all_tool_definitions();

                    tokio::spawn(async move {
                        let result = client
                            .send_with_tool_results(context.clone(), &tool_calls, &tool_results, Some(&tool_defs))
                            .await;
                        match result {
                            Ok(AIResponse::Text(text)) => {
                                let _ = tx.send(AppEvent::AiFinished {
                                    session_id,
                                    result: Ok(text),
                                });
                            }
                            Ok(AIResponse::ToolCalls(new_calls, text)) => {
                                let _ = tx.send(AppEvent::AiToolCalls {
                                    session_id,
                                    tool_calls: new_calls,
                                    text,
                                    context,
                                });
                            }
                            Err(e) => {
                                let _ = tx.send(AppEvent::AiFinished {
                                    session_id,
                                    result: Err(e.to_string()),
                                });
                            }
                        }
                    });
                }
                AppEvent::StreamChunk { session_id, chunk } => {
                    if session_id != self.session_id {
                        continue;
                    }
                    match chunk {
                        StreamChunk::Delta(text) => {
                            self.stream_buffer.push_str(&text);
                            if let Some(idx) = self.stream_message_index {
                                if let Some(msg) = self.messages.get_mut(idx) {
                                    msg.content = self.stream_buffer.clone();
                                }
                            }
                            self.follow_tail = true;
                        }
                        StreamChunk::ToolCallsReceived(tool_calls, _text) => {
                            self.streaming_active = false;
                            if let Some(idx) = self.stream_message_index {
                                if let Some(msg) = self.messages.get_mut(idx) {
                                    msg.content = self.stream_buffer.clone();
                                }
                            }
                            if self.stream_message_index.is_some() && !self.stream_buffer.is_empty() {
                                self.persist(&self.provider, "assistant", "chat", &self.stream_buffer);
                            }
                            self.stream_buffer.clear();
                            self.stream_message_index = None;

                            let context = self.build_context();
                            let needs_approval = match self.trust_level {
                                TrustLevel::FullAuto => false,
                                TrustLevel::ConfirmAll => true,
                                TrustLevel::ConfirmDestructive => tool_calls
                                    .iter()
                                    .any(|tc| tools::is_destructive(&tc.name, &tc.arguments)),
                            };

                            if needs_approval {
                                let summary: Vec<String> = tool_calls
                                    .iter()
                                    .map(|tc| format!("{}({})", tc.name, truncate(&tc.arguments.to_string(), 60)))
                                    .collect();
                                self.add_system_message(format!(
                                    "APPROVAL REQUIRED: agent wants to execute:\n  {}\nPress Enter to approve, Esc to reject",
                                    summary.join("\n  ")
                                ));
                                self.pending_approval = Some(PendingApprovalState {
                                    tool_calls,
                                    context,
                                    session_id,
                                });
                                self.status_note = "awaiting tool approval".to_string();
                            } else {
                                self.execute_tool_calls(tool_calls, context, session_id);
                            }
                        }
                        StreamChunk::Done => {
                            self.streaming_active = false;
                            self.pending_ai = false;
                            if let Some(idx) = self.stream_message_index {
                                if let Some(msg) = self.messages.get_mut(idx) {
                                    msg.content = self.stream_buffer.clone();
                                }
                            }
                            if self.stream_message_index.is_some() {
                                let final_text = self.stream_buffer.clone();
                                self.persist(&self.provider, "assistant", "chat", &final_text);
                            }
                            self.stream_buffer.clear();
                            self.stream_message_index = None;
                            self.status_note = format!("{} stream complete", self.provider.badge());
                        }
                    }
                }
                AppEvent::PendingApproval {
                    session_id,
                    tool_calls,
                    context,
                } => {
                    if session_id != self.session_id {
                        continue;
                    }
                    self.pending_approval = Some(PendingApprovalState {
                        tool_calls,
                        context,
                        session_id,
                    });
                }
                AppEvent::ShellFinished { outcome } => {
                    self.pending_shells = self.pending_shells.saturating_sub(1);
                    let success = outcome.exit_code.unwrap_or(1) == 0 && !outcome.timed_out;
                    let accent = if success { TEAL } else { DANGER };
                    let text = format_outcome(&outcome, 4200);
                    let index = self.messages.len();
                    self.messages.push(ChatMessage::shell(accent));
                    self.persist(&self.provider, "user", "shell", &text);
                    self.reveal_queue.push_back(RevealJob::new(index, text.clone(), 18));

                    self.shell_output_history.push_front(
                        format!("$ {}\n{}", outcome.command, truncate(&text, 500))
                    );
                    while self.shell_output_history.len() > 5 {
                        self.shell_output_history.pop_back();
                    }

                    self.last_shell_status = if success {
                        format!(
                            "{} ok ({:.2}s)",
                            outcome
                                .exit_code
                                .map(|code| format!("exit {}", code))
                                .unwrap_or_else(|| "exit ?".to_string()),
                            outcome.duration.as_secs_f32()
                        )
                    } else if outcome.timed_out {
                        "shell timeout after 90s".to_string()
                    } else {
                        format!(
                            "{} fail ({:.2}s)",
                            outcome
                                .exit_code
                                .map(|code| format!("exit {}", code))
                                .unwrap_or_else(|| "exit ?".to_string()),
                            outcome.duration.as_secs_f32()
                        )
                    };
                    self.status_note = format!(
                        "ops payload returned for `{}`",
                        truncate(&outcome.command, 26)
                    );
                    self.follow_tail = true;
                }
            }
        }

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        let tick_factor = ((elapsed.as_secs_f32() / 0.016).ceil() as usize).max(1);

        if let Some(job) = self.reveal_queue.front_mut() {
            job.revealed = (job.revealed + job.speed * tick_factor).min(job.full_text.len());
            if let Some(message) = self.messages.get_mut(job.message_index) {
                message.content = job.full_text.iter().take(job.revealed).collect();
            }
            if job.revealed >= job.full_text.len() {
                self.reveal_queue.pop_front();
            }
        }
    }

    fn handle_input(&mut self) -> Result<bool> {
        while event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        self.mode = AppMode::Exit;
                        return Ok(true);
                    }

                    match self.mode {
                        AppMode::Intro => {
                            if matches!(
                                key.code,
                                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Esc
                            ) {
                                self.mode = AppMode::Chat;
                                self.status_note = "intro skipped to command deck".to_string();
                            }
                            if matches!(key.code, KeyCode::Char('q')) {
                                self.mode = AppMode::Exit;
                                return Ok(true);
                            }
                        }
                        AppMode::Chat => {
                            if self.handle_chat_key(key)? {
                                return Ok(true);
                            }
                        }
                        AppMode::Exit => return Ok(true),
                    }
                }
                Event::Resize(_, _) => {
                    self.follow_tail = true;
                }
                _ => {}
            }
        }
        Ok(false)
    }

    fn handle_chat_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('l') {
            self.messages.clear();
            self.reveal_queue.clear();
            self.status_note = "transcript purged".to_string();
            return Ok(false);
        }

        match key.code {
            KeyCode::Esc => {
                if self.pending_approval.is_some() {
                    self.reject_pending();
                } else if !self.input.is_empty() {
                    self.input.clear();
                    self.status_note = "input cleared".to_string();
                } else if self.last_esc_time.elapsed() < Duration::from_millis(500) {
                    self.mode = AppMode::Exit;
                    return Ok(true);
                } else {
                    self.last_esc_time = Instant::now();
                    self.status_note = "press Esc again to exit (or Ctrl+C)".to_string();
                }
            }
            KeyCode::F(1) => {
                self.show_help = !self.show_help;
            }
            KeyCode::F(2) => {
                self.session_id = self.session_id.wrapping_add(1);
                self.pending_ai = false;
                self.provider = self.provider.cycle();
                self.ai_client = AIClient::new(self.provider.clone());
                self.add_system_message(format!("uplink rerouted -> {}", self.provider.name()));
                self.status_note = format!("active provider: {}", self.provider.badge());
            }
            KeyCode::F(3) => {
                self.video_enabled = !self.video_enabled;
                self.status_note = if self.video_enabled {
                    "video bus online".to_string()
                } else {
                    "video bus muted".to_string()
                };
            }
            KeyCode::F(4) => {
                self.effects.active = !self.effects.active;
                self.status_note = if self.effects.active {
                    format!("3D fx online: {}", self.effects.kind.name())
                } else {
                    "3D fx offline".to_string()
                };
            }
            KeyCode::F(5) => {
                if self.webcam.is_some() {
                    self.webcam = None;
                    self.webcam_frame = None;
                    self.status_note = "webcam offline".to_string();
                } else {
                    let config = self.webcam_config();
                    match WebcamCapture::start(config) {
                        Ok(cam) => {
                            self.webcam = Some(cam);
                            self.status_note = "webcam online: live ascii feed".to_string();
                        }
                        Err(e) => {
                            self.add_system_message(format!("webcam error: {}", e));
                            self.status_note = "webcam failed to start".to_string();
                        }
                    }
                }
            }
            KeyCode::F(6) => {
                let preset = self.tiling.preset.cycle();
                self.tiling.apply_preset(preset);
                self.status_note = format!("layout: {}", preset.name());
            }
            KeyCode::F(7) => {
                self.effects.cycle();
                if self.effects.active {
                    self.status_note = format!("3D fx: {}", self.effects.kind.name());
                } else {
                    self.effects.active = true;
                    self.status_note = format!("3D fx online: {}", self.effects.kind.name());
                }
            }
            KeyCode::F(8) => {
                self.tiling.cycle_focused_panel();
                if let Some(p) = self.tiling.focused_panel() {
                    self.status_note = format!("tile -> {}", p.name());
                }
            }
            KeyCode::PageUp => {
                self.follow_tail = false;
                self.scroll_lines = self.scroll_lines.saturating_sub(8);
            }
            KeyCode::PageDown => {
                self.scroll_lines += 8;
            }
            KeyCode::Up => {
                self.follow_tail = false;
                self.scroll_lines = self.scroll_lines.saturating_sub(1);
            }
            KeyCode::Down => {
                self.scroll_lines += 1;
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                if self.pending_approval.is_some() && self.input.is_empty() {
                    self.approve_pending();
                } else {
                    let input = self.input.trim().to_string();
                    self.input.clear();
                    if !input.is_empty() {
                        self.dispatch_input(input);
                    }
                }
            }
            KeyCode::Tab => {
                self.input.push_str("    ");
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    let area = self.body_area;
                    match c {
                        'h' => self.tiling.focus_direction(area, -1, 0),
                        'l' => self.tiling.focus_direction(area, 1, 0),
                        'k' => self.tiling.focus_direction(area, 0, -1),
                        'j' => self.tiling.focus_direction(area, 0, 1),
                        'H' => self.tiling.swap_focused_with_direction(area, -1, 0),
                        'L' => self.tiling.swap_focused_with_direction(area, 1, 0),
                        'K' => self.tiling.swap_focused_with_direction(area, 0, -1),
                        'J' => self.tiling.swap_focused_with_direction(area, 0, 1),
                        'n' => {
                            self.tiling.cycle_focused_panel();
                            if let Some(p) = self.tiling.focused_panel() {
                                self.status_note = format!("tile -> {}", p.name());
                            }
                        }
                        '[' => self.tiling.resize_focused(-0.05),
                        ']' => self.tiling.resize_focused(0.05),
                        _ => {}
                    }
                    if matches!(c, 'h' | 'l' | 'k' | 'j') {
                        if let Some(p) = self.tiling.focused_panel() {
                            self.status_note = format!("focus: {}", p.name());
                        }
                    }
                } else if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.input.push(c);
                }
            }
            _ => {}
        }

        Ok(false)
    }

    fn dispatch_input(&mut self, input: String) {
        self.follow_tail = true;

        if input == "/help" {
            self.show_help = !self.show_help;
            return;
        }

        if input == "/clear" {
            self.messages.clear();
            self.reveal_queue.clear();
            self.status_note = "transcript purged".to_string();
            return;
        }

        if input == "/video" {
            self.video_enabled = !self.video_enabled;
            self.status_note = if self.video_enabled {
                "video bus online".to_string()
            } else {
                "video bus muted".to_string()
            };
            return;
        }

        if input == "/webcam" {
            if self.webcam.is_some() {
                self.webcam = None;
                self.webcam_frame = None;
                self.add_system_message("webcam offline");
            } else {
                let config = self.webcam_config();
                match WebcamCapture::start(config) {
                    Ok(cam) => {
                        self.webcam = Some(cam);
                        self.add_system_message("webcam online: live ascii feed active");
                    }
                    Err(e) => self.add_system_message(format!("webcam error: {}", e)),
                }
            }
            return;
        }

        if input == "/3d" || input == "/effects" {
            self.effects.active = !self.effects.active;
            self.status_note = if self.effects.active {
                format!("3D fx: {}", self.effects.kind.name())
            } else {
                "3D fx offline".to_string()
            };
            return;
        }

        if input == "/fx" {
            self.effects.cycle();
            self.effects.active = true;
            self.add_system_message(format!("3D effect: {}", self.effects.kind.name()));
            return;
        }

        if input == "/analytics" {
            self.analytics.active = !self.analytics.active;
            if self.analytics.active {
                self.analytics.refresh(self.db.as_ref());
                self.tiling.set_focused_panel(PanelKind::Analytics);
            }
            return;
        }

        if input == "/layout" {
            let preset = self.tiling.preset.cycle();
            self.tiling.apply_preset(preset);
            self.add_system_message(format!("layout: {}", preset.name()));
            return;
        }

        if let Some(name) = input.strip_prefix("/layout ") {
            let preset = match name.trim().to_lowercase().as_str() {
                "default" => LayoutPreset::Default,
                "dual" => LayoutPreset::DualPane,
                "triple" => LayoutPreset::TripleColumn,
                "quad" => LayoutPreset::Quad,
                "webcam" | "cam" => LayoutPreset::WebcamFocus,
                "focus" | "full" => LayoutPreset::FullFocus,
                _ => {
                    self.add_system_message("layouts: default, dual, triple, quad, webcam, focus");
                    return;
                }
            };
            self.tiling.apply_preset(preset);
            self.add_system_message(format!("layout: {}", preset.name()));
            return;
        }

        if input == "/sysmon" {
            self.tiling.set_focused_panel(PanelKind::SystemMonitor);
            return;
        }

        if let Some(port_str) = input.strip_prefix("/server ") {
            if let Ok(port) = port_str.trim().parse::<u16>() {
                let addr = format!("0.0.0.0:{}", port);
                self.add_system_message(format!("starting video chat server on {}", addr));
                let server = Arc::new(VideoChatServer::new());
                let addr_clone = addr.clone();
                tokio::spawn(async move {
                    if let Err(e) = server.run(&addr_clone).await {
                        eprintln!("server error: {}", e);
                    }
                });
                self.status_note = format!("video chat server live on :{}", port);
            } else {
                self.add_system_message("usage: /server <port>");
            }
            return;
        }

        if let Some(url) = input.strip_prefix("/connect ") {
            let url = url.trim().to_string();
            let username = self.username.clone();
            let client = VideoChatClient::new(username.clone(), url.clone());
            self.add_system_message(format!("connecting to {} as {}", url, username));
            let status_arc = client.status.clone();
            self.video_chat = Some(client);
            // spawn connection using a fresh client that will manage its own Arc state
            tokio::spawn(async move {
                let temp_client = VideoChatClient::new(username, url);
                if let Err(e) = temp_client.connect().await {
                    *status_arc.write() = format!("connection failed: {}", e);
                }
            });
            self.tiling.set_focused_panel(PanelKind::VideoChatFeeds);
            self.status_note = "video chat connecting...".to_string();
            return;
        }

        if let Some(msg) = input.strip_prefix("/chat ") {
            if let Some(ref vc) = self.video_chat {
                vc.send_chat(msg.trim().to_string());
            } else {
                self.add_system_message("not connected to video chat. use /connect ws://<addr>");
            }
            return;
        }

        if let Some(name) = input.strip_prefix("/username ") {
            self.username = name.trim().to_string();
            self.add_system_message(format!("username set to: {}", self.username));
            return;
        }

        if let Some(provider_name) = input.strip_prefix("/provider ") {
            self.provider = AIProvider::from_input(provider_name);
            self.ai_client = AIClient::new(self.provider.clone());
            self.session_id = self.session_id.wrapping_add(1);
            self.pending_ai = false;
            self.add_system_message(format!("manual route -> {}", self.provider.name()));
            self.status_note = format!("active provider: {}", self.provider.badge());
            return;
        }

        // Phase 1 commands
        if input == "/trust" {
            self.trust_level = self.trust_level.cycle();
            self.add_system_message(format!("trust level: {}", self.trust_level.name()));
            self.status_note = format!("trust: {}", self.trust_level.name());
            return;
        }

        if let Some(rest) = input.strip_prefix("/remember ") {
            if let Some((key, value)) = rest.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                if let Some(ref db) = self.db {
                    match AgentMemory::remember(db, key, value, memory::MemoryKind::UserSet) {
                        Ok(_) => {
                            self.agent_memory.load(db);
                            self.add_system_message(format!("remembered: {} = {}", key, value));
                        }
                        Err(e) => self.add_system_message(format!("memory error: {}", e)),
                    }
                } else {
                    self.add_system_message("memory offline: database not available");
                }
            } else {
                self.add_system_message("usage: /remember key = value");
            }
            return;
        }

        if let Some(key) = input.strip_prefix("/forget ") {
            let key = key.trim();
            if let Some(ref db) = self.db {
                match AgentMemory::forget(db, key) {
                    Ok(true) => {
                        self.agent_memory.load(db);
                        self.add_system_message(format!("forgot: {}", key));
                    }
                    Ok(false) => self.add_system_message(format!("no memory found for: {}", key)),
                    Err(e) => self.add_system_message(format!("memory error: {}", e)),
                }
            }
            return;
        }

        if let Some(key) = input.strip_prefix("/recall ") {
            let key = key.trim();
            if let Some(ref db) = self.db {
                match AgentMemory::recall(db, key) {
                    Some(value) => self.add_system_message(format!("{} = {}", key, value)),
                    None => self.add_system_message(format!("no memory for: {}", key)),
                }
            }
            return;
        }

        if input == "/memory" {
            if let Some(ref db) = self.db {
                self.agent_memory.load(db);
            }
            let entries = self.agent_memory.all_entries();
            if entries.is_empty() {
                self.add_system_message("agent memory is empty. use /remember key = value");
            } else {
                let lines: Vec<String> = entries
                    .iter()
                    .map(|e| format!("  {} = {} [{}]", e.key, truncate(&e.value, 60), e.kind.as_str_pub()))
                    .collect();
                self.add_system_message(format!("agent memory ({} entries):\n{}", entries.len(), lines.join("\n")));
            }
            return;
        }

        if input == "/pin" {
            let last_idx = self.messages.len().saturating_sub(1);
            if !self.pinned_messages.contains(&last_idx) {
                self.pinned_messages.push(last_idx);
                self.add_system_message(format!("pinned message #{}", last_idx));
            } else {
                self.add_system_message("last message already pinned");
            }
            return;
        }

        if input == "/unpin" {
            if let Some(idx) = self.pinned_messages.pop() {
                self.add_system_message(format!("unpinned message #{}", idx));
            } else {
                self.add_system_message("no pinned messages");
            }
            return;
        }

        if input == "/stream" || input == "/streaming" {
            self.add_system_message("streaming is enabled for all non-tool-use prompts. responses appear character-by-character.");
            return;
        }

        if let Some(command) = parse_shell_command(&input) {
            self.start_shell(command.to_string());
            return;
        }

        self.start_ai(input);
    }

    fn start_ai(&mut self, input: String) {
        if self.pending_ai || !self.reveal_queue.is_empty() {
            self.add_system_message("output pipeline busy: wait for the current reveal to complete before sending a new model prompt");
            return;
        }

        let enriched_input = self.inject_file_references(&input);

        let message = ChatMessage::user(enriched_input.clone());
        self.persist(&self.provider, "user", "chat", &input);
        self.messages.push(message);
        self.pending_ai = true;
        self.streaming_active = true;
        self.tool_loop_depth = 0;
        self.status_note = format!("streaming -> {}", self.provider.badge());

        // Create the assistant message shell for streaming into
        let assistant_msg = ChatMessage::assistant(&self.provider);
        let msg_index = self.messages.len();
        self.messages.push(assistant_msg);
        self.stream_message_index = Some(msg_index);
        self.stream_buffer.clear();

        let session_id = self.session_id;
        let client = self.ai_client.clone();
        let tx = self.events_tx.clone();
        let context = self.build_context();
        let tool_defs = tools::all_tool_definitions();

        tokio::spawn(async move {
            let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();

            let stream_task = tokio::spawn(async move {
                client
                    .send_streaming_with_tools(context, Some(&tool_defs), chunk_tx)
                    .await
            });

            while let Some(chunk) = chunk_rx.recv().await {
                let _ = tx.send(AppEvent::StreamChunk {
                    session_id,
                    chunk,
                });
            }

            if let Ok(Err(e)) = stream_task.await {
                let _ = tx.send(AppEvent::AiFinished {
                    session_id,
                    result: Err(e.to_string()),
                });
            }
        });
    }

    fn start_shell(&mut self, command: String) {
        self.pending_shells += 1;
        self.status_note = format!("dispatching ops payload -> {}", truncate(&command, 28));
        self.last_shell_status = format!("running `{}`", truncate(&command, 26));
        self.recent_commands.push_front(command.clone());
        while self.recent_commands.len() > 5 {
            self.recent_commands.pop_back();
        }

        let tx = self.events_tx.clone();
        tokio::spawn(async move {
            let outcome = run_shell(command).await;
            let _ = tx.send(AppEvent::ShellFinished { outcome });
        });
    }

    fn webcam_config(&self) -> webcam::WebcamConfig {
        let w = if self.body_area.width > 10 {
            (self.body_area.width / 2).max(80).min(220)
        } else {
            160
        };
        let h = if self.body_area.height > 6 {
            (self.body_area.height).max(24).min(70)
        } else {
            48
        };
        webcam::WebcamConfig {
            width: w,
            height: h,
            ..webcam::WebcamConfig::default()
        }
    }

    fn add_system_message(&mut self, content: impl Into<String>) {
        let message = ChatMessage::system(content);
        self.messages.push(message);
    }

    fn persist(&self, provider: &AIProvider, role: &str, kind: &str, content: &str) {
        if let Some(db) = &self.db {
            let _ = db.save_message(provider.db_key(), role, kind, content);
        }
    }

    fn execute_tool_calls(
        &mut self,
        tool_calls: Vec<ToolCall>,
        context: Vec<ApiMessage>,
        session_id: u64,
    ) {
        let call_names: Vec<String> = tool_calls
            .iter()
            .map(|tc| tc.name.clone())
            .collect();
        self.status_note = format!("agent executing: {}", call_names.join(", "));
        self.add_system_message(format!(
            "agent tool loop [{}/10]: executing {}",
            self.tool_loop_depth + 1,
            call_names.join(", ")
        ));

        let tx = self.events_tx.clone();
        let calls = tool_calls.clone();
        tokio::spawn(async move {
            let mut results = Vec::new();
            for call in &calls {
                let result = tools::execute_tool(call).await;
                results.push(result);
            }
            let _ = tx.send(AppEvent::ToolResultsReady {
                session_id,
                tool_calls: calls,
                tool_results: results,
                context,
            });
        });
    }

    fn approve_pending(&mut self) {
        if let Some(state) = self.pending_approval.take() {
            self.add_system_message("tool execution approved");
            self.execute_tool_calls(state.tool_calls, state.context, state.session_id);
        }
    }

    fn reject_pending(&mut self) {
        if let Some(_state) = self.pending_approval.take() {
            self.pending_ai = false;
            self.tool_loop_depth = 0;
            self.add_system_message("tool execution rejected by user");
            self.status_note = "tools rejected".to_string();
        }
    }

    fn build_context(&self) -> Vec<ApiMessage> {
        const MAX_CONTEXT_CHARS: usize = 30000;
        const RECENT_BUDGET_RATIO: f32 = 0.70;

        let mut preamble: Vec<ApiMessage> = Vec::new();

        // Inject agent memory as system context
        let memory_block = self.agent_memory.context_block();
        if !memory_block.is_empty() {
            preamble.push(ApiMessage {
                role: "user".to_string(),
                content: format!("[System context - agent memory]\n{}", memory_block),
            });
        }

        // Inject last 5 shell outputs as context
        if !self.shell_output_history.is_empty() {
            let shell_ctx: String = self
                .shell_output_history
                .iter()
                .take(5)
                .enumerate()
                .map(|(i, s)| format!("--- Recent output {} ---\n{}", i + 1, s))
                .collect::<Vec<_>>()
                .join("\n\n");
            preamble.push(ApiMessage {
                role: "user".to_string(),
                content: format!("[System context - recent command outputs]\n{}", shell_ctx),
            });
        }

        // Add pinned messages
        for &idx in &self.pinned_messages {
            if let Some(msg) = self.messages.get(idx) {
                if msg.include_in_context {
                    preamble.push(ApiMessage {
                        role: msg.context_role.to_string(),
                        content: format!("[Pinned] {}", msg.content),
                    });
                }
            }
        }

        // Collect conversation messages (non-pinned)
        let conversation: Vec<(String, String)> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(i, msg)| msg.include_in_context && !self.pinned_messages.contains(i))
            .map(|(_, msg)| (msg.context_role.to_string(), msg.content.clone()))
            .collect();

        let preamble_chars: usize = preamble.iter().map(|m| m.content.len()).sum();
        let conv_chars: usize = conversation.iter().map(|(_, c)| c.len()).sum();
        let total_chars = preamble_chars + conv_chars;

        let mut context_msgs = preamble;

        if total_chars <= MAX_CONTEXT_CHARS || conversation.len() <= 4 {
            for (role, content) in conversation {
                context_msgs.push(ApiMessage { role, content });
            }
        } else {
            // Summarize older messages, keep recent ones verbatim
            let budget_for_recent =
                ((MAX_CONTEXT_CHARS - preamble_chars) as f32 * RECENT_BUDGET_RATIO) as usize;

            // Find split point: keep as many recent messages as fit in budget
            let mut recent_chars = 0;
            let mut split = conversation.len();
            for (i, (_, content)) in conversation.iter().enumerate().rev() {
                if recent_chars + content.len() > budget_for_recent {
                    split = i + 1;
                    break;
                }
                recent_chars += content.len();
                if i == 0 {
                    split = 0;
                }
            }
            split = split.max(1);

            let old_messages = &conversation[..split];
            let recent_messages = &conversation[split..];

            // Build a condensed summary of older messages
            let mut summary_parts: Vec<String> = Vec::new();
            let summary_budget = MAX_CONTEXT_CHARS - preamble_chars - recent_chars;
            let per_msg_budget = if old_messages.is_empty() {
                0
            } else {
                (summary_budget / old_messages.len()).max(40).min(200)
            };

            for (role, content) in old_messages {
                let tag = if role == "user" { "User" } else { "Assistant" };
                let compressed = truncate(content, per_msg_budget);
                summary_parts.push(format!("- {}: {}", tag, compressed));
            }

            if !summary_parts.is_empty() {
                context_msgs.push(ApiMessage {
                    role: "user".to_string(),
                    content: format!(
                        "[Conversation summary - {} earlier messages compressed]\n{}",
                        old_messages.len(),
                        summary_parts.join("\n")
                    ),
                });
            }

            for (role, content) in recent_messages {
                context_msgs.push(ApiMessage {
                    role: role.clone(),
                    content: content.clone(),
                });
            }
        }

        context_msgs
    }

    fn inject_file_references(&self, input: &str) -> String {
        let mut result = input.to_string();
        let mut injections = Vec::new();

        let words: Vec<&str> = input.split_whitespace().collect();
        for word in &words {
            if let Some(path) = word.strip_prefix('@') {
                if !path.is_empty() {
                    match std::fs::read_to_string(path) {
                        Ok(content) => {
                            let truncated = if content.len() > 8000 {
                                format!("{}\n[truncated at 8000 chars]", &content[..8000])
                            } else {
                                content
                            };
                            injections.push(format!(
                                "\n\n[Contents of {}]\n```\n{}\n```\n",
                                path, truncated
                            ));
                        }
                        Err(e) => {
                            injections.push(format!("\n[Failed to read {}: {}]\n", path, e));
                        }
                    }
                }
            }
        }

        for injection in injections {
            result.push_str(&injection);
        }
        result
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let phase = self.intro_started.elapsed().as_secs_f32();
        render_background(frame.buffer_mut(), area, phase);

        match self.mode {
            AppMode::Intro => self.render_intro(frame, area, phase),
            AppMode::Chat => self.render_chat(frame, area, phase),
            AppMode::Exit => {}
        }
    }

    fn render_intro(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let outer = Block::default()
            .title(" [SYSTEM:DEM0ZONE v2.0] [MODE:AGENTIC] [MODULES:AI+VIDEO+WEBCAM+3D+ANALYTICS] ")
            .title_style(Style::default().fg(CYAN).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(COPPER));
        frame.render_widget(outer, area);

        let inner = area.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });

        render_raster_bars(frame.buffer_mut(), inner, phase);
        if let Some(video) = &self.video {
            let video_area = centered_area(inner, 82, 54);
            let shell = Block::default()
                .title(" LIVE FEED // DECOMPRESSING ")
                .title_style(Style::default().fg(AMBER).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(TEAL));
            frame.render_widget(shell, video_area);
            video.render(
                frame,
                video_area.inner(Margin {
                    horizontal: 1,
                    vertical: 1,
                }),
                0.95,
            );
        }

        let burst_x = inner.x + inner.width.saturating_mul(22) / 100;
        let burst_y = inner.y + inner.height.saturating_mul(34) / 100;
        render_starburst(frame.buffer_mut(), burst_x, burst_y, 12, phase);

        let logo = if inner.width > 108 {
            LARGE_LOGO
        } else {
            SMALL_LOGO
        };
        let logo_width = logo
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0) as u16;
        let logo_x = inner.x + inner.width.saturating_sub(logo_width) / 2;
        let logo_y = inner.y + inner.height.saturating_mul(16) / 100;
        render_logo(frame.buffer_mut(), logo_x, logo_y, logo, phase);

        let info = vec![
            Line::from(vec![
                Span::styled("v2.0.0", Style::default().fg(MUTED)),
                Span::styled("  (POWERHOUSE)  ", Style::default().fg(ICE)),
                Span::styled("//", Style::default().fg(COPPER)),
                Span::styled(
                    "  ALL-IN-ONE TERMINAL COMMAND CENTER",
                    Style::default().fg(CYAN).bold(),
                ),
            ]),
            Line::from(vec![
                Span::styled("STACK:", Style::default().fg(AMBER).bold()),
                Span::styled(
                    " ASCII VIDEO | MULTI-AI | LIVE BASH | WEBCAM | 3D FX | VIDEO CHAT | ANALYTICS",
                    Style::default().fg(ICE),
                ),
            ]),
            Line::from(vec![
                Span::styled("STATE:", Style::default().fg(AMBER).bold()),
                Span::styled(
                    " cracktro boot stream -> auto-transitions into the full command deck",
                    Style::default().fg(ICE),
                ),
            ]),
            Line::from(vec![
                Span::styled("INPUT:", Style::default().fg(AMBER).bold()),
                Span::styled(
                    " ENTER / SPACE skips intro immediately",
                    Style::default().fg(ICE),
                ),
            ]),
        ];
        let info_area = Rect {
            x: inner.x + 4,
            y: inner.y + inner.height.saturating_sub(8),
            width: inner.width.saturating_sub(8),
            height: 5,
        };
        let info_block = Block::default()
            .borders(Borders::ALL)
            .title(" BOOT NOTE ")
            .border_style(Style::default().fg(Color::Rgb(115, 146, 159)));
        frame.render_widget(info_block, info_area);
        frame.render_widget(
            Paragraph::new(Text::from(info))
                .wrap(Wrap { trim: false })
                .style(Style::default().bg(PANEL_BG)),
            info_area.inner(Margin {
                horizontal: 1,
                vertical: 1,
            }),
        );

        let scroller_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        render_scroller(
            frame.buffer_mut(),
            scroller_area,
            SCROLLER_TEXT,
            phase,
            CYAN,
        );
    }

    fn render_chat(&mut self, frame: &mut Frame, area: Rect, phase: f32) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),
                Constraint::Min(14),
                Constraint::Length(5),
                Constraint::Length(1),
            ])
            .split(area);

        let body_area = layout[1];
        self.body_area = body_area;

        self.render_header(frame, layout[0], phase);

        // tiling-based panel rendering
        let tiles = self.tiling.layout(body_area);
        let focused_id = self.tiling.focused;
        for (id, panel, rect) in &tiles {
            let is_focused = *id == focused_id;
            self.render_tile_panel(frame, *panel, *rect, phase, is_focused);
        }

        // 3D effects overlay on the full body area
        if self.effects.active {
            self.effects.render(frame.buffer_mut(), body_area, phase);
        }

        self.render_input(frame, layout[2]);
        render_scroller(frame.buffer_mut(), layout[3], SCROLLER_TEXT, phase, COPPER);

        if self.show_help {
            self.render_help_overlay(frame, area);
        }
    }

    fn render_tile_panel(
        &mut self,
        frame: &mut Frame,
        panel: PanelKind,
        area: Rect,
        phase: f32,
        is_focused: bool,
    ) {
        if area.width < 6 || area.height < 4 {
            return;
        }
        match panel {
            PanelKind::Transcript => self.render_messages_tile(frame, area, is_focused),
            PanelKind::Video => self.render_video_panel(frame, area, phase),
            PanelKind::Webcam => self.render_webcam_panel(frame, area, phase),
            PanelKind::Telemetry => self.render_telemetry(frame, area, phase),
            PanelKind::OpsDeck => self.render_ops_panel(frame, area, phase),
            PanelKind::Effects3D => {
                let block = Block::default()
                    .title(" 3D EFFECTS ")
                    .title_style(Style::default().fg(AMBER).bold())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if is_focused { CYAN } else { COPPER }));
                frame.render_widget(block, area);
                let inner = area.inner(Margin { horizontal: 1, vertical: 1 });
                let mut standalone = EffectsEngine::new();
                standalone.active = true;
                standalone.kind = self.effects.kind;
                standalone.render(frame.buffer_mut(), inner, phase);
            }
            PanelKind::Analytics => {
                self.analytics.refresh(self.db.as_ref());
                self.analytics.render(frame, area, phase);
            }
            PanelKind::VideoChatFeeds => self.render_videochat_feeds(frame, area, phase),
            PanelKind::VideoChatMessages => self.render_videochat_messages(frame, area),
            PanelKind::VideoChatUsers => self.render_videochat_users(frame, area, phase),
            PanelKind::SystemMonitor => {
                self.sysmon.render(frame, area, phase, is_focused);
            }
        }
    }

    fn render_messages_tile(&self, frame: &mut Frame, area: Rect, is_focused: bool) {
        let border_color = if is_focused { CYAN } else { TEAL };
        let block = Block::default()
            .title(" TRANSCRIPT ")
            .title_style(Style::default().fg(CYAN).bold())
            .borders(Borders::ALL)
            .border_type(if is_focused {
                BorderType::Double
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(border_color));
        frame.render_widget(block, area);

        let inner = area.inner(Margin { horizontal: 1, vertical: 1 });
        self.render_messages_inner(frame, inner);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let block = Block::default()
            .title(" COMMAND DECK ")
            .title_style(Style::default().fg(COPPER).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(COPPER));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        render_starburst(
            frame.buffer_mut(),
            inner.x + 4,
            inner.y + 1,
            3,
            phase * 1.2 + 1.0,
        );
        render_gradient_text(
            frame.buffer_mut(),
            inner.x + 10,
            inner.y,
            "ASCIIVISION v2 // ALL-IN-ONE TERMINAL POWERHOUSE",
            COPPER,
            CYAN,
        );

        let fx_tag = if self.effects.active {
            format!("fx:{}", self.effects.kind.name())
        } else {
            "fx:off".to_string()
        };
        let cam_tag = if self.webcam.is_some() { "cam:on" } else { "cam:off" };
        let vc_tag = if self.video_chat.as_ref().map_or(false, |c| c.is_connected()) {
            "vc:live"
        } else {
            "vc:off"
        };

        let focused_tag = self
            .tiling
            .focused_panel()
            .map(|p| p.name())
            .unwrap_or("?");

        let meta = format!(
            "{} // {} {} {} {} // layout:{} focus:{} // ai:{} shell:{}",
            self.provider.badge(),
            if self.video_enabled { "vid:on" } else { "vid:off" },
            cam_tag,
            fx_tag,
            vc_tag,
            self.tiling.preset.name(),
            focused_tag,
            if self.pending_ai { "live" } else { "idle" },
            self.pending_shells,
        );
        render_gradient_text(
            frame.buffer_mut(),
            inner.x + 10,
            inner.y + 1,
            &meta,
            ICE,
            self.provider.color(),
        );

        let status = truncate(&self.status_note, inner.width.saturating_sub(24) as usize);
        let badge = format!("[{}]", current_spinner(phase));
        let x = area.x + area.width.saturating_sub(status.chars().count() as u16 + 6);
        render_gradient_text(
            frame.buffer_mut(),
            x,
            inner.y,
            &format!("{} {}", badge, status),
            AMBER,
            ICE,
        );
    }

    fn render_messages_inner(&self, frame: &mut Frame, inner: Rect) {
        let mut lines = Vec::new();
        for message in &self.messages {
            let tag = match message.kind {
                MessageKind::User => "USER",
                MessageKind::Assistant => "AI",
                MessageKind::Shell => "OPS",
                MessageKind::System => "SYS",
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", message.label),
                    Style::default().fg(message.accent).bold(),
                ),
                Span::styled(format!("[{}]", tag), Style::default().fg(AMBER)),
            ]));

            if message.content.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  . . .",
                    Style::default().fg(MUTED),
                )));
            } else {
                for content_line in message.content.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", content_line),
                        Style::default().fg(match message.kind {
                            MessageKind::System => Color::Rgb(171, 183, 192),
                            _ => ICE,
                        }),
                    )));
                }
            }
            lines.push(Line::from(""));
        }

        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "No traffic yet. Ask the model something, !shell, /webcam, /3d, /server, or /connect.",
                Style::default().fg(MUTED),
            )));
        }

        let total_lines = lines.len().max(1);
        let visible_lines = inner.height.saturating_sub(1) as usize;
        let max_scroll = total_lines.saturating_sub(visible_lines);
        let scroll = if self.follow_tail {
            max_scroll
        } else {
            self.scroll_lines.min(max_scroll)
        };

        let widget = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0))
            .style(Style::default().bg(PANEL_BG));
        frame.render_widget(widget, inner);
    }

    fn render_video_panel(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let title = if self.video_enabled {
            " LIVE VIDEO BUS "
        } else {
            " SYNTHETIC FIELD "
        };
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(AMBER).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COPPER));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if self.video_enabled {
            if let Some(video) = &self.video {
                video.render(frame, inner, 0.92);
                let meta = format!(
                    "sig:{}  provider:{}",
                    if video.has_signal() { "lock" } else { "seek" },
                    self.provider.badge()
                );
                render_gradient_text(frame.buffer_mut(), inner.x + 1, inner.y, &meta, CYAN, ICE);
                return;
            }
        }

        render_synthetic_scope(frame.buffer_mut(), inner, phase);
    }

    fn render_webcam_panel(&self, frame: &mut Frame, area: Rect, _phase: f32) {
        let title = if self.webcam.is_some() {
            " WEBCAM // LIVE ASCII "
        } else {
            " WEBCAM // OFFLINE "
        };
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(CYAN).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TEAL));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if let Some(ref ascii) = self.webcam_frame {
            render_ascii_frame(frame.buffer_mut(), inner, ascii, 0.9);
        } else {
            let msg = if let Some(ref cam) = self.webcam {
                if let Some(err) = cam.error() {
                    format!("WEBCAM ERROR: {}\n\nIs another app using the camera?\n(OBS, FaceTime, Zoom, etc.)\n\nClose it and press F5 to retry.", err)
                } else {
                    "signal lock pending...".to_string()
                }
            } else {
                "F5 or /webcam to activate".to_string()
            };
            let color = if self.webcam.as_ref().and_then(|c| c.error()).is_some() {
                DANGER
            } else {
                MUTED
            };
            frame.render_widget(
                Paragraph::new(msg)
                    .style(Style::default().fg(color).bg(PANEL_BG))
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }
    }

    fn render_telemetry(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let block = Block::default()
            .title(" TELEMETRY ")
            .title_style(Style::default().fg(CYAN).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TEAL));
        frame.render_widget(block, area);
        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        let lines = vec![
            Line::from(vec![
                Span::styled("provider: ", Style::default().fg(AMBER).bold()),
                Span::styled(
                    self.provider.name(),
                    Style::default().fg(self.provider.color()),
                ),
            ]),
            Line::from(vec![
                Span::styled("status:   ", Style::default().fg(AMBER).bold()),
                Span::styled(
                    if self.pending_ai {
                        "awaiting model response"
                    } else {
                        "terminal steady"
                    },
                    Style::default().fg(ICE),
                ),
            ]),
            Line::from(vec![
                Span::styled("shell:    ", Style::default().fg(AMBER).bold()),
                Span::styled(&self.last_shell_status, Style::default().fg(ICE)),
            ]),
            Line::from(vec![
                Span::styled("3d fx:    ", Style::default().fg(AMBER).bold()),
                Span::styled(
                    if self.effects.active {
                        self.effects.kind.name()
                    } else {
                        "offline"
                    },
                    Style::default().fg(if self.effects.active { CYAN } else { MUTED }),
                ),
            ]),
            Line::from(vec![
                Span::styled("webcam:   ", Style::default().fg(AMBER).bold()),
                Span::styled(
                    if self.webcam.is_some() { "active" } else { "offline" },
                    Style::default().fg(if self.webcam.is_some() { TEAL } else { MUTED }),
                ),
            ]),
        ];

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().bg(PANEL_ALT))
                .wrap(Wrap { trim: false }),
            inner,
        );

        let eq_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(2),
            width: inner.width,
            height: 2,
        };
        render_equalizer(frame.buffer_mut(), eq_area, phase);
    }

    fn render_ops_panel(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let block = Block::default()
            .title(" OPS DECK ")
            .title_style(Style::default().fg(COPPER).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COPPER));
        frame.render_widget(block, area);
        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        let mut lines = vec![
            Line::from(Span::styled(
                "!<cmd>         raw shell",
                Style::default().fg(ICE),
            )),
            Line::from(Span::styled(
                "/server <port> host video chat",
                Style::default().fg(ICE),
            )),
            Line::from(Span::styled(
                "/connect <url> join video chat",
                Style::default().fg(ICE),
            )),
            Line::from(Span::styled(
                "/webcam /3d /fx /analytics",
                Style::default().fg(ICE),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "recent ops:",
                Style::default().fg(AMBER).bold(),
            )),
        ];

        if self.recent_commands.is_empty() {
            lines.push(Line::from(Span::styled(
                "  none yet",
                Style::default().fg(MUTED),
            )));
        } else {
            for command in &self.recent_commands {
                lines.push(Line::from(Span::styled(
                    format!(
                        "  {}",
                        truncate(command, inner.width.saturating_sub(4) as usize)
                    ),
                    Style::default().fg(CYAN),
                )));
            }
        }

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().bg(PANEL_BG))
                .wrap(Wrap { trim: false }),
            inner,
        );

        let pulse_x = inner.x + inner.width.saturating_sub(8);
        render_starburst(frame.buffer_mut(), pulse_x, inner.y + 1, 2, phase * 1.7);
    }

    fn render_videochat_feeds(&self, frame: &mut Frame, area: Rect, _phase: f32) {
        let block = Block::default()
            .title(" VIDEO CHAT // LIVE FEEDS ")
            .title_style(Style::default().fg(AMBER).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(COPPER));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if let Some(ref vc) = self.video_chat {
            let frames = vc.remote_frames.read();
            if frames.is_empty() {
                if let Some(ref local) = *vc.local_frame.read() {
                    render_ascii_frame(frame.buffer_mut(), inner, local, 0.9);
                    render_gradient_text(
                        frame.buffer_mut(),
                        inner.x + 1,
                        inner.y,
                        &format!("{} (you)", self.username),
                        CYAN,
                        ICE,
                    );
                } else {
                    frame.render_widget(
                        Paragraph::new("waiting for video feeds...")
                            .style(Style::default().fg(MUTED).bg(PANEL_BG))
                            .alignment(Alignment::Center),
                        inner,
                    );
                }
            } else {
                let count = frames.len().min(4);
                let cols = if count <= 2 { count } else { 2 };
                let rows = (count + cols - 1) / cols;

                let row_constraints: Vec<Constraint> = (0..rows)
                    .map(|_| Constraint::Percentage((100 / rows) as u16))
                    .collect();
                let row_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(row_constraints)
                    .split(inner);

                let mut frame_iter = frames.iter();
                for r in 0..rows {
                    let col_constraints: Vec<Constraint> = (0..cols)
                        .map(|_| Constraint::Percentage((100 / cols) as u16))
                        .collect();
                    let col_layout = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints(col_constraints)
                        .split(row_layout[r]);

                    for c in 0..cols {
                        if let Some((uname, ascii_frame)) = frame_iter.next() {
                            let cell_area = col_layout[c];
                            render_ascii_frame(frame.buffer_mut(), cell_area, ascii_frame, 0.85);
                            let is_self = uname == &self.username;
                            let label = if is_self {
                                format!("{} (you)", uname)
                            } else {
                                uname.clone()
                            };
                            render_gradient_text(
                                frame.buffer_mut(),
                                cell_area.x + 1,
                                cell_area.y,
                                &label,
                                if is_self { TEAL } else { CYAN },
                                ICE,
                            );
                        }
                    }
                }
            }
        } else {
            frame.render_widget(
                Paragraph::new("not connected. use /server <port> or /connect ws://<addr>")
                    .style(Style::default().fg(MUTED).bg(PANEL_BG))
                    .alignment(Alignment::Center),
                inner,
            );
        }
    }

    fn render_videochat_messages(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" CHAT STREAM ")
            .title_style(Style::default().fg(CYAN).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TEAL));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if let Some(ref vc) = self.video_chat {
            let msgs = vc.chat_messages.read();
            let visible = inner.height as usize;
            let start = msgs.len().saturating_sub(visible);
            let mut lines = Vec::new();
            for (uname, content) in msgs.iter().skip(start) {
                let is_sys = uname == "SYSTEM";
                let color = if is_sys {
                    MUTED
                } else if uname == &self.username {
                    TEAL
                } else {
                    CYAN
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", uname), Style::default().fg(color).bold()),
                    Span::styled(content.as_str(), Style::default().fg(ICE)),
                ]));
            }
            if lines.is_empty() {
                lines.push(Line::from(Span::styled(
                    "no messages yet. use /chat <msg>",
                    Style::default().fg(MUTED),
                )));
            }
            frame.render_widget(
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().bg(PANEL_BG)),
                inner,
            );
        } else {
            frame.render_widget(
                Paragraph::new("offline")
                    .style(Style::default().fg(MUTED).bg(PANEL_BG))
                    .alignment(Alignment::Center),
                inner,
            );
        }
    }

    fn render_videochat_users(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let block = Block::default()
            .title(" CONNECTED USERS ")
            .title_style(Style::default().fg(AMBER).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COPPER));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if let Some(ref vc) = self.video_chat {
            let users = vc.connected_users.read();
            let status = vc.get_status();
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("status: ", Style::default().fg(AMBER).bold()),
                    Span::styled(status, Style::default().fg(ICE)),
                ]),
            ];
            if users.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  no users yet",
                    Style::default().fg(MUTED),
                )));
            } else {
                for u in users.iter() {
                    let indicator = current_spinner(phase);
                    let is_self = u == &self.username;
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  [{}] ", indicator),
                            Style::default().fg(if is_self { TEAL } else { CYAN }),
                        ),
                        Span::styled(
                            u.as_str(),
                            Style::default()
                                .fg(if is_self { TEAL } else { ICE })
                                .bold(),
                        ),
                        if is_self {
                            Span::styled(" (you)", Style::default().fg(MUTED))
                        } else {
                            Span::raw("")
                        },
                    ]));
                }
            }
            frame.render_widget(
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().bg(PANEL_BG)),
                inner,
            );
        } else {
            frame.render_widget(
                Paragraph::new("offline")
                    .style(Style::default().fg(MUTED).bg(PANEL_BG))
                    .alignment(Alignment::Center),
                inner,
            );
        }
    }

    fn render_input(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" TRANSMIT ")
            .title_style(Style::default().fg(CYAN).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TEAL))
            .border_type(BorderType::Double);
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        let status = if self.pending_approval.is_some() {
            "APPROVAL PENDING [Enter=approve Esc=reject]"
        } else if self.pending_ai {
            if self.tool_loop_depth > 0 {
                "AGENT TOOL LOOP"
            } else {
                "MODEL LINK BUSY"
            }
        } else if self.streaming_active {
            "STREAMING"
        } else if self.pending_shells > 0 {
            "OPS EXECUTING"
        } else if self.video_chat.as_ref().map_or(false, |c| c.is_connected()) {
            "READY // VC LIVE"
        } else {
            "READY"
        };
        let status_color = if self.pending_approval.is_some() {
            AMBER
        } else {
            self.provider.color()
        };
        let trust_tag = format!("trust:{}", match self.trust_level {
            TrustLevel::FullAuto => "auto",
            TrustLevel::ConfirmDestructive => "safe",
            TrustLevel::ConfirmAll => "ask",
        });
        let lines = vec![
            Line::from(vec![
                Span::styled("> ", Style::default().fg(CYAN).bold()),
                Span::styled(
                    if self.input.is_empty() {
                        "prompt, !bash, @file, /trust, /remember, /pin, /webcam, /3d ..."
                    } else {
                        self.input.as_str()
                    },
                    Style::default().fg(ICE),
                ),
                Span::styled("_", Style::default().fg(AMBER).bold()),
            ]),
            Line::from(vec![
                Span::styled("mode: ", Style::default().fg(AMBER).bold()),
                Span::styled(status, Style::default().fg(status_color)),
                Span::styled(
                    format!("  |  {}  |  F1 help  F2 ai  F4 3D  F5 cam  F6 layout  Ctrl+hjkl tile", trust_tag),
                    Style::default().fg(MUTED),
                ),
            ]),
        ];

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().bg(PANEL_ALT))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn render_help_overlay(&self, frame: &mut Frame, area: Rect) {
        let popup = centered_area(area, 78, 75);
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .title(" HELP // ASCIIVISION v2 OPERATIONS MANUAL ")
            .title_style(Style::default().fg(COPPER).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(COPPER));
        frame.render_widget(block, popup);

        let text = Text::from(vec![
            Line::from(vec![
                Span::styled("PROMPTS    ", Style::default().fg(AMBER).bold()),
                Span::styled("plain text goes to the active AI provider", Style::default().fg(ICE)),
            ]),
            Line::from(vec![
                Span::styled("SHELL      ", Style::default().fg(AMBER).bold()),
                Span::styled("prefix with ! to execute locally: !ls, !git status, !curl ...", Style::default().fg(ICE)),
            ]),
            Line::from(vec![
                Span::styled("VIDEO CHAT ", Style::default().fg(AMBER).bold()),
                Span::styled("/server <port>, /connect ws://<addr>, /chat <msg>", Style::default().fg(ICE)),
            ]),
            Line::from(vec![
                Span::styled("WEBCAM     ", Style::default().fg(AMBER).bold()),
                Span::styled("/webcam or F5 to toggle live ASCII webcam feed", Style::default().fg(ICE)),
            ]),
            Line::from(vec![
                Span::styled("3D EFFECTS ", Style::default().fg(AMBER).bold()),
                Span::styled("/3d, /fx, F4 toggle, F7 cycle (matrix, plasma, starfield, wireframe, fire, particles)", Style::default().fg(ICE)),
            ]),
            Line::from(vec![
                Span::styled("ANALYTICS  ", Style::default().fg(AMBER).bold()),
                Span::styled("/analytics or F6 for live conversation stats dashboard", Style::default().fg(ICE)),
            ]),
            Line::from(vec![
                Span::styled("SHORTCUTS  ", Style::default().fg(AMBER).bold()),
                Span::styled("/curl, /brew, /provider, /video, /clear, /help, /username", Style::default().fg(ICE)),
            ]),
            Line::from(""),
            Line::from(Span::styled("Keyboard", Style::default().fg(CYAN).bold())),
            Line::from("  F1       toggle this overlay"),
            Line::from("  F2       cycle AI provider (Claude, Grok, GPT-5, Gemini)"),
            Line::from("  F3       toggle live video panel"),
            Line::from("  F4       toggle 3D effects overlay"),
            Line::from("  F5       toggle webcam capture"),
            Line::from("  F6       cycle tiling layout preset"),
            Line::from("  F7       cycle 3D effect type"),
            Line::from("  F8       cycle focused tile panel type"),
            Line::from("  Ctrl+L   clear transcript"),
            Line::from("  PgUp/Dn  scroll transcript"),
            Line::from("  Esc      exit"),
            Line::from(""),
            Line::from(Span::styled("Tiling (Hyprland-style)", Style::default().fg(CYAN).bold())),
            Line::from("  Ctrl+h/l  focus tile left/right"),
            Line::from("  Ctrl+j/k  focus tile down/up"),
            Line::from("  Ctrl+H/L  swap tile left/right (shift)"),
            Line::from("  Ctrl+J/K  swap tile down/up (shift)"),
            Line::from("  Ctrl+[/]  resize focused split narrower/wider"),
            Line::from("  Ctrl+n    cycle focused tile to next panel type"),
            Line::from("  /layout  cycle layout (default, dual, triple, quad, webcam, focus)"),
            Line::from(""),
            Line::from(Span::styled("Modules", Style::default().fg(CYAN).bold())),
            Line::from("  AI Chat: Claude 4.5, Grok 4, GPT-5, Gemini 2.5 Pro"),
            Line::from("  Video: MP4 ASCII playback | Webcam: Live camera ASCII feed"),
            Line::from("  Video Chat: WebSocket multi-user streaming"),
            Line::from("  3D FX: matrix, plasma, starfield, wireframe, fire, particles"),
            Line::from("  Sys Monitor: CPU, memory, swap, network, load average"),
            Line::from("  Analytics: Real-time conversation statistics"),
            Line::from("  Shell: Full bash, curl, brew integration"),
        ]);

        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(ICE).bg(PANEL_BG)),
            popup.inner(Margin {
                horizontal: 2,
                vertical: 1,
            }),
        );
    }
}

fn parse_shell_command(input: &str) -> Option<&str> {
    if let Some(rest) = input.strip_prefix('!') {
        let command = rest.trim();
        if command.is_empty() {
            None
        } else {
            Some(command)
        }
    } else if let Some(rest) = input.strip_prefix("/run ") {
        Some(rest.trim())
    } else if let Some(rest) = input.strip_prefix("/bash ") {
        Some(rest.trim())
    } else if let Some(rest) = input.strip_prefix("/curl ") {
        Some(input.strip_prefix("/").unwrap_or(rest).trim())
    } else if let Some(rest) = input.strip_prefix("/brew ") {
        Some(input.strip_prefix("/").unwrap_or(rest).trim())
    } else {
        None
    }
}

fn resolve_video_path(background: Option<String>, intro: Option<String>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = background {
        candidates.push(PathBuf::from(path));
    }
    if let Some(path) = intro {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(PathBuf::from("demo-videos/demo.mp4"));

    candidates.into_iter().find(|path| Path::new(path).exists())
}

fn render_ascii_frame(buffer: &mut Buffer, area: Rect, ascii: &video::AsciiFrame, intensity: f32) {
    if area.width == 0 || area.height == 0 || ascii.width == 0 || ascii.height == 0 {
        return;
    }

    // If source matches destination closely, render directly (fast path)
    let needs_scaling =
        ascii.width.abs_diff(area.width) > 2 || ascii.height.abs_diff(area.height) > 2;

    if !needs_scaling {
        let content_width = std::cmp::min(ascii.width, area.width);
        let content_height = std::cmp::min(ascii.height, area.height);
        let offset_x = area.x + (area.width - content_width) / 2;
        let offset_y = area.y + (area.height - content_height) / 2;

        for y in 0..content_height {
            for x in 0..content_width {
                let index = y as usize * ascii.width as usize + x as usize;
                if index >= ascii.cells.len() {
                    break;
                }
                let (glyph, r, g, b) = ascii.cells[index];
                let scanline = if y % 2 == 0 { 0.84 } else { 1.0 };
                let factor = (intensity * scanline).clamp(0.1, 1.2);
                let fg = scale_rgb(r, g, b, factor);
                let bg = scale_rgb(r, g, b, factor * 0.16);

                if let Some(cell) = buffer.cell_mut((offset_x + x, offset_y + y)) {
                    cell.set_char(glyph);
                    cell.set_fg(fg);
                    cell.set_bg(bg);
                }
            }
        }
        return;
    }

    // Scaled path: nearest-neighbor sampling with aspect-ratio-preserving letterbox
    let src_ratio = ascii.width as f32 / ascii.height as f32;
    let dst_ratio = area.width as f32 / area.height as f32;

    let (fit_w, fit_h) = if src_ratio > dst_ratio {
        let h = (area.width as f32 / src_ratio).round() as u16;
        (area.width, h.max(1).min(area.height))
    } else {
        let w = (area.height as f32 * src_ratio).round() as u16;
        (w.max(1).min(area.width), area.height)
    };

    let offset_x = area.x + (area.width.saturating_sub(fit_w)) / 2;
    let offset_y = area.y + (area.height.saturating_sub(fit_h)) / 2;

    for y in 0..fit_h {
        let src_y = ((y as f32 * ascii.height as f32 / fit_h as f32) as usize)
            .min(ascii.height as usize - 1);
        for x in 0..fit_w {
            let src_x = ((x as f32 * ascii.width as f32 / fit_w as f32) as usize)
                .min(ascii.width as usize - 1);
            let index = src_y * ascii.width as usize + src_x;
            if index >= ascii.cells.len() {
                continue;
            }
            let (glyph, r, g, b) = ascii.cells[index];
            let scanline = if y % 2 == 0 { 0.84 } else { 1.0 };
            let factor = (intensity * scanline).clamp(0.1, 1.2);
            let fg = scale_rgb(r, g, b, factor);
            let bg = scale_rgb(r, g, b, factor * 0.16);

            if let Some(cell) = buffer.cell_mut((offset_x + x, offset_y + y)) {
                cell.set_char(glyph);
                cell.set_fg(fg);
                cell.set_bg(bg);
            }
        }
    }
}

fn render_background(buffer: &mut Buffer, area: Rect, phase: f32) {
    for y in area.y..area.y + area.height {
        let band = (((y as f32 * 0.23) + phase * 1.6).sin() * 0.5 + 0.5) * 0.26;
        for x in area.x..area.x + area.width {
            let noise = hash32(x, y, (phase * 33.0) as u32);
            let base = mix_color(BG_BASE, BG_ALT, band + ((noise & 0x07) as f32 / 90.0));
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_bg(base);
                cell.set_fg(mix_color(MUTED, ICE, ((noise & 0x0f) as f32) / 42.0));

                let sparkle = noise % 173 == 0;
                let embers = y % 3 == 0 && noise % 97 == 0;
                if sparkle {
                    cell.set_char(if noise % 2 == 0 { '.' } else { '*' });
                } else if embers {
                    cell.set_char(if noise % 3 == 0 { '\'' } else { '`' });
                    cell.set_fg(mix_color(COPPER, CYAN, ((x + y) % 9) as f32 / 9.0));
                } else {
                    cell.set_char(' ');
                }
            }
        }
    }
}

fn render_raster_bars(buffer: &mut Buffer, area: Rect, phase: f32) {
    for band in 0..4 {
        let y = area.y
            + ((area.height as f32 * (0.18 + band as f32 * 0.16))
                + (phase * (1.7 + band as f32 * 0.2)).sin() * 2.5)
                .max(0.0) as u16;
        if y >= area.y + area.height {
            continue;
        }
        let width = area.width.saturating_sub(6);
        for offset in 0..width {
            let x = area.x + 3 + offset;
            let blend = (offset as f32 / width.max(1) as f32 + phase * 0.06).fract();
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_bg(mix_color(PANEL_ALT, BG_ALT, 0.4));
                cell.set_char('\u{2584}');
                cell.set_fg(mix_color(COPPER, CYAN, blend));
            }
        }
    }
}

fn render_logo(buffer: &mut Buffer, x: u16, y: u16, lines: &[&str], phase: f32) {
    for (row, line) in lines.iter().enumerate() {
        for (column, glyph) in line.chars().enumerate() {
            if glyph == ' ' {
                continue;
            }

            if let Some(shadow) = buffer.cell_mut((x + column as u16 + 1, y + row as u16 + 1)) {
                shadow.set_char(glyph);
                shadow.set_fg(Color::Rgb(37, 18, 10));
            }

            let blend = ((column as f32 / line.len().max(1) as f32) + phase * 0.07).fract();
            if let Some(cell) = buffer.cell_mut((x + column as u16, y + row as u16)) {
                cell.set_char(glyph);
                cell.set_fg(mix_color(COPPER, AMBER, blend));
            }
        }
    }

    render_gradient_text(
        buffer,
        x + 40.min(18),
        y + lines.len() as u16 + 1,
        "CLI // AI + OPS + VIDEO + WEBCAM + 3D + CHAT + ANALYTICS",
        CYAN,
        ICE,
    );
}

fn render_starburst(buffer: &mut Buffer, center_x: u16, center_y: u16, radius: u16, phase: f32) {
    let rays = 12;
    for ray in 0..rays {
        let angle = phase * 0.45 + ray as f32 * std::f32::consts::TAU / rays as f32;
        let dynamic = radius as f32 * (0.75 + 0.25 * (phase * 1.8 + ray as f32).sin());
        for step in 0..=dynamic.max(1.0) as u16 {
            let dx = angle.cos() * step as f32 * 1.2;
            let dy = angle.sin() * step as f32 * 0.55;
            let x = center_x as i16 + dx.round() as i16;
            let y = center_y as i16 + dy.round() as i16;
            if x < 0 || y < 0 {
                continue;
            }
            if let Some(cell) = buffer.cell_mut((x as u16, y as u16)) {
                let blend = step as f32 / dynamic.max(1.0);
                cell.set_char(if step < radius / 2 { '*' } else { '+' });
                cell.set_fg(mix_color(COPPER, AMBER, blend));
            }
        }
    }

    if let Some(cell) = buffer.cell_mut((center_x, center_y)) {
        cell.set_char('@');
        cell.set_fg(AMBER);
    }
}

fn render_gradient_text(buffer: &mut Buffer, x: u16, y: u16, text: &str, start: Color, end: Color) {
    let length = text.chars().count().max(1);
    for (index, glyph) in text.chars().enumerate() {
        if let Some(cell) = buffer.cell_mut((x + index as u16, y)) {
            cell.set_char(glyph);
            cell.set_fg(mix_color(start, end, index as f32 / length as f32));
        }
    }
}

fn render_equalizer(buffer: &mut Buffer, area: Rect, phase: f32) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let columns = area.width.min(18);
    for i in 0..columns {
        let wave = ((phase * 1.8 + i as f32 * 0.43).sin() * 0.5 + 0.5) * area.height as f32;
        let height = wave.max(1.0) as u16;
        for step in 0..height.min(area.height) {
            let x = area.x + i;
            let y = area.y + area.height - 1 - step;
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_char('\u{2588}');
                cell.set_fg(mix_color(COPPER, CYAN, i as f32 / columns.max(1) as f32));
            }
        }
    }
}

fn render_synthetic_scope(buffer: &mut Buffer, area: Rect, phase: f32) {
    for x in 0..area.width {
        let wave = ((phase * 2.1 + x as f32 * 0.17).sin() * 0.35 + 0.5) * area.height as f32;
        let y = area.y + area.height.saturating_sub(wave as u16 + 1);
        if y >= area.y + area.height {
            continue;
        }
        if let Some(cell) = buffer.cell_mut((area.x + x, y)) {
            cell.set_char('*');
            cell.set_fg(mix_color(COPPER, CYAN, x as f32 / area.width.max(1) as f32));
        }
    }

    for row in (0..area.height).step_by(3) {
        for column in 0..area.width {
            if let Some(cell) = buffer.cell_mut((area.x + column, area.y + row)) {
                if cell.symbol() == " " {
                    cell.set_char('\u{00B7}');
                    cell.set_fg(Color::Rgb(32, 69, 77));
                }
            }
        }
    }
}

fn render_scroller(buffer: &mut Buffer, area: Rect, text: &str, phase: f32, accent: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let mut tape = String::new();
    while tape.chars().count() < area.width as usize * 3 {
        tape.push_str(text);
    }
    let total = tape.chars().count();
    let offset = ((phase * 18.0) as usize) % total.max(1);
    for index in 0..area.width {
        let character = tape
            .chars()
            .nth((offset + index as usize) % total)
            .unwrap_or(' ');
        if let Some(cell) = buffer.cell_mut((area.x + index, area.y)) {
            cell.set_char(character);
            cell.set_bg(Color::Rgb(9, 16, 24));
            cell.set_fg(mix_color(
                accent,
                ICE,
                index as f32 / area.width.max(1) as f32,
            ));
        }
    }
}

fn current_spinner(phase: f32) -> &'static str {
    const FRAMES: [&str; 4] = ["-", "\\", "|", "/"];
    FRAMES[((phase * 8.0) as usize) % FRAMES.len()]
}

fn centered_area(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn mix_color(start: Color, end: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (r1, g1, b1) = to_rgb(start);
    let (r2, g2, b2) = to_rgb(end);
    Color::Rgb(
        (r1 as f32 + (r2 as f32 - r1 as f32) * t) as u8,
        (g1 as f32 + (g2 as f32 - g1 as f32) * t) as u8,
        (b1 as f32 + (b2 as f32 - b1 as f32) * t) as u8,
    )
}

fn to_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        Color::Gray => (128, 128, 128),
        Color::DarkGray => (64, 64, 64),
        _ => (180, 180, 180),
    }
}

fn scale_rgb(r: u8, g: u8, b: u8, factor: f32) -> Color {
    Color::Rgb(
        (r as f32 * factor).clamp(0.0, 255.0) as u8,
        (g as f32 * factor).clamp(0.0, 255.0) as u8,
        (b as f32 * factor).clamp(0.0, 255.0) as u8,
    )
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        let mut result = value
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        result.push('\u{2026}');
        result
    }
}

fn hash32(x: u16, y: u16, seed: u32) -> u32 {
    let mut value = x as u32;
    value = value.wrapping_mul(0x45d9f3b);
    value ^= (y as u32).wrapping_mul(0x119de1f3);
    value ^= seed.wrapping_mul(0x3449_5cbd);
    value ^= value >> 16;
    value = value.wrapping_mul(0x45d9f3b);
    value ^ (value >> 16)
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    args: Args,
) -> Result<()> {
    if let Some(port) = args.serve {
        let addr = format!("0.0.0.0:{}", port);
        let server = Arc::new(VideoChatServer::new());
        let server_clone = Arc::clone(&server);
        let addr_clone = addr.clone();
        tokio::spawn(async move {
            if let Err(e) = server_clone.run(&addr_clone).await {
                eprintln!("server error: {}", e);
            }
        });
    }

    let connect_url = args.connect.clone();
    let username = args.username.clone();
    let mut app = App::new(args)?;

    if let Some(url) = connect_url {
        let client = VideoChatClient::new(username.clone(), url.clone());
        app.video_chat = Some(client);
        app.tiling.set_focused_panel(PanelKind::VideoChatFeeds);
        app.add_system_message(format!(
            "video chat primed for {} as {} -- type /connect {} to go live",
            url, username, url
        ));
    }

    loop {
        terminal.draw(|frame| app.render(frame))?;
        if app.handle_input()? {
            break;
        }
        app.tick();
        tokio::time::sleep(Duration::from_millis(16)).await;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_filename("archive/mega-cli/.env");

    let args = Args::parse();

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, args).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
