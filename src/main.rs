use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use serde::Deserialize;
use std::{
    collections::VecDeque,
    net::SocketAddrV4,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

mod ai;
mod analytics;
mod client;
mod commands;
mod db;
mod effects;
mod games;
mod memory;
mod message;
mod gfx;
mod palette;
mod render;
mod roomcode;
mod server;
mod shader;
mod shell;
mod sysmon;
mod theme;
mod tiling;
mod tiles;
mod tools;
mod video;
mod webcam;

use ai::{
    list_ollama_models, ollama_install_hint, AIClient, AIProvider, AIResponse,
    Message as ApiMessage, OllamaModelInfo, StreamChunk,
};
use analytics::AnalyticsPanel;
use client::{NetEvent, VideoChatClient};
use commands::CommandId;
use db::Database;
use effects::EffectsEngine;
use games::{GameKind, GamesPanel};
use memory::AgentMemory;
use palette::{EntryAction, Palette, PaletteAction, PaletteEntry};
use server::VideoChatServer;
use shell::{format_outcome, run as run_shell, ShellOutcome};
use sysmon::SystemMonitor;
use tiling::{LayoutPreset, PanelKind, TilingManager};
use tiles::TilesPanel;
use tools::{ToolCall, ToolResult, TrustLevel};
use video::VideoPlayer;
use theme::t;
use webcam::WebcamCapture;

const INTRO_DURATION: Duration = Duration::from_millis(7600);

const LARGE_LOGO: &[&str] = &[
    "  █████╗ ███████╗ ██████╗ ██╗ ██╗ ██╗   ██╗ ██╗ ███████╗ ██╗  ██████╗  ███╗   ██╗",
    " ██╔══██╗██╔════╝██╔════╝ ██║ ██║ ██║   ██║ ██║ ██╔════╝ ██║ ██╔═══██╗ ████╗  ██║",
    " ███████║███████╗ ██║      ██║ ██║ ██║   ██║ ██║ ███████╗ ██║ ██║   ██║ ██╔██╗ ██║",
    " ██╔══██║╚════██║ ██║      ██║ ██║ ╚██╗ ██╔╝ ██║ ╚════██║ ██║ ██║   ██║ ██║╚██╗██║",
    " ██║  ██║███████║ ╚██████╗ ██║ ██║  ╚████╔╝  ██║ ███████║ ██║ ╚██████╔╝ ██║ ╚████║",
    " ╚═╝  ╚═╝╚══════╝ ╚═════╝ ╚═╝ ╚═╝   ╚═══╝   ╚═╝ ╚══════╝ ╚═╝  ╚═════╝  ╚═╝  ╚═══╝",
];

const SMALL_LOGO: &[&str] = &[
    "    ___   _____  ____ ____ _    __ ____ _____ ____  _   __",
    "   /   | / ___/ / ___/  _/| |  / //  _// ___//  _/ / | / /",
    "  / /| | \\__ \\/ /   / /  | | / / / / / /__ / /  /  |/ / ",
    " / ___ |___/ / /___/ /   | |/ /_/ / ___/ /_/ / / /|  /  ",
    "/_/  |_/____/\\____/___/  |___//___/____//___//_/ |_/   ",
];

const SCROLLER_TEXT: &str =
    " ASCIIVISION v3.0 // CTRL+P COMMAND PALETTE // /HOST A ROOM AND FRIENDS /JOIN <CODE> // ONLINE PONG + TRON OVER THE WIRE // SNAKE + BREAKOUT // RAY-MARCHED FX: TORUS KNOT METABALLS TUNNEL SYNTHWAVE JULIA // TRUE PTY TILES // F1 MANUAL // F4 FX // F6 LAYOUT // THIS TERMINAL HAS LEFT THE BUILDING ";

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
    YoutubeReady {
        title: String,
        source: String,
    },
    YoutubeFailed {
        error: String,
    },
    OllamaModelsReady {
        models: Vec<OllamaModelInfo>,
    },
    OllamaModelsFailed {
        error: String,
    },
    PendingApproval {
        session_id: u64,
        tool_calls: Vec<ToolCall>,
        context: Vec<ApiMessage>,
    },
    NetStatus {
        message: String,
    },
    NetUserJoined {
        username: String,
    },
    NetUserLeft {
        /// server-assigned id of the peer that left -- forwarded to the games
        /// glue so an online match against them ends (finding #4)
        user_id: String,
        username: String,
    },
}

struct App {
    mode: AppMode,
    provider: AIProvider,
    ai_client: AIClient,
    video: Option<VideoPlayer>,
    video_enabled: bool,
    video_render_mode: render::VideoRenderMode,
    /// terminal graphics-protocol probe (true-pixel path); None until attached
    picker: Option<ratatui_image::picker::Picker>,
    /// a real pixel protocol (Kitty/iTerm2/Sixel) is available
    hw_graphics: bool,
    /// lazily-(re)built pixel protocol for the video panel + screen share.
    /// RefCell so the &self render path can take the &mut the widget needs.
    video_proto: std::cell::RefCell<Option<ratatui_image::protocol::StatefulProtocol>>,
    screen_proto: std::cell::RefCell<Option<ratatui_image::protocol::StatefulProtocol>>,
    video_source_label: String,
    pending_video_load: bool,
    ollama_models: Vec<OllamaModelInfo>,
    ollama_selected_model: Option<String>,
    show_ollama_picker: bool,
    ollama_picker_loading: bool,
    ollama_picker_error: Option<String>,
    ollama_selection_input: String,
    ollama_picker_scroll: usize,
    input: String,
    messages: Vec<ChatMessage>,
    reveal_queue: VecDeque<RevealJob>,
    show_help: bool,
    help_scroll: usize,
    palette: Palette,
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
    games: GamesPanel,
    tiles: TilesPanel,
    analytics: AnalyticsPanel,
    tiling: TilingManager,
    sysmon: SystemMonitor,
    webcam: Option<WebcamCapture>,
    webcam_frame: Option<render::RgbFrame>,
    /// live screen capture (desktop) when screen sharing / previewing
    screenshare: Option<WebcamCapture>,
    screen_frame: Option<render::RgbFrame>,
    /// true while the captured screen is being streamed into the room
    sharing_screen: bool,
    video_chat: Option<Arc<VideoChatClient>>,
    /// hosted video chat server handle, if this instance is hosting
    chat_server: Option<Arc<VideoChatServer>>,
    chat_server_port: Option<u16>,
    /// games<->network glue: outbound channel handed to GamesPanel while a
    /// video chat connection is live (games push, tick() forwards to the wire)
    game_net_tx: Option<mpsc::UnboundedSender<(String, serde_json::Value)>>,
    game_net_rx: Option<mpsc::UnboundedReceiver<(String, serde_json::Value)>>,
    /// last my_id passed to games.set_net (Welcome can arrive after connect)
    game_net_my_id: Option<String>,
    /// bumped on every new dial and on /disconnect; a connect-retry loop
    /// whose generation is stale stops instead of resurrecting a replaced or
    /// cancelled client (finding #6)
    dial_generation: Arc<AtomicU64>,
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
    prev_mode: AppMode,
}

impl ChatMessage {
    fn user(content: String) -> Self {
        Self {
            kind: MessageKind::User,
            label: "YOU".to_string(),
            content,
            accent: t().accent4,
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
            accent: t().accent1,
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
        let video_source_label = video_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("synthetic raster")
            .to_string();
        let video = match video_path {
            Some(path) => Some(VideoPlayer::new(path, video::DECODE_BOX, true)?),
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
                width: 320,
                height: 240,
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
            ai_client: AIClient::new(provider.clone(), None),
            video_enabled: true,
            video,
            video_render_mode: render::VideoRenderMode::default(),
            picker: None,
            hw_graphics: false,
            video_proto: std::cell::RefCell::new(None),
            screen_proto: std::cell::RefCell::new(None),
            video_source_label,
            pending_video_load: false,
            ollama_models: Vec::new(),
            ollama_selected_model: None,
            show_ollama_picker: false,
            ollama_picker_loading: false,
            ollama_picker_error: None,
            ollama_selection_input: String::new(),
            ollama_picker_scroll: 0,
            input: String::new(),
            messages: Vec::new(),
            reveal_queue: VecDeque::new(),
            show_help: false,
            help_scroll: 0,
            palette: Palette::new(),
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
            games: GamesPanel::new(),
            tiles: TilesPanel::new(),
            analytics: AnalyticsPanel::new(),
            tiling: TilingManager::new(),
            sysmon: SystemMonitor::new(),
            webcam,
            webcam_frame: None,
            screenshare: None,
            screen_frame: None,
            sharing_screen: false,
            video_chat: None,
            chat_server: None,
            chat_server_port: None,
            game_net_tx: None,
            game_net_rx: None,
            game_net_my_id: None,
            dial_generation: Arc::new(AtomicU64::new(0)),
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
            prev_mode: if args.skip_intro {
                AppMode::Chat
            } else {
                AppMode::Intro
            },
        };

        app.add_system_message(
            "ASCIIVISION v3 online — press Ctrl+P for the command palette: fuzzy-search every command, effect, game, and layout",
        );
        app.add_system_message(format!(
            "provider uplink live: {} // F1 full manual // F2 rotate provider // shell via !<command>",
            app.provider_display_name()
        ));
        app.add_system_message(
            "multiplayer: /host opens a room and prints a code, a friend runs /join <code> — then Pong or Tron -> HOST/JOIN ONLINE",
        );
        app.add_system_message(format!(
            "agent tools active // trust: {} (/trust cycles) // @<file> injects, /remember <key>=<value> stores memory",
            app.trust_level.name()
        ));

        if app.video.is_none() {
            app.add_system_message("video signal offline: no bundled mp4 found, falling back to synthetic raster field");
        }

        if app.db.is_none() && !args.no_db {
            app.add_system_message(
                "conversation archive offline: ~/.config/asciivision could not be initialized",
            );
        }

        if app.provider == AIProvider::Ollama {
            app.prepare_ollama_provider("startup route");
        }

        if app.webcam.is_some() {
            app.add_system_message("webcam capture online: live ascii feed active");
        }

        Ok(app)
    }

    fn rebuild_ai_client(&mut self) {
        let model = if self.provider == AIProvider::Ollama {
            self.ollama_selected_model.clone()
        } else {
            None
        };
        self.ai_client = AIClient::new(self.provider.clone(), model);
    }

    fn provider_display_name(&self) -> String {
        if self.provider == AIProvider::Ollama {
            if let Some(model) = &self.ollama_selected_model {
                format!("{} ({})", self.provider.name(), model)
            } else {
                format!("{} (select model)", self.provider.name())
            }
        } else {
            self.provider.name().to_string()
        }
    }

    fn provider_status_badge(&self) -> String {
        if self.provider == AIProvider::Ollama {
            if let Some(model) = &self.ollama_selected_model {
                format!("{} // {}", self.provider.badge(), truncate(model, 24))
            } else {
                format!("{} // select model", self.provider.badge())
            }
        } else {
            self.provider.badge().to_string()
        }
    }

    fn request_ollama_models(&mut self) {
        self.ollama_picker_loading = true;
        self.ollama_picker_error = None;
        let tx = self.events_tx.clone();
        tokio::spawn(async move {
            let event = match list_ollama_models().await {
                Ok(models) => AppEvent::OllamaModelsReady { models },
                Err(error) => AppEvent::OllamaModelsFailed {
                    error: error.to_string(),
                },
            };
            let _ = tx.send(event);
        });
    }

    fn prepare_ollama_provider(&mut self, route: &str) {
        self.show_ollama_picker = true;
        self.ollama_selection_input.clear();
        self.ollama_picker_scroll = 0;
        self.request_ollama_models();
        self.rebuild_ai_client();
        if let Some(model) = self.ollama_selected_model.clone() {
            self.add_system_message(format!(
                "{} -> {} // current model: {} // type a number to switch",
                route,
                self.provider.name(),
                model
            ));
            self.status_note = format!("ollama ready: {}", truncate(&model, 28));
        } else {
            self.add_system_message(format!(
                "{} -> {} // type a model number and press Enter",
                route,
                self.provider.name()
            ));
            self.status_note = "ollama: select model".to_string();
        }
    }

    fn set_provider(&mut self, provider: AIProvider, route: &str) {
        self.session_id = self.session_id.wrapping_add(1);
        self.pending_ai = false;
        self.provider = provider;
        if self.provider == AIProvider::Ollama {
            self.prepare_ollama_provider(route);
        } else {
            self.show_ollama_picker = false;
            self.ollama_selection_input.clear();
            self.ollama_picker_error = None;
            self.rebuild_ai_client();
            self.add_system_message(format!("{} -> {}", route, self.provider.name()));
            self.status_note = format!("active provider: {}", self.provider_status_badge());
        }
    }

    fn select_ollama_model_by_number(&mut self, selection: usize) {
        if selection == 0 || selection > self.ollama_models.len() {
            self.status_note = format!("ollama model {} is out of range", selection);
            self.add_system_message(format!(
                "ollama model {} is out of range. choose 1-{}.",
                selection,
                self.ollama_models.len()
            ));
            return;
        }

        let model = self.ollama_models[selection - 1].name.clone();
        self.ollama_selected_model = Some(model.clone());
        self.show_ollama_picker = false;
        self.ollama_selection_input.clear();
        self.rebuild_ai_client();
        self.add_system_message(format!("ollama model selected -> {}", model));
        self.status_note = format!("ollama model: {}", truncate(&model, 28));
    }

    fn confirm_ollama_selection(&mut self) {
        if self.ollama_picker_loading {
            self.status_note = "ollama models are still loading".to_string();
            return;
        }

        if self.ollama_models.is_empty() {
            let error = self
                .ollama_picker_error
                .clone()
                .unwrap_or_else(|| "no ollama models available".to_string());
            self.add_system_message(format!("ollama unavailable: {}", error));
            self.status_note = "ollama unavailable".to_string();
            return;
        }

        if self.ollama_selection_input.is_empty() {
            if let Some(model) = &self.ollama_selected_model {
                self.show_ollama_picker = false;
                self.status_note = format!("ollama model: {}", truncate(model, 28));
            } else {
                self.status_note = "type an ollama model number first".to_string();
            }
            return;
        }

        match self.ollama_selection_input.parse::<usize>() {
            Ok(selection) => self.select_ollama_model_by_number(selection),
            Err(_) => {
                self.add_system_message("ollama selection must be a number");
                self.status_note = "invalid ollama model number".to_string();
            }
        }
    }

    fn handle_ollama_picker_key(&mut self, key: KeyEvent) -> bool {
        if !self.show_ollama_picker {
            return false;
        }

        match key.code {
            KeyCode::Esc => {
                self.show_ollama_picker = false;
                self.ollama_selection_input.clear();
                self.status_note = if let Some(model) = &self.ollama_selected_model {
                    format!("ollama model: {}", truncate(model, 28))
                } else {
                    "ollama picker closed".to_string()
                };
            }
            KeyCode::Enter => self.confirm_ollama_selection(),
            KeyCode::Backspace => {
                self.ollama_selection_input.pop();
            }
            KeyCode::Up => {
                self.ollama_picker_scroll = self.ollama_picker_scroll.saturating_sub(1);
            }
            KeyCode::Down => {
                self.ollama_picker_scroll = self
                    .ollama_picker_scroll
                    .saturating_add(1)
                    .min(self.ollama_models.len().saturating_sub(1));
            }
            KeyCode::PageUp => {
                self.ollama_picker_scroll = self.ollama_picker_scroll.saturating_sub(8);
            }
            KeyCode::PageDown => {
                self.ollama_picker_scroll = self
                    .ollama_picker_scroll
                    .saturating_add(8)
                    .min(self.ollama_models.len().saturating_sub(1));
            }
            KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.ollama_picker_scroll = self
                    .ollama_picker_scroll
                    .saturating_add(1)
                    .min(self.ollama_models.len().saturating_sub(1));
            }
            KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.ollama_picker_scroll = self.ollama_picker_scroll.saturating_sub(1);
            }
            KeyCode::Char('r') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.request_ollama_models();
                self.status_note = "refreshing ollama models".to_string();
            }
            KeyCode::Char(c)
                if c.is_ascii_digit() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.ollama_selection_input.push(c);
            }
            _ => {}
        }

        true
    }

    fn tick(&mut self) {
        if matches!(self.mode, AppMode::Intro) && self.intro_started.elapsed() >= INTRO_DURATION {
            self.mode = AppMode::Chat;
            self.status_note = "intro faded into live deck".to_string();
        }

        let mut video_new = false;
        if let Some(video) = &mut self.video {
            if self.video_enabled || matches!(self.mode, AppMode::Intro) {
                video_new = video.tick();
            }
        }
        if video_new {
            self.rebuild_video_proto();
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

        // poll screen share -- drive the local preview AND, when connected and
        // actively sharing, stream each captured frame into the room
        let mut screen_new = false;
        if let Some(ref cam) = self.screenshare {
            let live = self.sharing_screen
                && self.video_chat.as_ref().map_or(false, |c| c.is_connected());
            while let Some(frame) = cam.try_recv() {
                if live {
                    if let Some(vc) = &self.video_chat {
                        vc.send_frame(frame.clone());
                    }
                }
                self.screen_frame = Some(frame);
                screen_new = true;
            }
            if self.screen_frame.is_none() {
                if let Some(err) = cam.error() {
                    self.status_note = format!("screen: {}", truncate(&err, 44));
                }
            }
        }
        if screen_new {
            self.rebuild_screen_proto();
        }

        self.sync_game_net();

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
                                format!("{} response injected", self.provider_status_badge());
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
                        // finding #20: never leave the palette hiding an
                        // approval prompt -- Enter/Esc must mean approve/reject
                        self.palette.close();
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
                        let _accent = if tr.success { t().accent3 } else { t().danger };
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
                                // finding #20: see the AiToolCalls branch
                                self.palette.close();
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
                            self.status_note =
                                format!("{} stream complete", self.provider_status_badge());
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
                    // finding #20: see the AiToolCalls branch
                    self.palette.close();
                    self.pending_approval = Some(PendingApprovalState {
                        tool_calls,
                        context,
                        session_id,
                    });
                }
                AppEvent::ShellFinished { outcome } => {
                    self.pending_shells = self.pending_shells.saturating_sub(1);
                    let success = outcome.exit_code.unwrap_or(1) == 0 && !outcome.timed_out;
                    let accent = if success { t().accent3 } else { t().danger };
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
                AppEvent::YoutubeReady { title, source } => {
                    self.pending_video_load = false;
                    match VideoPlayer::new(source, video::DECODE_BOX, false) {
                        Ok(player) => {
                            self.video = Some(player);
                            self.video_enabled = true;
                            self.video_source_label = title.clone();
                            self.tiling.set_focused_panel(PanelKind::Video);
                            self.add_system_message(format!("youtube stream locked: {}", title));
                            self.status_note =
                                format!("youtube streaming: {}", truncate(&title, 30));
                        }
                        Err(error) => {
                            self.add_system_message(format!("youtube video error: {}", error));
                            self.status_note = "youtube video failed to open".to_string();
                        }
                    }
                }
                AppEvent::YoutubeFailed { error } => {
                    self.pending_video_load = false;
                    self.add_system_message(format!("youtube error: {}", error));
                    self.status_note = "youtube load failed".to_string();
                }
                AppEvent::OllamaModelsReady { models } => {
                    self.ollama_picker_loading = false;
                    self.ollama_picker_error = None;
                    self.ollama_models = models;
                    if let Some(selected) = &self.ollama_selected_model {
                        if !self.ollama_models.iter().any(|model| &model.name == selected) {
                            self.ollama_selected_model = None;
                            self.rebuild_ai_client();
                        }
                    }
                    if self.provider == AIProvider::Ollama {
                        self.add_system_message(format!(
                            "ollama models ready: {} detected on this machine. type a number and press Enter.",
                            self.ollama_models.len()
                        ));
                        self.status_note =
                            format!("ollama models ready: {}", self.ollama_models.len());
                    }
                }
                AppEvent::OllamaModelsFailed { error } => {
                    self.ollama_picker_loading = false;
                    self.ollama_models.clear();
                    self.ollama_picker_error = Some(error.clone());
                    if self.provider == AIProvider::Ollama {
                        self.add_system_message(format!("ollama error: {}", error));
                        self.status_note = "ollama unavailable".to_string();
                    }
                }
                AppEvent::NetStatus { message } => {
                    self.add_system_message(format!("videochat: {}", message));
                    self.status_note = format!("vc: {}", truncate(&message, 44));
                }
                AppEvent::NetUserJoined { username } => {
                    self.add_system_message(format!("videochat: {} joined the room", username));
                    self.status_note = format!("{} joined", truncate(&username, 24));
                }
                AppEvent::NetUserLeft { user_id, username } => {
                    // finding #4: a vanished peer must reach online games --
                    // if an online match is locked to this user, it ends as
                    // if they sent quit instead of freezing forever.
                    self.games.peer_disconnected(Some(&user_id));
                    self.add_system_message(format!("videochat: {} left the room", username));
                    self.status_note = format!("{} left", truncate(&username, 24));
                }
            }
        }

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        self.games.tick(elapsed.as_secs_f32());
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
                    // Accept Press + Repeat only. On Windows (and with the
                    // kitty keyboard protocol) Release events are delivered
                    // too and would double-fire every keystroke.
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }

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
                    self.scroll_lines = 0;
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

        if !matches!(key.code, KeyCode::F(_))
            && !(key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('l') | KeyCode::Char('c')))
            && self.handle_ollama_picker_key(key)
        {
            return Ok(false);
        }

        // Finding #20: a tool approval that landed while the palette was open
        // must win the keyboard. Close the palette and fall through so this
        // very Enter/Esc reaches the approve/reject flow below.
        if self.palette.is_open() && self.pending_approval.is_some() {
            self.palette.close();
            self.status_note = "palette closed -- tool approval pending".to_string();
        }

        // Finding #7: F-keys bypass the palette interceptor below, and their
        // side effects can open another modal (F2 -> provider cycle -> Ollama
        // picker) which would render UNDER the palette while stealing its
        // keys. Close the palette first, then let the F-key process normally.
        // The mirror case is already safe: while the Ollama picker is open,
        // Ctrl+P is consumed by the picker interceptor above, so the palette
        // cannot stack on top of the picker.
        if self.palette.is_open() && matches!(key.code, KeyCode::F(_)) {
            self.palette.close();
            self.status_note = "palette closed".to_string();
        }

        // Command palette: modal interceptor (same bypass list as the Ollama
        // picker: F-keys, Ctrl+L, Ctrl+C pass through). Esc is consumed here,
        // so closing the palette can never trigger the double-Esc quit.
        if self.palette.is_open()
            && !matches!(key.code, KeyCode::F(_))
            && !(key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('l') | KeyCode::Char('c')))
        {
            match self.palette.handle_key(key) {
                PaletteAction::Close => {
                    self.palette.close();
                    self.status_note = "palette closed".to_string();
                }
                PaletteAction::Execute { action, title } => {
                    self.palette.close();
                    self.run_palette_action(action, &title);
                }
                PaletteAction::Consumed => {}
            }
            return Ok(false);
        }

        // Ctrl+P opens the palette — but never over a pending tool approval
        // (those keys must keep meaning approve/reject).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('p')
            && self.pending_approval.is_none()
        {
            self.open_palette();
            return Ok(false);
        }

        // Help overlay: scroll keys while open (picker-style). Other keys
        // fall through so typing and F-keys keep working underneath.
        if self.show_help {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Esc if self.pending_approval.is_none() => {
                    self.show_help = false;
                    self.status_note = "help closed".to_string();
                    return Ok(false);
                }
                KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(8);
                    return Ok(false);
                }
                KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(8);
                    return Ok(false);
                }
                KeyCode::Up => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                    return Ok(false);
                }
                KeyCode::Down => {
                    self.help_scroll = self.help_scroll.saturating_add(1);
                    return Ok(false);
                }
                KeyCode::Char('j') if !ctrl && self.input.is_empty() => {
                    self.help_scroll = self.help_scroll.saturating_add(1);
                    return Ok(false);
                }
                KeyCode::Char('k') if !ctrl && self.input.is_empty() => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                    return Ok(false);
                }
                _ => {}
            }
        }

        if self.pending_approval.is_none()
            && self.input.is_empty()
            && self.tiling.focused_panel() == Some(PanelKind::Games)
            && self.games.handle_key(key)
        {
            self.status_note = self.games.status_note().to_string();
            return Ok(false);
        }

        if self.pending_approval.is_none()
            && self.input.is_empty()
            && self.tiling.focused_panel() == Some(PanelKind::Tiles)
            && self.should_route_key_to_tiles(key)
            && self.tiles.handle_key(key)
        {
            self.status_note = self.tiles.status_note().to_string();
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
                if self.show_help {
                    self.help_scroll = 0;
                    self.status_note = "help open // PgUp/PgDn scroll // Esc closes".to_string();
                } else {
                    self.status_note = "help closed".to_string();
                }
            }
            KeyCode::F(9) => {
                theme::set_random_theme();
                self.status_note = "theme randomized".to_string();
                self.add_system_message("color palette randomized -- F9 again for another, /theme reset to restore defaults");
            }
            KeyCode::F(10) => {
                theme::reset_theme();
                self.status_note = "theme restored to default".to_string();
            }
            KeyCode::F(2) => {
                self.set_provider(self.provider.cycle(), "uplink rerouted");
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
                self.effects.cycle_with_off();
                self.status_note = if self.effects.active {
                    format!("3D fx: {}", self.effects.current_name())
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
                match self.tiles.activate_default() {
                    Ok(()) => {
                        self.tiling.set_focused_panel(PanelKind::Tiles);
                        self.status_note = self.tiles.status_note().to_string();
                    }
                    Err(error) => {
                        self.add_system_message(format!("tiles error: {}", error));
                        self.status_note = "tiles failed to boot".to_string();
                    }
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
                    // keep leading whitespace: it is the explicit
                    // "send this to the AI" escape hatch (finding #21)
                    let input = std::mem::take(&mut self.input);
                    if !input.trim().is_empty() {
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

    fn should_route_key_to_tiles(&self, key: KeyEvent) -> bool {
        if matches!(key.code, KeyCode::F(_)) {
            return false;
        }

        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }

        // Ctrl+j/k goes to tiles for inner terminal cycling
        if matches!(key.code, KeyCode::Char('j') | KeyCode::Char('k')) {
            return true;
        }

        // Block Ctrl+h/l (outer focus), Ctrl+Shift (swap), Ctrl+n, Ctrl+[/],
        // and Ctrl+p (command palette) from reaching the PTY.
        !matches!(
            key.code,
            KeyCode::Char('h')
                | KeyCode::Char('l')
                | KeyCode::Char('H')
                | KeyCode::Char('J')
                | KeyCode::Char('K')
                | KeyCode::Char('L')
                | KeyCode::Char('n')
                | KeyCode::Char('p')
                | KeyCode::Char('[')
                | KeyCode::Char(']')
        )
    }

    fn dispatch_input(&mut self, input: String) {
        self.follow_tail = true;

        // A leading space always forces the input to the AI, bypassing the
        // command/shell prefixes entirely (finding #21).
        if input.starts_with(char::is_whitespace) {
            self.start_ai(input.trim().to_string());
            return;
        }
        let input = input.trim().to_string();

        // !<command> raw shell escape hatch
        if let Some(rest) = input.strip_prefix('!') {
            let command = rest.trim();
            if command.is_empty() {
                self.add_system_message("usage: !<command>");
            } else {
                self.start_shell(command.to_string());
            }
            return;
        }

        // Slash commands resolve through the registry. Unknown commands are
        // guarded here only when the input is command-shaped: a typo never
        // leaks to the AI, but a filesystem path or a longer '/'-leading
        // sentence is a prompt and still reaches the model (finding #21).
        if input.starts_with('/') {
            let (token, args) = match input.split_once(char::is_whitespace) {
                Some((token, rest)) => (token, rest.trim()),
                None => (input.as_str(), ""),
            };
            match commands::find(token) {
                Some(spec) => {
                    let id = spec.id;
                    self.run_command(id, args);
                }
                None => {
                    if looks_like_mistyped_command(&input) {
                        self.add_system_message(format!(
                            "unknown command {} — press Ctrl+P for the palette, F1 for the manual, or prefix with a space to send it to the AI",
                            token
                        ));
                        self.status_note =
                            format!("unknown command {} — Ctrl+P or /help", truncate(token, 24));
                        return;
                    }
                    self.start_ai(input);
                }
            }
            return;
        }

        self.start_ai(input);
    }

    /// Build the palette entry list (registry commands + dynamic actions for
    /// effects, games, layouts, and providers) and open the overlay.
    fn open_palette(&mut self) {
        let mut entries: Vec<PaletteEntry> = Vec::with_capacity(64);
        for spec in commands::REGISTRY {
            entries.push(PaletteEntry::for_command(spec));
        }
        for name in self.effects.names() {
            entries.push(PaletteEntry::effect(name));
        }
        for kind in GameKind::ALL {
            entries.push(PaletteEntry::game(kind));
        }
        for preset in LayoutPreset::ALL {
            entries.push(PaletteEntry::layout(preset));
        }
        for provider in palette::PROVIDERS {
            entries.push(PaletteEntry::provider(provider));
        }
        self.palette.open(entries);
        self.status_note = "palette: type to filter // Enter runs // Esc closes".to_string();
    }

    /// Execute a palette entry after the overlay closes.
    fn run_palette_action(&mut self, action: EntryAction, title: &str) {
        match action {
            EntryAction::Command { id, args } => self.run_command(id, args),
            EntryAction::Insert(text) => {
                self.input = text;
                self.status_note = format!("{} — type the argument, then Enter", title);
            }
            EntryAction::Effect(name) => {
                if self.effects.set_by_name(name) {
                    self.add_system_message(format!("3D effect: {}", self.effects.current_name()));
                    self.status_note = format!("3D fx: {}", self.effects.current_name());
                }
            }
            EntryAction::Game(kind) => {
                self.ensure_games_visible();
                self.games.launch(kind);
                self.status_note = self.games.status_note().to_string();
            }
            EntryAction::Layout(preset) => {
                self.tiling.apply_preset(preset);
                self.status_note = format!("layout: {}", preset.name());
            }
            EntryAction::Provider(provider) => {
                self.set_provider(provider, "palette route");
            }
        }
    }

    /// Make sure a Games tile is on screen: focus an existing one, or apply
    /// the Arcade preset when no tile currently shows Games.
    fn ensure_games_visible(&mut self) {
        let existing = self
            .tiling
            .leaves()
            .into_iter()
            .find(|(_, panel)| *panel == PanelKind::Games);
        match existing {
            Some((id, _)) => self.tiling.focused = id,
            None => {
                self.tiling.apply_preset(LayoutPreset::Arcade);
                self.add_system_message("layout: ARCADE");
            }
        }
    }

    /// Execute one registry command. This match is EXHAUSTIVE over CommandId
    /// (no wildcard arm), so adding a registry entry without a dispatch
    /// branch here is a compile error — registry and dispatcher cannot drift.
    fn run_command(&mut self, id: CommandId, args: &str) {
        match id {
            CommandId::Help => {
                self.show_help = !self.show_help;
                if self.show_help {
                    self.help_scroll = 0;
                }
            }
            CommandId::Clear => {
                self.messages.clear();
                self.reveal_queue.clear();
                self.status_note = "transcript purged".to_string();
            }
            CommandId::Video => {
                self.video_enabled = !self.video_enabled;
                self.status_note = if self.video_enabled {
                    "video bus online".to_string()
                } else {
                    "video bus muted".to_string()
                };
            }
            CommandId::VideoMode => {
                self.cycle_video_mode();
            }
            CommandId::Screenshare => {
                self.toggle_screenshare();
            }
            CommandId::Youtube => {
                let url = args.to_string();
                if url.is_empty() {
                    self.add_system_message("usage: /youtube https://youtube.com/watch?v=...");
                    return;
                }
                self.pending_video_load = true;
                self.status_note = "youtube stream resolving".to_string();
                self.add_system_message(format!(
                    "youtube stream requested: {}",
                    truncate(&url, 72)
                ));
                let tx = self.events_tx.clone();
                tokio::spawn(async move {
                    let event = match resolve_youtube_stream(url).await {
                        Ok(video) => AppEvent::YoutubeReady {
                            title: video.title,
                            source: video.source,
                        },
                        Err(error) => AppEvent::YoutubeFailed {
                            error: error.to_string(),
                        },
                    };
                    let _ = tx.send(event);
                });
            }
            CommandId::Webcam => {
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
            }

            CommandId::Effects => {
                self.effects.active = !self.effects.active;
                self.status_note = if self.effects.active {
                    format!("3D fx: {}", self.effects.current_name())
                } else {
                    "3D fx offline".to_string()
                };
            }
            CommandId::Fx => {
                if !args.is_empty() {
                    if self.effects.set_by_name(args) {
                        self.add_system_message(format!(
                            "3D effect: {}",
                            self.effects.current_name()
                        ));
                        self.status_note = format!("3D fx: {}", self.effects.current_name());
                    } else {
                        self.add_system_message(format!(
                            "unknown effect '{}' -- available: {}",
                            args,
                            self.effects.names().join(", ")
                        ));
                    }
                    return;
                }
                self.effects.cycle_with_off();
                if self.effects.active {
                    self.add_system_message(format!("3D effect: {}", self.effects.current_name()));
                } else {
                    self.add_system_message("3D effects offline");
                }
            }
            CommandId::Randomize => {
                theme::set_random_theme();
                self.add_system_message(
                    "color palette randomized -- /theme reset to restore defaults",
                );
                self.status_note = "theme randomized".to_string();
            }
            CommandId::Theme => match args.to_lowercase().as_str() {
                "random" | "randomize" => self.run_command(CommandId::Randomize, ""),
                "reset" | "default" => {
                    theme::reset_theme();
                    self.add_system_message("theme restored to factory defaults");
                    self.status_note = "theme reset".to_string();
                }
                _ => {
                    self.add_system_message("usage: /theme random|reset");
                }
            },
            CommandId::Analytics => {
                self.analytics.active = !self.analytics.active;
                if self.analytics.active {
                    self.analytics.refresh(self.db.as_ref());
                    self.tiling.set_focused_panel(PanelKind::Analytics);
                }
            }

            CommandId::Layout => {
                if args.is_empty() {
                    let preset = self.tiling.preset.cycle();
                    self.tiling.apply_preset(preset);
                    self.add_system_message(format!("layout: {}", preset.name()));
                    return;
                }
                let preset = match args.to_lowercase().as_str() {
                    "default" => LayoutPreset::Default,
                    "dual" => LayoutPreset::DualPane,
                    "triple" => LayoutPreset::TripleColumn,
                    "quad" => LayoutPreset::Quad,
                    "webcam" | "cam" => LayoutPreset::WebcamFocus,
                    "focus" | "full" => LayoutPreset::FullFocus,
                    "videochat" | "vc" => LayoutPreset::VideoChat,
                    "arcade" | "games" => LayoutPreset::Arcade,
                    _ => {
                        self.add_system_message(
                            "layouts: default, dual, triple, quad, webcam, focus, videochat, arcade",
                        );
                        return;
                    }
                };
                self.tiling.apply_preset(preset);
                self.add_system_message(format!("layout: {}", preset.name()));
            }
            CommandId::Sysmon => {
                self.tiling.set_focused_panel(PanelKind::SystemMonitor);
            }
            CommandId::Games => {
                if args.is_empty() {
                    // bare /games: bring the arcade up — Arcade preset when no
                    // Games tile is visible, else focus the existing one
                    self.ensure_games_visible();
                    self.status_note = self.games.status_note().to_string();
                    return;
                }
                match args.to_lowercase().as_str() {
                    "menu" | "select" | "stop" => {
                        self.games.stop();
                        self.tiling.set_focused_panel(PanelKind::Games);
                        self.status_note = self.games.status_note().to_string();
                    }
                    "play" | "start" => {
                        self.games.activate_selected();
                        self.tiling.set_focused_panel(PanelKind::Games);
                        self.status_note = self.games.status_note().to_string();
                    }
                    "next" => {
                        self.games.next_game();
                        self.tiling.set_focused_panel(PanelKind::Games);
                        self.status_note = self.games.status_note().to_string();
                    }
                    "prev" | "previous" => {
                        self.games.previous_game();
                        self.tiling.set_focused_panel(PanelKind::Games);
                        self.status_note = self.games.status_note().to_string();
                    }
                    _ => {
                        if let Some(game) = GameKind::from_input(args) {
                            self.ensure_games_visible();
                            self.games.launch(game);
                            self.status_note = self.games.status_note().to_string();
                        } else {
                            self.add_system_message(
                                "games: /games, /games play, /games menu, /games next, /games pacman|space|penguin|pong|tron|snake|breakout",
                            );
                        }
                    }
                }
            }
            CommandId::Tiles => {
                let count = if args.is_empty() {
                    2
                } else {
                    match args.parse::<usize>() {
                        Ok(count @ 1..=8) => count,
                        _ => {
                            self.add_system_message("tiles: /tiles or /tiles <1-8>");
                            return;
                        }
                    }
                };
                match self.tiles.activate_count(count) {
                    Ok(()) => {
                        self.tiling.set_focused_panel(PanelKind::Tiles);
                        self.status_note = self.tiles.status_note().to_string();
                    }
                    Err(error) => {
                        self.add_system_message(format!("tiles error: {}", error));
                        self.status_note = "tiles failed to boot".to_string();
                    }
                }
            }

            CommandId::Server => {
                if let Ok(port) = args.parse::<u16>() {
                    if self.start_chat_server(port) {
                        self.add_system_message(format!(
                            "video chat server live on 0.0.0.0:{} -- /host does this plus auto-join",
                            port
                        ));
                        self.status_note = format!("video chat server live on :{}", port);
                    }
                } else {
                    self.add_system_message("usage: /server <port>");
                }
            }
            CommandId::Host => {
                let port = if args.is_empty() {
                    Some(9999)
                } else {
                    args.parse::<u16>().ok()
                };
                let Some(port) = port else {
                    self.add_system_message("usage: /host [port]   (default 9999)");
                    return;
                };
                if self.video_chat.as_ref().map_or(false, |c| c.is_connected()) {
                    self.add_system_message("already in a room -- /disconnect first");
                    return;
                }
                // finding #5: after /disconnect the server keeps listening, so
                // a repeat /host must rejoin the running room instead of dead-
                // ending on "server already running" without a client.
                match self.chat_server_port {
                    Some(running) if running == port => {
                        self.add_system_message(format!(
                            "rejoining your running room on :{}",
                            port
                        ));
                        self.start_video_client(format!("ws://127.0.0.1:{}", port));
                        self.print_room_invite();
                        self.status_note = format!("hosting room on :{}", port);
                    }
                    Some(running) => {
                        self.add_system_message(format!(
                            "server already on :{} -- /host {} to rejoin it",
                            running, running
                        ));
                        self.status_note = format!("server already on :{}", running);
                    }
                    None => {
                        if self.start_chat_server(port) {
                            self.start_video_client(format!("ws://127.0.0.1:{}", port));
                            self.print_room_invite();
                            self.status_note = format!("hosting room on :{}", port);
                        }
                    }
                }
            }
            CommandId::Join => {
                let code = args;
                if code.is_empty() {
                    self.add_system_message(
                        "usage: /join <room-code>   (get one from a /host friend)",
                    );
                    return;
                }
                match roomcode::decode(code) {
                    Some(addr) => {
                        if self.video_chat.as_ref().map_or(false, |c| c.is_connected()) {
                            self.add_system_message("already in a room -- /disconnect first");
                            return;
                        }
                        self.add_system_message(format!(
                            "joining room {} ({}) as {}",
                            code.to_uppercase(),
                            addr,
                            self.username
                        ));
                        self.start_video_client(format!("ws://{}", addr));
                    }
                    None => {
                        self.add_system_message(
                            "that room code didn't parse -- expected something like K7QM3-XZ2AB",
                        );
                    }
                }
            }
            CommandId::Invite => {
                self.print_room_invite();
            }
            CommandId::Disconnect => {
                // cancel any dial retry loop still in flight (finding #6)
                self.dial_generation.fetch_add(1, AtomicOrdering::SeqCst);
                if let Some(vc) = self.video_chat.take() {
                    vc.disconnect();
                    // finding #4: leaving the room ends any online game
                    // session immediately (the opponent gets our quit or
                    // their own UserLeft; we must not keep simulating them)
                    self.games.peer_disconnected(None);
                    if let Some(port) = self.chat_server_port {
                        self.add_system_message(format!(
                            "left the room (your server is still listening on :{})",
                            port
                        ));
                    } else {
                        self.add_system_message("left the room");
                    }
                    self.status_note = "video chat offline".to_string();
                } else {
                    self.add_system_message("not connected to video chat");
                }
            }
            CommandId::Connect => {
                if args.is_empty() {
                    self.add_system_message("usage: /connect ws://<addr>:<port>");
                    return;
                }
                let url = if args.contains("://") {
                    args.to_string()
                } else {
                    format!("ws://{}", args)
                };
                if self.video_chat.as_ref().map_or(false, |c| c.is_connected()) {
                    self.add_system_message("already in a room -- /disconnect first");
                    return;
                }
                self.add_system_message(format!("connecting to {} as {}", url, self.username));
                self.start_video_client(url);
            }
            CommandId::Chat => {
                if args.is_empty() {
                    self.add_system_message("usage: /chat <message>");
                    return;
                }
                match self.video_chat {
                    Some(ref vc) if vc.is_connected() => {
                        vc.send_chat(args.to_string());
                    }
                    _ => {
                        self.add_system_message(
                            "not connected to video chat. use /host, /join <code>, or /connect ws://<addr>",
                        );
                    }
                }
            }
            CommandId::Username => {
                if args.is_empty() {
                    self.add_system_message("usage: /username <name>");
                    return;
                }
                self.username = args.to_string();
                if self.video_chat.as_ref().map_or(false, |c| c.is_connected()) {
                    self.add_system_message(format!(
                        "username set to: {} (applies to your next connection)",
                        self.username
                    ));
                } else {
                    self.add_system_message(format!("username set to: {}", self.username));
                }
            }

            CommandId::Provider => {
                if args.is_empty() {
                    self.add_system_message("usage: /provider claude|grok|gpt|gemini|ollama");
                    return;
                }
                self.set_provider(AIProvider::from_input(args), "manual route");
            }
            CommandId::Ollama => {
                self.set_provider(AIProvider::Ollama, "manual route");
            }
            CommandId::Trust => {
                self.trust_level = self.trust_level.cycle();
                self.add_system_message(format!("trust level: {}", self.trust_level.name()));
                self.status_note = format!("trust: {}", self.trust_level.name());
            }
            CommandId::Remember => {
                if let Some((key, value)) = args.split_once('=') {
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
            }
            CommandId::Forget => {
                if args.is_empty() {
                    self.add_system_message("usage: /forget <key>");
                    return;
                }
                if let Some(ref db) = self.db {
                    match AgentMemory::forget(db, args) {
                        Ok(true) => {
                            self.agent_memory.load(db);
                            self.add_system_message(format!("forgot: {}", args));
                        }
                        Ok(false) => {
                            self.add_system_message(format!("no memory found for: {}", args))
                        }
                        Err(e) => self.add_system_message(format!("memory error: {}", e)),
                    }
                }
            }
            CommandId::Recall => {
                if args.is_empty() {
                    self.add_system_message("usage: /recall <key>");
                    return;
                }
                if let Some(ref db) = self.db {
                    match AgentMemory::recall(db, args) {
                        Some(value) => self.add_system_message(format!("{} = {}", args, value)),
                        None => self.add_system_message(format!("no memory for: {}", args)),
                    }
                }
            }
            CommandId::Memory => {
                if let Some(ref db) = self.db {
                    self.agent_memory.load(db);
                }
                let entries = self.agent_memory.all_entries();
                if entries.is_empty() {
                    self.add_system_message("agent memory is empty. use /remember key = value");
                } else {
                    let lines: Vec<String> = entries
                        .iter()
                        .map(|e| {
                            format!(
                                "  {} = {} [{}]",
                                e.key,
                                truncate(&e.value, 60),
                                e.kind.as_str_pub()
                            )
                        })
                        .collect();
                    self.add_system_message(format!(
                        "agent memory ({} entries):\n{}",
                        entries.len(),
                        lines.join("\n")
                    ));
                }
            }
            CommandId::Pin => {
                let last_idx = self.messages.len().saturating_sub(1);
                if !self.pinned_messages.contains(&last_idx) {
                    self.pinned_messages.push(last_idx);
                    self.add_system_message(format!("pinned message #{}", last_idx));
                } else {
                    self.add_system_message("last message already pinned");
                }
            }
            CommandId::Unpin => {
                if let Some(idx) = self.pinned_messages.pop() {
                    self.add_system_message(format!("unpinned message #{}", idx));
                } else {
                    self.add_system_message("no pinned messages");
                }
            }
            CommandId::Stream => {
                self.add_system_message(
                    "streaming is enabled for all non-tool-use prompts. responses appear character-by-character.",
                );
            }
            CommandId::Run => {
                if args.is_empty() {
                    self.add_system_message("usage: /run <command>   (or !<command>)");
                    return;
                }
                self.start_shell(args.to_string());
            }
            CommandId::Curl => {
                if args.is_empty() {
                    self.add_system_message("usage: /curl <args>");
                    return;
                }
                self.start_shell(format!("curl {}", args));
            }
            CommandId::Brew => {
                if args.is_empty() {
                    self.add_system_message("usage: /brew <args>");
                    return;
                }
                self.start_shell(format!("brew {}", args));
            }
        }
    }

    fn start_ai(&mut self, input: String) {
        if self.pending_ai || !self.reveal_queue.is_empty() {
            self.add_system_message("output pipeline busy: wait for the current reveal to complete before sending a new model prompt");
            return;
        }

        if self.provider == AIProvider::Ollama && self.ollama_selected_model.is_none() {
            self.show_ollama_picker = true;
            self.status_note = "select an ollama model first".to_string();
            self.add_system_message(
                "Ollama is active, but no model is selected yet. Type a model number in the picker and press Enter.",
            );
            return;
        }

        let enriched_input = self.inject_file_references(&input);

        let message = ChatMessage::user(enriched_input.clone());
        self.persist(&self.provider, "user", "chat", &input);
        self.messages.push(message);
        self.pending_ai = true;
        self.streaming_active = true;
        self.tool_loop_depth = 0;
        self.status_note = format!("streaming -> {}", self.provider_status_badge());

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
        // A generous SQUARE-pixel box for the local view; the renderer scales it
        // into whatever panel the webcam lands in (these are pixels, not cells).
        webcam::WebcamConfig {
            width: 320,
            height: 240,
            ..webcam::WebcamConfig::default()
        }
    }

    /// Cycle the video render mode (glyph <-> half-block <-> true pixels),
    /// skipping Pixel unless a terminal graphics protocol was detected.
    fn cycle_video_mode(&mut self) {
        let mut next = self.video_render_mode.cycle();
        if next == render::VideoRenderMode::Pixel && !self.pixel_protocol_available() {
            next = next.cycle();
        }
        self.video_render_mode = next;
        self.status_note = format!("video render: {}", next.label());
        // switching INTO pixel mode: encode the current frames now so the toggle
        // is immediate instead of waiting for the next decoded frame
        if next == render::VideoRenderMode::Pixel {
            self.rebuild_video_proto();
            self.rebuild_screen_proto();
        }
    }

    /// True when a real pixel graphics protocol is available (else half-block).
    fn pixel_protocol_available(&self) -> bool {
        self.hw_graphics
    }

    /// Store the startup graphics probe. Called once, after alt-screen, before
    /// the event loop begins.
    fn attach_graphics(&mut self, picker: ratatui_image::picker::Picker, hw_graphics: bool) {
        self.picker = Some(picker);
        self.hw_graphics = hw_graphics;
    }

    /// True while the video / screen-share panels should blit real pixels.
    fn pixel_active(&self) -> bool {
        self.hw_graphics && self.video_render_mode == render::VideoRenderMode::Pixel
    }

    /// Rebuild the video panel's pixel protocol from the latest decoded frame.
    /// Cheap: it just stores the image; the resize+encode is lazy at render.
    fn rebuild_video_proto(&self) {
        if !self.pixel_active() {
            return;
        }
        if let (Some(picker), Some(frame)) = (
            self.picker.as_ref(),
            self.video.as_ref().and_then(|v| v.latest_frame()),
        ) {
            if let Some(img) = gfx::frame_to_dynamic(frame) {
                *self.video_proto.borrow_mut() = Some(picker.new_resize_protocol(img));
            }
        }
    }

    /// Rebuild the screen-share panel's pixel protocol from the latest frame.
    fn rebuild_screen_proto(&self) {
        if !self.pixel_active() {
            return;
        }
        if let (Some(picker), Some(frame)) = (self.picker.as_ref(), self.screen_frame.as_ref()) {
            if let Some(img) = gfx::frame_to_dynamic(frame) {
                *self.screen_proto.borrow_mut() = Some(picker.new_resize_protocol(img));
            }
        }
    }

    /// Toggle screen sharing. Starts a desktop capture rendered locally in the
    /// SCREEN SHARE panel and -- when connected to a room -- streamed into it as
    /// this client's feed (the camera is muted for the duration).
    fn toggle_screenshare(&mut self) {
        if self.screenshare.is_some() {
            self.screenshare = None;
            self.screen_frame = None;
            self.sharing_screen = false;
            if let Some(vc) = &self.video_chat {
                vc.set_webcam_enabled(true); // hand the room feed back to the camera
            }
            self.status_note = "screen share stopped".to_string();
            return;
        }
        match WebcamCapture::start(self.screenshare_config()) {
            Ok(cap) => {
                self.screenshare = Some(cap);
                self.sharing_screen = true;
                self.tiling.set_focused_panel(PanelKind::ScreenShare);
                let live = self
                    .video_chat
                    .as_ref()
                    .map_or(false, |c| c.is_connected());
                if live {
                    if let Some(vc) = &self.video_chat {
                        vc.set_webcam_enabled(false); // App now drives the outgoing feed
                    }
                    self.status_note = "screen share LIVE -> room".to_string();
                } else {
                    self.status_note = "screen share preview (join a room to share)".to_string();
                }
            }
            Err(e) => {
                self.add_system_message(format!("screen share failed: {}", e));
                self.status_note = "screen share failed (see transcript)".to_string();
            }
        }
    }

    fn screenshare_config(&self) -> webcam::WebcamConfig {
        // Screens carry fine detail (text/UI) so favor resolution, but keep the
        // frame rate low -- flat UI regions compress well and the wire stays light.
        webcam::WebcamConfig {
            device: resolve_screen_device(),
            width: 480,
            height: 270,
            fps_cap: 12,
            source: webcam::CaptureSource::Screen,
        }
    }

    fn add_system_message(&mut self, content: impl Into<String>) {
        let message = ChatMessage::system(content);
        self.messages.push(message);
    }

    /// Games <-> network glue, run every tick.
    ///
    /// While a video chat connection is live, GamesPanel holds an outbound
    /// `(game, payload)` sender; this method (1) wires/unwires that sender as
    /// the connection comes and goes, (2) refreshes my_id when the server's
    /// Welcome lands after the link flips connected, (3) pumps outbound game
    /// payloads onto the wire, and (4) drains the inbound game inbox into
    /// GamesPanel::handle_net.
    fn sync_game_net(&mut self) {
        let connected = self
            .video_chat
            .as_ref()
            .map_or(false, |vc| vc.is_connected());

        if connected {
            let my_id = self.video_chat.as_ref().and_then(|vc| vc.my_id());
            if self.game_net_tx.is_none() {
                let (tx, rx) = mpsc::unbounded_channel();
                self.games.set_net(Some(tx.clone()), my_id.clone());
                self.game_net_tx = Some(tx);
                self.game_net_rx = Some(rx);
                self.game_net_my_id = my_id;
            } else if my_id != self.game_net_my_id {
                // Welcome (our id) arrived after connect: refresh in place,
                // keeping the existing channel so no queued payload is lost.
                self.games.set_net(self.game_net_tx.clone(), my_id.clone());
                self.game_net_my_id = my_id;
            }
        } else if self.game_net_tx.is_some() {
            // finding #4: the link just died (connection lost, kicked, or
            // /disconnect) -- end any online game session instead of leaving
            // a zombie match simulating a vanished opponent.
            self.games.peer_disconnected(None);
            self.games.set_net(None, None);
            self.game_net_tx = None;
            self.game_net_rx = None;
            self.game_net_my_id = None;
        }

        // outbound: games -> wire
        if let Some(rx) = self.game_net_rx.as_mut() {
            while let Ok((game, payload)) = rx.try_recv() {
                if let Some(vc) = &self.video_chat {
                    vc.send_game(&game, payload);
                }
            }
        }

        // inbound: wire -> games (drain even when no session is running so
        // lobby invites surface in the selector)
        let inbound = self
            .video_chat
            .as_ref()
            .map(|vc| vc.drain_game_inbox())
            .unwrap_or_default();
        for msg in inbound {
            self.games
                .handle_net(&msg.from_id, &msg.from_name, &msg.game, &msg.payload);
        }
    }

    /// Adapt VideoChatClient lifecycle events into AppEvents so the tick
    /// loop can surface them via status_note + transcript (never eprintln!).
    fn net_event_hook(&self) -> impl Fn(NetEvent) + Send + Sync + 'static {
        let events_tx = self.events_tx.clone();
        move |event| {
            let app_event = match event {
                NetEvent::Connected { user_id } => AppEvent::NetStatus {
                    message: format!(
                        "link established (id {})",
                        user_id.chars().take(8).collect::<String>()
                    ),
                },
                NetEvent::Disconnected { reason } => AppEvent::NetStatus { message: reason },
                NetEvent::UserJoined { username } => AppEvent::NetUserJoined { username },
                NetEvent::UserLeft { user_id, username } => {
                    AppEvent::NetUserLeft { user_id, username }
                }
            };
            let _ = events_tx.send(app_event);
        }
    }

    /// Create, store, and actually connect a video chat client. The stored
    /// Arc and the connecting instance are one and the same (this replaces
    /// the old broken temp-client pattern).
    fn start_video_client(&mut self, url: String) {
        // Finding #6: never overwrite a stored client without killing it
        // first. A previous dial can still be in flight (is_connected() is
        // false the whole time), and its connect task holds its own Arc --
        // left alone it would eventually join the room as an orphaned ghost
        // (4 leaked tasks + a second webcam capture + duplicate presence).
        if let Some(old) = self.video_chat.take() {
            old.disconnect();
            self.add_system_message("replacing the previous connection attempt");
        }
        let client = Arc::new(VideoChatClient::new(self.username.clone(), url));
        client.set_event_hook(self.net_event_hook());
        // If a screen share is already active, the App drives the outgoing feed
        // (desktop frames via tick), so the client's camera must be muted from
        // its very first frame. A fresh client defaults webcam_enabled=true, so
        // without this a /screenshare-then-/join (or a reconnect while sharing)
        // would broadcast BOTH the webcam and the screen -- a privacy leak.
        client.set_webcam_enabled(!self.sharing_screen);
        self.video_chat = Some(Arc::clone(&client));
        let events_tx = self.events_tx.clone();
        let dial_generation = Arc::clone(&self.dial_generation);
        let my_generation = dial_generation.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        tokio::spawn(async move {
            // a couple of quick retries cover the /host self-connect racing
            // the server's accept loop startup
            let mut attempt = 0;
            loop {
                // superseded by a newer dial or /disconnect? stop, don't
                // resurrect a replaced client (finding #6)
                if dial_generation.load(AtomicOrdering::SeqCst) != my_generation {
                    client.disconnect();
                    break;
                }
                match Arc::clone(&client).connect().await {
                    Ok(()) => {
                        // the dial itself cannot be cancelled mid-await; if
                        // we were superseded while it ran, tear it down now
                        if dial_generation.load(AtomicOrdering::SeqCst) != my_generation {
                            client.disconnect();
                        }
                        break;
                    }
                    Err(error) => {
                        attempt += 1;
                        if attempt >= 3 {
                            let _ = events_tx.send(AppEvent::NetStatus {
                                message: format!("connection failed: {}", error),
                            });
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                }
            }
        });
        // Room view: feeds + chat stream + roster, transcript kept visible.
        self.tiling.apply_preset(LayoutPreset::VideoChat);
        self.status_note = "video chat connecting...".to_string();
    }

    /// Bind and spawn the video chat server. Binding happens synchronously
    /// so "port busy" is reported immediately and a follow-up self-connect
    /// cannot race the bind. Returns false if it did not start.
    fn start_chat_server(&mut self, port: u16) -> bool {
        if self.chat_server.is_some() {
            let running = self.chat_server_port.unwrap_or(port);
            self.add_system_message(format!(
                "video chat server already running on :{} -- /invite to reprint the room code",
                running
            ));
            self.status_note = format!("server already on :{}", running);
            return false;
        }
        match std::net::TcpListener::bind(("0.0.0.0", port)) {
            Ok(listener) => {
                let server = Arc::new(VideoChatServer::new());
                self.chat_server = Some(Arc::clone(&server));
                self.chat_server_port = Some(port);
                let events_tx = self.events_tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = server.run_std(listener).await {
                        let _ = events_tx.send(AppEvent::NetStatus {
                            message: format!("server error: {}", error),
                        });
                    }
                });
                true
            }
            Err(error) => {
                self.add_system_message(format!(
                    "could not bind video chat server to port {}: {}",
                    port, error
                ));
                self.status_note = "server bind failed".to_string();
                false
            }
        }
    }

    /// Print the shareable room block: code, /join line, and raw ws:// URL.
    fn print_room_invite(&mut self) {
        let addr = if let Some(port) = self.chat_server_port {
            Some(SocketAddrV4::new(roomcode::lan_ip(), port))
        } else if let Some(ref vc) = self.video_chat {
            roomcode::parse_ws_url(&vc.server_url).map(|a| {
                if a.ip().is_loopback() {
                    SocketAddrV4::new(roomcode::lan_ip(), a.port())
                } else {
                    a
                }
            })
        } else {
            None
        };
        match addr {
            Some(addr) => {
                let code = roomcode::encode(addr);
                self.add_system_message("================= ROOM OPEN =================");
                self.add_system_message(format!("  ROOM CODE: {}", code));
                self.add_system_message(format!("  friend on the same WiFi runs: /join {}", code));
                self.add_system_message(format!("  or direct: /connect ws://{}", addr));
                self.add_system_message("=============================================");
                self.status_note = format!("room code: {}", code);
            }
            None => {
                self.add_system_message("no room to invite anyone to -- /host to open one");
            }
        }
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
            .title_style(Style::default().fg(t().accent4).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t().accent1));
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
                .title_style(Style::default().fg(t().accent2).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t().accent3));
            frame.render_widget(shell, video_area);
            video.render(
                frame,
                video_area.inner(Margin {
                    horizontal: 1,
                    vertical: 1,
                }),
                0.95,
                self.video_render_mode,
            );
        }

        let burst_x = inner.x + inner.width.saturating_mul(22) / 100;
        let burst_y = inner.y + inner.height.saturating_mul(34) / 100;
        render_starburst(frame.buffer_mut(), burst_x, burst_y, 12, phase, Some(inner));

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
                Span::styled("v2.0.0", Style::default().fg(t().muted)),
                Span::styled("  (POWERHOUSE)  ", Style::default().fg(t().text)),
                Span::styled("//", Style::default().fg(t().accent1)),
                Span::styled(
                    "  ALL-IN-ONE TERMINAL COMMAND CENTER",
                    Style::default().fg(t().accent4).bold(),
                ),
            ]),
            Line::from(vec![
                Span::styled("STACK:", Style::default().fg(t().accent2).bold()),
                Span::styled(
                    " ASCII VIDEO | MULTI-AI | LIVE BASH | WEBCAM | 3D FX | VIDEO CHAT | ANALYTICS",
                    Style::default().fg(t().text),
                ),
            ]),
            Line::from(vec![
                Span::styled("STATE:", Style::default().fg(t().accent2).bold()),
                Span::styled(
                    " cracktro boot stream -> auto-transitions into the full command deck",
                    Style::default().fg(t().text),
                ),
            ]),
            Line::from(vec![
                Span::styled("INPUT:", Style::default().fg(t().accent2).bold()),
                Span::styled(
                    " ENTER / SPACE skips intro immediately",
                    Style::default().fg(t().text),
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
                .style(Style::default().bg(t().panel_bg)),
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
            t().accent4,
        );
    }

    fn render_chat(&mut self, frame: &mut Frame, area: Rect, phase: f32) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),
                Constraint::Min(14),
                Constraint::Length(4),
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

        // Effects render ONLY inside the dedicated Effects3D tile panel,
        // not as a global overlay (which would overwrite all other panels).

        self.render_input(frame, layout[2]);
        render_scroller(frame.buffer_mut(), layout[3], SCROLLER_TEXT, phase, t().accent1);

        if self.show_help {
            self.render_help_overlay(frame, area);
        }

        if self.show_ollama_picker {
            self.render_ollama_overlay(frame, area);
        }

        // topmost overlay: the command palette
        self.palette.render(frame, area);
    }

    fn render_tile_panel(
        &mut self,
        frame: &mut Frame,
        panel: PanelKind,
        area: Rect,
        phase: f32,
        is_focused: bool,
    ) {
        if area.width < 8 || area.height < 4 {
            return;
        }
        // solid background fill: kill ghost artifacts from previous frames
        frame.render_widget(
            Block::default().style(Style::default().bg(t().panel_bg)),
            area,
        );
        match panel {
            PanelKind::Transcript => self.render_messages_tile(frame, area, is_focused),
            PanelKind::Games => self.games.render(frame, area, phase, is_focused),
            PanelKind::Tiles => self.tiles.render(frame, area, is_focused),
            PanelKind::Video => self.render_video_panel(frame, area, phase),
            PanelKind::Webcam => self.render_webcam_panel(frame, area, phase),
            PanelKind::ScreenShare => self.render_screenshare_panel(frame, area, phase),
            PanelKind::Telemetry => self.render_telemetry(frame, area, phase),
            PanelKind::OpsDeck => self.render_ops_panel(frame, area, phase),
            PanelKind::Effects3D => {
                let title = if self.effects.active {
                    format!(" 3D EFFECTS // {} ", self.effects.current_name())
                } else {
                    " 3D EFFECTS // OFFLINE ".to_string()
                };
                let block = Block::default()
                    .title(title)
                    .title_style(Style::default().fg(t().accent2).bold())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if is_focused { t().accent4 } else { t().accent1 }));
                frame.render_widget(block, area);
                let inner = area.inner(Margin { horizontal: 1, vertical: 1 });
                if self.effects.active {
                    self.effects.render(frame.buffer_mut(), inner, phase);
                } else {
                    frame.render_widget(
                        Paragraph::new("F4 to cycle effects (includes off)")
                            .style(Style::default().fg(t().muted).bg(t().panel_bg))
                            .alignment(Alignment::Center),
                        inner,
                    );
                }
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
        let border_color = if is_focused { t().accent4 } else { t().accent3 };
        let block = Block::default()
            .title(" TRANSCRIPT ")
            .title_style(Style::default().fg(t().accent4).bold())
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
            .title_style(Style::default().fg(t().accent1).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t().accent1));
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
            Some(inner),
        );
        render_gradient_text(
            frame.buffer_mut(),
            inner.x + 10,
            inner.y,
            "ASCIIVISION v2 // ALL-IN-ONE TERMINAL POWERHOUSE",
            t().accent1,
            t().accent4,
        );

        let fx_tag = if self.effects.active {
            format!("fx:{}", self.effects.current_name())
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
            self.provider_status_badge(),
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
            t().text,
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
            t().accent2,
            t().text,
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
                Span::styled(format!("[{}]", tag), Style::default().fg(t().accent2)),
            ]));

            if message.content.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  . . .",
                    Style::default().fg(t().muted),
                )));
            } else {
                for content_line in message.content.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", content_line),
                        Style::default().fg(match message.kind {
                            MessageKind::System => Color::Rgb(171, 183, 192),
                            _ => t().text,
                        }),
                    )));
                }
            }
            lines.push(Line::from(""));
        }

        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "No traffic yet. Ask the model something, !shell, /webcam, /3d, /server, or /connect.",
                Style::default().fg(t().muted),
            )));
        }

        // Estimate wrapped line count so scroll doesn't overshoot
        let wrap_width = inner.width.max(1) as usize;
        let total_wrapped: usize = lines.iter().map(|l| {
            let w = l.width();
            if w == 0 { 1 } else { (w + wrap_width - 1) / wrap_width }
        }).sum();
        let total_lines = total_wrapped.max(1);
        let visible_lines = inner.height as usize;
        let max_scroll = total_lines.saturating_sub(visible_lines);
        let scroll = if self.follow_tail {
            max_scroll
        } else {
            self.scroll_lines.min(max_scroll)
        };

        let widget = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0))
            .style(Style::default().bg(t().panel_bg));
        frame.render_widget(widget, inner);
    }

    fn render_video_panel(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let title = if self.video_enabled {
            if self.pending_video_load {
                " LIVE VIDEO BUS // YOUTUBE LOADING "
            } else {
                " LIVE VIDEO BUS "
            }
        } else {
            " SYNTHETIC FIELD "
        };
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(t().accent2).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t().accent1));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if self.video_enabled {
            if let Some(video) = &self.video {
                // true-pixel path: blit the decoded frame via the terminal
                // graphics protocol (image widget punches its own hole in the
                // buffer). Falls through to half-block if not yet encoded.
                if self.pixel_active() {
                    let mut guard = self.video_proto.borrow_mut();
                    if let Some(proto) = guard.as_mut() {
                        frame.render_stateful_widget(
                            ratatui_image::StatefulImage::default()
                                .resize(ratatui_image::Resize::Fit(None)),
                            inner,
                            proto,
                        );
                        return;
                    }
                }
                video.render(frame, inner, 0.92, self.video_render_mode);
                let meta = format!(
                    "sig:{}  source:{}",
                    if video.has_signal() { "lock" } else { "seek" },
                    truncate(&self.video_source_label, 22)
                );
                render_gradient_text(frame.buffer_mut(), inner.x + 1, inner.y, &meta, t().accent4, t().text);
                return;
            }
        }

        if self.pending_video_load {
            frame.render_widget(
                Paragraph::new("yt-dlp is resolving a playable YouTube stream...\n\nWhen the stream handshake completes, this panel will switch over automatically.")
                    .style(Style::default().fg(t().accent4).bg(t().panel_bg))
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: false }),
                inner,
            );
            return;
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
            .title_style(Style::default().fg(t().accent4).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t().accent3));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if let Some(ref rgb) = self.webcam_frame {
            render::render_frame(frame.buffer_mut(), inner, rgb, 0.9, self.video_render_mode);
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
                t().danger
            } else {
                t().muted
            };
            frame.render_widget(
                Paragraph::new(msg)
                    .style(Style::default().fg(color).bg(t().panel_bg))
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }
    }

    fn render_screenshare_panel(&self, frame: &mut Frame, area: Rect, _phase: f32) {
        let connected = self.video_chat.as_ref().map_or(false, |c| c.is_connected());
        let title = if self.screenshare.is_some() {
            if self.sharing_screen && connected {
                " SCREEN SHARE // LIVE -> ROOM "
            } else {
                " SCREEN SHARE // PREVIEW "
            }
        } else {
            " SCREEN SHARE // OFFLINE "
        };
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(t().accent2).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t().accent1));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if self.pixel_active() {
            let mut guard = self.screen_proto.borrow_mut();
            if let Some(proto) = guard.as_mut() {
                frame.render_stateful_widget(
                    ratatui_image::StatefulImage::default().resize(ratatui_image::Resize::Fit(None)),
                    inner,
                    proto,
                );
                return;
            }
        }
        if let Some(ref rgb) = self.screen_frame {
            render::render_frame(frame.buffer_mut(), inner, rgb, 0.95, self.video_render_mode);
        } else {
            let has_err = self.screenshare.as_ref().and_then(|c| c.error()).is_some();
            let msg = if let Some(ref cam) = self.screenshare {
                if let Some(err) = cam.error() {
                    format!(
                        "SCREEN CAPTURE ERROR: {}\n\nmacOS: grant Screen Recording to your terminal in\nSystem Settings > Privacy & Security > Screen Recording,\nthen restart asciivision.",
                        err
                    )
                } else {
                    "acquiring display...\n(macOS may prompt for Screen Recording permission)".to_string()
                }
            } else {
                "/screenshare to broadcast your desktop as live ASCII".to_string()
            };
            let color = if has_err { t().danger } else { t().muted };
            frame.render_widget(
                Paragraph::new(msg)
                    .style(Style::default().fg(color).bg(t().panel_bg))
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }
    }

    fn render_telemetry(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let block = Block::default()
            .title(" TELEMETRY ")
            .title_style(Style::default().fg(t().accent4).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t().accent3));
        frame.render_widget(block, area);
        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        let lines = vec![
            Line::from(vec![
                Span::styled("provider: ", Style::default().fg(t().accent2).bold()),
                Span::styled(
                    self.provider_display_name(),
                    Style::default().fg(self.provider.color()),
                ),
            ]),
            Line::from(vec![
                Span::styled("status:   ", Style::default().fg(t().accent2).bold()),
                Span::styled(
                    if self.pending_ai {
                        "awaiting model response"
                    } else {
                        "terminal steady"
                    },
                    Style::default().fg(t().text),
                ),
            ]),
            Line::from(vec![
                Span::styled("shell:    ", Style::default().fg(t().accent2).bold()),
                Span::styled(&self.last_shell_status, Style::default().fg(t().text)),
            ]),
            Line::from(vec![
                Span::styled("3d fx:    ", Style::default().fg(t().accent2).bold()),
                Span::styled(
                    if self.effects.active {
                        self.effects.current_name()
                    } else {
                        "offline"
                    },
                    Style::default().fg(if self.effects.active { t().accent4 } else { t().muted }),
                ),
            ]),
            Line::from(vec![
                Span::styled("webcam:   ", Style::default().fg(t().accent2).bold()),
                Span::styled(
                    if self.webcam.is_some() { "active" } else { "offline" },
                    Style::default().fg(if self.webcam.is_some() { t().accent3 } else { t().muted }),
                ),
            ]),
        ];

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().bg(t().panel_alt))
                .wrap(Wrap { trim: false }),
            inner,
        );

        if inner.height > 7 {
            let eq_area = Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(2),
                width: inner.width,
                height: 2.min(inner.height.saturating_sub(5)),
            };
            render_equalizer(frame.buffer_mut(), eq_area, phase);
        }
    }

    fn render_ops_panel(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let block = Block::default()
            .title(" OPS DECK ")
            .title_style(Style::default().fg(t().accent1).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t().accent1));
        frame.render_widget(block, area);
        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        // cheatsheet: featured commands straight from the registry
        let mut lines = vec![Line::from(vec![
            Span::styled("Ctrl+P", Style::default().fg(t().accent2).bold()),
            Span::styled(" palette   ", Style::default().fg(t().text)),
            Span::styled("!<cmd>", Style::default().fg(t().accent2).bold()),
            Span::styled(" raw shell", Style::default().fg(t().text)),
        ])];
        for spec in commands::featured() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<18}", truncate(spec.usage, 18)),
                    Style::default().fg(t().accent4),
                ),
                Span::styled(
                    truncate(spec.description, inner.width.saturating_sub(19) as usize),
                    Style::default().fg(t().text),
                ),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "recent ops:",
            Style::default().fg(t().accent2).bold(),
        )));

        if self.recent_commands.is_empty() {
            lines.push(Line::from(Span::styled(
                "  none yet",
                Style::default().fg(t().muted),
            )));
        } else {
            for command in &self.recent_commands {
                lines.push(Line::from(Span::styled(
                    format!(
                        "  {}",
                        truncate(command, inner.width.saturating_sub(4) as usize)
                    ),
                    Style::default().fg(t().accent4),
                )));
            }
        }

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().bg(t().panel_bg))
                .wrap(Wrap { trim: false }),
            inner,
        );

        let pulse_x = inner.x + inner.width.saturating_sub(8);
        render_starburst(frame.buffer_mut(), pulse_x, inner.y + 1, 2, phase * 1.7, Some(inner));
    }

    fn render_videochat_feeds(&self, frame: &mut Frame, area: Rect, _phase: f32) {
        let block = Block::default()
            .title(" VIDEO CHAT // LIVE FEEDS ")
            .title_style(Style::default().fg(t().accent2).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t().accent1));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if let Some(ref vc) = self.video_chat {
            let remote = vc.remote_frames.read();
            let local = vc.local_frame.read();

            // Finding #9: the v3 server never echoes our own frames back, so
            // the self-view must come from local_frame. It is ALWAYS tile 0
            // ("you"); remote feeds fill the remaining tiles, capped at 4
            // total. Feeds beyond the grid are summarized as "+N more".
            let mut tiles: Vec<(String, &render::RgbFrame, bool)> = Vec::new();
            if let Some(ref own) = *local {
                tiles.push((format!("{} (you)", self.username), own, true));
            }
            let mut remotes: Vec<(&String, &String, &render::RgbFrame)> = remote
                .iter()
                .map(|(uid, (uname, rgb))| (uname, uid, rgb))
                .collect();
            // stable grid order (HashMap iteration order must not decide
            // which feeds are shown when more than 4 are live)
            remotes.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(b.1)));
            let mut overflow = 0usize;
            for (uname, _uid, ascii) in remotes {
                if tiles.len() < 4 {
                    tiles.push((uname.clone(), ascii, false));
                } else {
                    overflow += 1;
                }
            }

            if tiles.is_empty() {
                frame.render_widget(
                    Paragraph::new("waiting for video feeds...")
                        .style(Style::default().fg(t().muted).bg(t().panel_bg))
                        .alignment(Alignment::Center),
                    inner,
                );
            } else {
                let count = tiles.len();
                let cols = if count <= 2 { count } else { 2 };
                let rows = (count + cols - 1) / cols;

                let row_constraints: Vec<Constraint> = (0..rows)
                    .map(|_| Constraint::Percentage((100 / rows) as u16))
                    .collect();
                let row_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(row_constraints)
                    .split(inner);

                let mut tile_iter = tiles.iter();
                for r in 0..rows {
                    let col_constraints: Vec<Constraint> = (0..cols)
                        .map(|_| Constraint::Percentage((100 / cols) as u16))
                        .collect();
                    let col_layout = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints(col_constraints)
                        .split(row_layout[r]);

                    for c in 0..cols {
                        if let Some((label, rgb_frame, is_self)) = tile_iter.next() {
                            let cell_area = col_layout[c];
                            render::render_frame(
                                frame.buffer_mut(),
                                cell_area,
                                rgb_frame,
                                if *is_self { 0.9 } else { 0.85 },
                                self.video_render_mode,
                            );
                            render_gradient_text(
                                frame.buffer_mut(),
                                cell_area.x + 1,
                                cell_area.y,
                                label,
                                if *is_self { t().accent3 } else { t().accent4 },
                                t().text,
                            );
                        }
                    }
                }

                if overflow > 0 {
                    render_gradient_text(
                        frame.buffer_mut(),
                        inner.x + 1,
                        inner.y + inner.height.saturating_sub(1),
                        &format!(
                            "+{} more feed{} off-grid",
                            overflow,
                            if overflow == 1 { "" } else { "s" }
                        ),
                        t().accent1,
                        t().text,
                    );
                }
            }
        } else {
            frame.render_widget(
                Paragraph::new("not connected. use /server <port> or /connect ws://<addr>")
                    .style(Style::default().fg(t().muted).bg(t().panel_bg))
                    .alignment(Alignment::Center),
                inner,
            );
        }
    }

    fn render_videochat_messages(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" CHAT STREAM ")
            .title_style(Style::default().fg(t().accent4).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t().accent3));
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
                    t().muted
                } else if uname == &self.username {
                    t().accent3
                } else {
                    t().accent4
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", uname), Style::default().fg(color).bold()),
                    Span::styled(content.as_str(), Style::default().fg(t().text)),
                ]));
            }
            if lines.is_empty() {
                lines.push(Line::from(Span::styled(
                    "no messages yet. use /chat <msg>",
                    Style::default().fg(t().muted),
                )));
            }
            frame.render_widget(
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().bg(t().panel_bg)),
                inner,
            );
        } else {
            frame.render_widget(
                Paragraph::new("offline")
                    .style(Style::default().fg(t().muted).bg(t().panel_bg))
                    .alignment(Alignment::Center),
                inner,
            );
        }
    }

    fn render_videochat_users(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let block = Block::default()
            .title(" CONNECTED USERS ")
            .title_style(Style::default().fg(t().accent2).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t().accent1));
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
                    Span::styled("status: ", Style::default().fg(t().accent2).bold()),
                    Span::styled(status, Style::default().fg(t().text)),
                ]),
            ];
            if users.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  no users yet",
                    Style::default().fg(t().muted),
                )));
            } else {
                let my_id = vc.my_id();
                for u in users.iter() {
                    let indicator = current_spinner(phase);
                    let is_self = my_id.as_deref() == Some(u.user_id.as_str());
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  [{}] ", indicator),
                            Style::default().fg(if is_self { t().accent3 } else { t().accent4 }),
                        ),
                        Span::styled(
                            u.username.as_str(),
                            Style::default()
                                .fg(if is_self { t().accent3 } else { t().text })
                                .bold(),
                        ),
                        if is_self {
                            Span::styled(" (you)", Style::default().fg(t().muted))
                        } else {
                            Span::raw("")
                        },
                    ]));
                }
            }
            frame.render_widget(
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().bg(t().panel_bg)),
                inner,
            );
        } else {
            frame.render_widget(
                Paragraph::new("offline")
                    .style(Style::default().fg(t().muted).bg(t().panel_bg))
                    .alignment(Alignment::Center),
                inner,
            );
        }
    }

    fn render_input(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" TRANSMIT ")
            .title_style(Style::default().fg(t().accent4).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t().accent3))
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
            t().accent2
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
                Span::styled("> ", Style::default().fg(t().accent4).bold()),
                Span::styled(
                    if self.input.is_empty() {
                        "Ctrl+P palette // prompt, !bash, @file, /host, /games, /fx ..."
                    } else {
                        self.input.as_str()
                    },
                    Style::default().fg(t().text),
                ),
                Span::styled("_", Style::default().fg(t().accent2).bold()),
            ]),
            Line::from(vec![
                Span::styled("mode: ", Style::default().fg(t().accent2).bold()),
                Span::styled(status, Style::default().fg(status_color)),
                Span::styled(
                    format!("  |  {}  |  ctrl+p palette  F1 help  F2 ai  F4 fx  F5 cam  F6 layout  F7 tiles", trust_tag),
                    Style::default().fg(t().muted),
                ),
            ]),
        ];

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().bg(t().panel_alt))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn render_ollama_overlay(&self, frame: &mut Frame, area: Rect) {
        let popup = centered_area(area, 74, 72);
        frame.render_widget(Clear, popup);
        let title = if self.ollama_picker_loading {
            " OLLAMA // DISCOVERING LOCAL MODELS "
        } else {
            " OLLAMA // MODEL PICKER "
        };
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(self.provider.color()).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t().accent1));
        frame.render_widget(block, popup);

        let inner = popup.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        let current_model = self
            .ollama_selected_model
            .clone()
            .unwrap_or_else(|| "none selected".to_string());
        let mut lines = vec![
            Line::from(vec![
                Span::styled("MODEL SELECT ", Style::default().fg(t().accent2).bold()),
                Span::styled(
                    "Type a number and press Enter. Esc closes. R refreshes. J/K or arrows scroll.",
                    Style::default().fg(t().text),
                ),
            ]),
            Line::from(vec![
                Span::styled("current: ", Style::default().fg(t().accent2).bold()),
                Span::styled(current_model, Style::default().fg(self.provider.color())),
            ]),
            Line::from(""),
        ];

        if self.ollama_picker_loading {
            lines.push(Line::from(Span::styled(
                "Scanning the local Ollama API for installed models...",
                Style::default().fg(t().accent4),
            )));
        } else if let Some(error) = &self.ollama_picker_error {
            lines.push(Line::from(Span::styled(
                "Ollama is not ready on this machine:",
                Style::default().fg(t().danger).bold(),
            )));
            lines.push(Line::from(Span::styled(
                error,
                Style::default().fg(t().text),
            )));
            if error.contains("not installed") {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    ollama_install_hint(),
                    Style::default().fg(t().accent4),
                )));
            }
        } else {
            for (idx, model) in self.ollama_models.iter().enumerate() {
                let marker = if self
                    .ollama_selected_model
                    .as_ref()
                    .map(|selected| selected == &model.name)
                    .unwrap_or(false)
                {
                    ">"
                } else {
                    " "
                };
                let meta = format_ollama_model_meta(model);
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{} {:>2}. ", marker, idx + 1),
                        Style::default().fg(t().accent2).bold(),
                    ),
                    Span::styled(model.name.clone(), Style::default().fg(t().text)),
                    Span::styled(
                        format!("  [{}]", meta),
                        Style::default().fg(t().muted),
                    ),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("selection: ", Style::default().fg(t().accent2).bold()),
            Span::styled(
                if self.ollama_selection_input.is_empty() {
                    "_".to_string()
                } else {
                    self.ollama_selection_input.clone()
                },
                Style::default().fg(self.provider.color()),
            ),
        ]));

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .scroll((self.ollama_picker_scroll as u16, 0))
                .style(Style::default().bg(t().panel_bg)),
            inner,
        );
    }

    /// Help overlay, generated from the command registry (grouped by
    /// category) plus a KEYS section and the live effects/games rosters.
    /// Scrollable: PgUp/PgDn/arrows/j/k while open, Esc or F1 closes.
    fn render_help_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_area(area, 78, 80);
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .title(" HELP // ASCIIVISION v3 OPERATIONS MANUAL ")
            .title_style(Style::default().fg(t().accent1).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t().accent1));
        frame.render_widget(block, popup);

        let header = |label: &str| {
            Line::from(Span::styled(
                label.to_string(),
                Style::default().fg(t().accent4).bold(),
            ))
        };
        let entry = |usage: &str, description: &str| {
            Line::from(vec![
                Span::styled(
                    format!("  {:<26} ", usage),
                    Style::default().fg(t().accent2).bold(),
                ),
                Span::styled(description.to_string(), Style::default().fg(t().text)),
            ])
        };

        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled(
                    "Ctrl+P opens the command palette",
                    Style::default().fg(t().accent2).bold(),
                ),
                Span::styled(
                    " — fuzzy-search everything below. Plain text goes to the AI; !<cmd> runs shell.",
                    Style::default().fg(t().text),
                ),
            ]),
            Line::from(Span::styled(
                "PgUp/PgDn, arrows, or j/k scroll this manual. Esc or F1 closes it.",
                Style::default().fg(t().muted),
            )),
            Line::from(""),
        ];

        // command sections, straight from the registry
        for category in commands::Category::ALL {
            lines.push(header(category.name()));
            for spec in commands::in_category(category) {
                let description = if spec.aliases.is_empty() {
                    spec.description.to_string()
                } else {
                    format!("{} (alias {})", spec.description, spec.aliases.join(", "))
                };
                lines.push(entry(spec.usage, &description));
            }
            lines.push(Line::from(""));
        }

        lines.push(header("KEYS"));
        for (key, what) in [
            ("Ctrl+P", "command palette (fuzzy search everything)"),
            (
                "Ctrl+P (in Tiles)",
                "still the palette, even in a PTY tile -- use Up arrow for shell history",
            ),
            ("F1", "toggle this manual"),
            ("F2", "cycle AI provider (Claude, Grok, GPT-5, Gemini, Ollama)"),
            ("F3", "toggle live video panel"),
            ("F4", "cycle 3D effects (includes off)"),
            ("F5", "toggle webcam capture"),
            ("F6", "cycle tiling layout preset"),
            ("F7", "boot/focus the Tiles PTY panel"),
            ("F8", "cycle focused tile panel type"),
            ("F9", "randomize color theme"),
            ("F10", "reset theme to defaults"),
            ("Ctrl+L", "clear transcript"),
            ("PgUp/PgDn", "scroll transcript"),
            ("Esc Esc", "quit (press twice within half a second)"),
        ] {
            lines.push(entry(key, what));
        }
        lines.push(Line::from(""));

        lines.push(header("TILING (Hyprland-style)"));
        for (key, what) in [
            ("Ctrl+h/l", "focus tile left/right"),
            ("Ctrl+j/k", "focus tile down/up (cycles PTYs inside Tiles)"),
            ("Ctrl+H/J/K/L", "swap tiles (shift)"),
            ("Ctrl+[/]", "resize focused split"),
            ("Ctrl+n", "cycle focused tile to the next panel type"),
        ] {
            lines.push(entry(key, what));
        }
        lines.push(Line::from(""));

        lines.push(header("GAMES (focus the arcade tile, prompt empty)"));
        for (key, what) in [
            ("1-7", "launch a game directly"),
            ("WASD/arrows", "select + play"),
            ("Enter/Space", "launch selected"),
            ("R", "restart session"),
            ("Esc", "back to selector"),
        ] {
            lines.push(entry(key, what));
        }
        let roster: Vec<String> = GameKind::ALL
            .iter()
            .map(|kind| {
                if kind.multiplayer() {
                    format!("{} [MP]", kind.label())
                } else {
                    kind.label().to_string()
                }
            })
            .collect();
        lines.push(entry("roster", &roster.join(", ")));
        lines.push(entry(
            "online play",
            "/host (or /join <code>), open Pong/Tron, pick HOST or JOIN ONLINE",
        ));
        lines.push(Line::from(""));

        lines.push(header("3D EFFECTS"));
        lines.push(entry("ring", &self.effects.names().join(", ")));

        let inner = popup.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });
        // Finding #17: the Paragraph word-wraps, so clamp the scroll against
        // the ESTIMATED wrapped row count at the overlay's inner width (same
        // idiom as render_messages_inner) -- clamping to the logical line
        // count leaves the bottom sections unreachable on narrow terminals.
        let wrap_width = inner.width.max(1) as usize;
        let total_rows: usize = lines
            .iter()
            .map(|line| {
                let w = line.width();
                if w == 0 { 1 } else { (w + wrap_width - 1) / wrap_width }
            })
            .sum();
        let max_scroll = total_rows.saturating_sub(inner.height as usize);
        self.help_scroll = self.help_scroll.min(max_scroll);

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .scroll((self.help_scroll as u16, 0))
                .style(Style::default().fg(t().text).bg(t().panel_bg)),
            inner,
        );
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

#[derive(Deserialize)]
struct YoutubeInfo {
    title: Option<String>,
}

struct YoutubeStream {
    title: String,
    source: String,
}

async fn resolve_youtube_stream(url: String) -> Result<YoutubeStream> {
    let info_output = tokio::process::Command::new("yt-dlp")
        .arg("--dump-single-json")
        .arg("--no-playlist")
        .arg("--no-warnings")
        .arg(&url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(ytdlp_spawn_error)?;

    if !info_output.status.success() {
        anyhow::bail!(command_error_message("yt-dlp metadata lookup", &info_output));
    }

    let info: YoutubeInfo = serde_json::from_slice(&info_output.stdout)
        .context("parse yt-dlp metadata json")?;
    let title = info
        .title
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "youtube video".to_string());
    let stream_output = tokio::process::Command::new("yt-dlp")
        .arg("--no-playlist")
        .arg("--no-warnings")
        .arg("--get-url")
        .arg("--format")
        .arg("bestvideo[height<=720]/best[height<=720]/bestvideo/best")
        .arg(&url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(ytdlp_spawn_error)?;

    if !stream_output.status.success() {
        anyhow::bail!(command_error_message("yt-dlp stream lookup", &stream_output));
    }

    let source = parse_stream_url(&stream_output.stdout)
        .context("yt-dlp did not report a playable stream url")?;

    Ok(YoutubeStream { title, source })
}

fn parse_stream_url(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("http://") || line.starts_with("https://"))
        .map(str::to_string)
}

fn command_error_message(context: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };
    if detail.is_empty() {
        format!(
            "{} failed with status {}",
            context,
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "?".to_string())
        )
    } else {
        format!("{} failed: {}", context, detail)
    }
}

/// Resolve the platform capture-device string for the primary display. On macOS
/// the avfoundation screen index is (num_cameras + N); we discover it by asking
/// ffmpeg to list devices and parsing the first "Capture screen" entry, falling
/// back to index 1 (the common single-camera case). Linux/Windows use their
/// fixed x11grab/gdigrab selectors (webcam.rs maps these per-OS).
fn resolve_screen_device() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-f",
                "avfoundation",
                "-list_devices",
                "true",
                "-i",
                "",
            ])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stderr);
            for line in text.lines() {
                if let Some(pos) = line.find("Capture screen") {
                    let prefix = &line[..pos];
                    if let (Some(lb), Some(rb)) = (prefix.rfind('['), prefix.rfind(']')) {
                        if lb < rb {
                            if let Ok(n) = prefix[lb + 1..rb].trim().parse::<u32>() {
                                return n.to_string();
                            }
                        }
                    }
                }
            }
        }
        "1".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        ":0.0".to_string()
    }
    #[cfg(target_os = "windows")]
    {
        "desktop".to_string()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "0".to_string()
    }
}

fn ytdlp_spawn_error(error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        anyhow::anyhow!("yt-dlp is not installed. install it first, then retry /youtube")
    } else {
        anyhow::Error::new(error).context("spawn yt-dlp")
    }
}

fn render_background(buffer: &mut Buffer, area: Rect, phase: f32) {
    // Snapshot the theme ONCE per frame (the old code took the lock twice
    // per cell), and lerp in u8 space without re-resolving colors.
    let (base_rgb, alt_rgb) = {
        let th = theme::t();
        (to_rgb(th.bg_base), to_rgb(th.bg_alt))
    };
    let w = area.width.max(1) as f32;
    let h = area.height.max(1) as f32;
    let cx = area.x as f32 + w * 0.5;
    let cy = area.y as f32 + h * 0.5;
    // Slow-breathing radial vignette (cells are ~2:1, so y counts double).
    let vig_scale = 1.0 + (phase * 0.23).sin() * 0.08;
    let inv_rx = 2.0 / (w * vig_scale);
    let inv_ry = 2.0 / (h * 1.15 * vig_scale);
    let seed_fine = (phase * 3.0) as u32;
    let seed_coarse = (phase * 0.7) as u32;

    for y in area.y..area.y + area.height {
        let band = (((y as f32 * 0.21) + phase * 1.2).sin() * 0.5 + 0.5) * 0.16;
        let dy = (y as f32 - cy) * inv_ry;
        let dy2 = dy * dy;
        for x in area.x..area.x + area.width {
            // Two-octave hash noise: fine shimmer + drifting coarse blobs.
            let fine = (hash32(x, y, seed_fine) & 0x07) as f32 / 7.0;
            let coarse = (hash32(x / 5, y / 3, seed_coarse) & 0x0f) as f32 / 15.0;
            let dx = (x as f32 - cx) * inv_rx;
            let vignette = (1.0 - (dx * dx + dy2) * 0.55).clamp(0.35, 1.0);
            let field = (band + fine * 0.07 + coarse * 0.17) * vignette;
            let t_mix = field.clamp(0.0, 1.0);
            let r = (base_rgb.0 as f32 + (alt_rgb.0 as f32 - base_rgb.0 as f32) * t_mix) as u8;
            let g = (base_rgb.1 as f32 + (alt_rgb.1 as f32 - base_rgb.1 as f32) * t_mix) as u8;
            let b = (base_rgb.2 as f32 + (alt_rgb.2 as f32 - base_rgb.2 as f32) * t_mix) as u8;
            let shade = Color::Rgb(r, g, b);
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.reset();
                cell.set_char(' ');
                cell.set_bg(shade);
                cell.set_fg(shade);
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
                cell.set_bg(mix_color(t().panel_alt, t().bg_alt, 0.4));
                cell.set_char('\u{2584}');
                cell.set_fg(mix_color(t().accent1, t().accent4, blend));
            }
        }
    }
}

fn render_logo(buffer: &mut Buffer, x: u16, y: u16, lines: &[&str], phase: f32) {
    let buf_area = *buffer.area();
    let max_x = buf_area.x + buf_area.width;
    let max_y = buf_area.y + buf_area.height;
    for (row, line) in lines.iter().enumerate() {
        for (column, glyph) in line.chars().enumerate() {
            if glyph == ' ' {
                continue;
            }
            let sx = x.saturating_add(column as u16 + 1);
            let sy = y.saturating_add(row as u16 + 1);
            if sx < max_x && sy < max_y {
                if let Some(shadow) = buffer.cell_mut((sx, sy)) {
                    shadow.set_char(glyph);
                    shadow.set_fg(Color::Rgb(37, 18, 10));
                }
            }
            let px = x.saturating_add(column as u16);
            let py = y.saturating_add(row as u16);
            if px < max_x && py < max_y {
                let blend = ((column as f32 / line.len().max(1) as f32) + phase * 0.07).fract();
                if let Some(cell) = buffer.cell_mut((px, py)) {
                    cell.set_char(glyph);
                    cell.set_fg(mix_color(t().accent1, t().accent2, blend));
                }
            }
        }
    }

    render_gradient_text(
        buffer,
        x + 40.min(18),
        y + lines.len() as u16 + 1,
        "CLI // AI + OPS + VIDEO + WEBCAM + 3D + CHAT + ANALYTICS",
        t().accent4,
        t().text,
    );
}

fn render_starburst(buffer: &mut Buffer, center_x: u16, center_y: u16, radius: u16, phase: f32, clip: Option<Rect>) {
    let clip_area = clip.unwrap_or(*buffer.area());
    let rays = 12;
    for ray in 0..rays {
        let angle = phase * 0.45 + ray as f32 * std::f32::consts::TAU / rays as f32;
        let dynamic = radius as f32 * (0.75 + 0.25 * (phase * 1.8 + ray as f32).sin());
        for step in 0..=dynamic.max(1.0) as u16 {
            let dx = angle.cos() * step as f32 * 1.2;
            let dy = angle.sin() * step as f32 * 0.55;
            let x = center_x as i16 + dx.round() as i16;
            let y = center_y as i16 + dy.round() as i16;
            if x < clip_area.x as i16 || y < clip_area.y as i16
                || x >= (clip_area.x + clip_area.width) as i16
                || y >= (clip_area.y + clip_area.height) as i16
            {
                continue;
            }
            if let Some(cell) = buffer.cell_mut((x as u16, y as u16)) {
                let blend = step as f32 / dynamic.max(1.0);
                cell.set_char(if step < radius / 2 { '*' } else { '+' });
                cell.set_fg(mix_color(t().accent1, t().accent2, blend));
            }
        }
    }

    if center_x >= clip_area.x && center_x < clip_area.x + clip_area.width
        && center_y >= clip_area.y && center_y < clip_area.y + clip_area.height
    {
        if let Some(cell) = buffer.cell_mut((center_x, center_y)) {
            cell.set_char('@');
            cell.set_fg(t().accent2);
        }
    }
}

fn render_gradient_text(buffer: &mut Buffer, x: u16, y: u16, text: &str, start: Color, end: Color) {
    let buf_area = *buffer.area();
    if y < buf_area.y || y >= buf_area.y + buf_area.height {
        return;
    }
    let max_x = buf_area.x + buf_area.width;
    let length = text.chars().count().max(1);
    for (index, glyph) in text.chars().enumerate() {
        let px = x.saturating_add(index as u16);
        if px >= max_x {
            break;
        }
        if px < buf_area.x {
            continue;
        }
        if let Some(cell) = buffer.cell_mut((px, y)) {
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
                cell.set_fg(mix_color(t().accent1, t().accent4, i as f32 / columns.max(1) as f32));
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
            cell.set_fg(mix_color(t().accent1, t().accent4, x as f32 / area.width.max(1) as f32));
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
                t().text,
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

fn format_ollama_model_meta(model: &OllamaModelInfo) -> String {
    let mut parts = Vec::new();
    if model.is_cloud {
        parts.push("cloud".to_string());
    } else if model.size_bytes > 0 {
        parts.push(human_bytes(model.size_bytes));
    }
    if let Some(parameter_size) = &model.parameter_size {
        if !parameter_size.is_empty() {
            parts.push(parameter_size.clone());
        }
    }
    if let Some(family) = &model.family {
        if !family.is_empty() {
            parts.push(family.clone());
        }
    }
    if parts.is_empty() {
        "model".to_string()
    } else {
        parts.join(" | ")
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
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

/// Finding #21: decide whether a '/'-prefixed input that matched no
/// registered command is a *mistyped command* (guard it with a hint) or a
/// *prompt* that must reach the AI (paths, sentences).
///
/// Command-shaped means BOTH:
///   - the first token matches `^/[A-Za-z0-9_-]{1,16}$` (so no second '/'
///     and no '.', which rules out filesystem paths and file names), and
///   - the whole input has at most 3 whitespace-separated tokens (a longer
///     input is a sentence, e.g. "/tmp has weird perms?").
fn looks_like_mistyped_command(input: &str) -> bool {
    let mut words = input.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    let Some(body) = first.strip_prefix('/') else {
        return false;
    };
    let token_ok = !body.is_empty()
        && body.len() <= 16
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    token_ok && words.count() <= 2 // first token + at most 2 more = 3 total
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
    let serve_port = args.serve;
    let connect_url = args.connect.clone();
    let mut app = App::new(args)?;

    // one-time terminal graphics-protocol probe: after alt-screen, before the
    // event loop reads input. Bulletproof -- unsupported/tmux/Apple Terminal
    // resolve to half-block, so this never breaks the guaranteed path.
    let (picker, hw_graphics, proto) = gfx::init_picker();
    app.attach_graphics(picker, hw_graphics);
    if hw_graphics {
        app.add_system_message(format!(
            "true-pixel graphics detected ({}) -- /vmode cycles to pixel for crisp video",
            gfx::protocol_label(proto)
        ));
    }

    if let Some(port) = serve_port {
        if app.start_chat_server(port) {
            app.add_system_message(format!("video chat server live on 0.0.0.0:{}", port));
            app.print_room_invite();
        }
    }

    if let Some(url) = connect_url {
        let url = if url.contains("://") {
            url
        } else {
            format!("ws://{}", url)
        };
        app.add_system_message(format!(
            "connecting to {} as {} ...",
            url, app.username
        ));
        app.start_video_client(url);
    }

    loop {
        if app.handle_input()? {
            break;
        }
        app.tick();
        // detect mode transitions (intro->chat) and force full terminal redraw
        if app.mode != app.prev_mode {
            app.prev_mode = app.mode.clone();
            // 1) physically clear the terminal screen via raw ANSI
            execute!(
                terminal.backend_mut(),
                crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
                crossterm::cursor::MoveTo(0, 0)
            )?;
            // 2) reset ratatui's back buffer so next draw() diffs against blank
            terminal.clear()?;
            // 3) immediately draw the new mode's first frame
            terminal.draw(|frame| app.render(frame))?;
        }
        terminal.draw(|frame| app.render(frame))?;
        tokio::time::sleep(Duration::from_millis(16)).await;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_filename("archive/mega-cli/.env");
    // also look for .env next to the installed binary's repo root and in the
    // config dir, so API keys load no matter which directory launched us
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            let _ = dotenvy::from_path(root.join(".env"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let _ = dotenvy::from_path(std::path::Path::new(&home).join(".config/asciivision/.env"));
    }

    let args = Args::parse();

    // restore the terminal on panic so a crash never leaves the shell frozen
    // in raw mode; stderr is muted, so the message goes to a log file and to
    // stdout after leaving the alternate screen
    std::panic::set_hook(Box::new(|info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
        let msg = format!(
            "asciivision panicked: {info}\nbacktrace:\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        if let Ok(home) = std::env::var("HOME") {
            let dir = std::path::Path::new(&home).join(".config/asciivision");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("panic.log"), &msg);
        }
        println!("{msg}");
    }));

    // suppress ALL FFmpeg log output before anything else --
    // FFmpeg writes to stderr which corrupts the TUI display
    unsafe { ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_QUIET) };

    // redirect stderr to /dev/null so nothing can corrupt the TUI
    // (must be opened writable or every stderr write fails with EBADF)
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if let Ok(devnull) = std::fs::OpenOptions::new().write(true).open("/dev/null") {
            extern "C" { fn dup2(oldfd: i32, newfd: i32) -> i32; }
            unsafe { dup2(devnull.as_raw_fd(), 2); }
        }
    }

    // request a large terminal window before entering raw mode
    // \x1b[8;rows;colst resizes the terminal on macOS Terminal.app, iTerm2, etc.
    {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[8;58;200t");
        let _ = out.flush();
        std::thread::sleep(Duration::from_millis(150));
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_app(&mut terminal, args).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

#[cfg(test)]
mod tests {
    use super::looks_like_mistyped_command;

    /// Finding #21: typo-shaped slash inputs stay guarded.
    #[test]
    fn mistyped_commands_are_guarded() {
        assert!(looks_like_mistyped_command("/hots"));
        assert!(looks_like_mistyped_command("/hots 9999"));
        assert!(looks_like_mistyped_command("/joim K7QM3-XZ2AB"));
        assert!(looks_like_mistyped_command("/tmp")); // accepted narrow miss
        assert!(looks_like_mistyped_command("/fx torus knot"));
        assert!(looks_like_mistyped_command("/a-b_c2"));
    }

    /// Finding #21: paths and '/'-leading sentences must reach the AI.
    #[test]
    fn paths_and_sentences_reach_the_ai() {
        // second '/' or '.' in the token -> path or file, not a command
        assert!(!looks_like_mistyped_command("/Users/me/notes.txt explain"));
        assert!(!looks_like_mistyped_command("/etc/hosts is blocking me, why?"));
        assert!(!looks_like_mistyped_command("/notes.txt summarize"));
        assert!(!looks_like_mistyped_command("/Users/x/y.txt"));
        // more than 3 whitespace-separated tokens -> a sentence
        assert!(!looks_like_mistyped_command("/tmp has weird perms?"));
        // over-long token -> not a plausible command name
        assert!(!looks_like_mistyped_command("/waytoolongcommandname"));
        // bare or empty slash tokens are not command-shaped
        assert!(!looks_like_mistyped_command("/"));
        assert!(!looks_like_mistyped_command("/ what is this"));
        assert!(!looks_like_mistyped_command(""));
    }
}
