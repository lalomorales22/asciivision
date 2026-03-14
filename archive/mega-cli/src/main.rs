use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use std::time::Duration;

mod video;
mod chat;
mod ai;
mod db;

use video::VideoPlayer;
use chat::ChatInterface;
use ai::AIProvider;

#[derive(Parser, Debug)]
#[command(name = "MEGA-CLI", about = "Multi-AI terminal chatbot with cinematic loading")]
struct Args {
    /// Skip the loading video
    #[arg(long, default_value_t = false)]
    skip_loading: bool,

    /// AI provider to use (claude, grok, gpt, gemini)
    #[arg(long, default_value = "claude")]
    provider: String,
}

#[derive(Debug, Clone, PartialEq)]
enum AppState {
    Loading,
    Chat,
    Exiting,
}

struct App {
    state: AppState,
    video_player: Option<VideoPlayer>,
    chat: ChatInterface,
}

impl App {
    fn new(provider: AIProvider, skip_loading: bool) -> Result<Self> {
        let video_player = if !skip_loading {
            Some(VideoPlayer::new("loading.mp4")?)
        } else {
            None
        };

        Ok(Self {
            state: if skip_loading { AppState::Chat } else { AppState::Loading },
            video_player,
            chat: ChatInterface::new(provider.clone()),
        })
    }

    fn handle_input(&mut self) -> Result<bool> {
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                match self.state {
                    AppState::Loading => {
                        // Only allow quitting during loading, don't skip
                        if key.code == KeyCode::Char('q')
                            || key.code == KeyCode::Esc
                            || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')) {
                            self.state = AppState::Exiting;
                            return Ok(true);
                        }
                        // All other keys are ignored - let video play
                    }
                    AppState::Chat => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                            self.state = AppState::Exiting;
                            return Ok(true);
                        }
                        self.chat.handle_key(key)?;
                    }
                    AppState::Exiting => return Ok(true),
                }
            }
        }
        Ok(false)
    }

    fn update(&mut self) -> Result<()> {
        match self.state {
            AppState::Loading => {
                if let Some(ref mut player) = self.video_player {
                    if player.is_finished() {
                        self.state = AppState::Chat;
                    }
                }
            }
            AppState::Chat => {
                self.chat.update()?;
            }
            AppState::Exiting => {}
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) -> Result<()> {
        match self.state {
            AppState::Loading => {
                if let Some(ref mut player) = self.video_player {
                    player.render(frame)?;
                } else {
                    // Fallback if no video
                    let loading = Paragraph::new("MEGA-CLI Loading...")
                        .alignment(Alignment::Center)
                        .block(Block::default().borders(Borders::ALL).title("MEGA-CLI"));
                    frame.render_widget(loading, frame.area());
                }
            }
            AppState::Chat => {
                self.chat.render(frame)?;
            }
            AppState::Exiting => {
                let goodbye = Paragraph::new("Goodbye! 👋")
                    .alignment(Alignment::Center);
                frame.render_widget(goodbye, frame.area());
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Load environment variables
    let _ = dotenvy::dotenv();

    // Parse AI provider
    let provider = match args.provider.to_lowercase().as_str() {
        "claude" => AIProvider::Claude,
        "grok" => AIProvider::Grok,
        "gpt" | "openai" => AIProvider::OpenAI,
        "gemini" => AIProvider::Gemini,
        _ => {
            eprintln!("Unknown provider: {}. Using Claude.", args.provider);
            AIProvider::Claude
        }
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run app
    let mut app = App::new(provider, args.skip_loading)?;

    loop {
        terminal.draw(|f| {
            if let Err(e) = app.render(f) {
                eprintln!("Render error: {}", e);
            }
        })?;

        if app.handle_input()? {
            break;
        }

        app.update()?;

        // Small sleep to prevent CPU spinning
        tokio::time::sleep(Duration::from_millis(16)).await;
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
