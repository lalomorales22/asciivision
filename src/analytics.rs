use crate::db::Database;
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};

use crate::theme::t;

pub struct AnalyticsPanel {
    pub active: bool,
    stats_cache: Option<AnalyticsStats>,
    last_refresh: std::time::Instant,
}

struct AnalyticsStats {
    total_messages: usize,
    user_messages: usize,
    assistant_messages: usize,
    shell_commands: usize,
    providers_used: Vec<String>,
}

impl AnalyticsPanel {
    pub fn new() -> Self {
        Self {
            active: false,
            stats_cache: None,
            last_refresh: std::time::Instant::now(),
        }
    }

    pub fn refresh(&mut self, db: Option<&Database>) {
        if self.last_refresh.elapsed().as_secs() < 5 && self.stats_cache.is_some() {
            return;
        }
        self.last_refresh = std::time::Instant::now();

        let db = match db {
            Some(db) => db,
            None => {
                self.stats_cache = None;
                return;
            }
        };

        let conn = db.connection();
        let total = count_query(conn, "SELECT COUNT(*) FROM messages");
        let user_msgs = count_query(conn, "SELECT COUNT(*) FROM messages WHERE role = 'user' AND kind = 'chat'");
        let assistant_msgs = count_query(conn, "SELECT COUNT(*) FROM messages WHERE role = 'assistant'");
        let shell_cmds = count_query(conn, "SELECT COUNT(*) FROM messages WHERE kind = 'shell'");

        let providers: Vec<String> = conn
            .prepare("SELECT DISTINCT provider FROM messages")
            .ok()
            .map(|mut stmt| {
                stmt.query_map([], |row| row.get::<_, String>(0))
                    .ok()
                    .map(|rows| rows.filter_map(|r| r.ok()).collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        self.stats_cache = Some(AnalyticsStats {
            total_messages: total,
            user_messages: user_msgs,
            assistant_messages: assistant_msgs,
            shell_commands: shell_cmds,
            providers_used: providers,
        });
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let block = Block::default()
            .title(" ANALYTICS DASHBOARD ")
            .title_style(Style::default().fg(t().accent2).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t().accent1));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if let Some(ref stats) = self.stats_cache {
            let provider_str = if stats.providers_used.is_empty() {
                "none".to_string()
            } else {
                stats.providers_used.join(", ")
            };

            let bar_width = inner.width.saturating_sub(22) as usize;
            let total = stats.total_messages.max(1);
            let user_bar = make_bar(stats.user_messages, total, bar_width, t().accent4);
            let ai_bar = make_bar(stats.assistant_messages, total, bar_width, t().accent3);
            let shell_bar = make_bar(stats.shell_commands, total, bar_width, t().accent1);

            let mut lines = vec![
                Line::from(vec![
                    Span::styled("TOTAL MSGS:  ", Style::default().fg(t().accent2).bold()),
                    Span::styled(
                        format!("{}", stats.total_messages),
                        Style::default().fg(t().text),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled("USER:        ", Style::default().fg(t().accent2).bold()),
                    Span::styled(
                        format!("{:>5}  ", stats.user_messages),
                        Style::default().fg(t().text),
                    ),
                ]),
                Line::from(user_bar),
                Line::from(vec![
                    Span::styled("AI:          ", Style::default().fg(t().accent2).bold()),
                    Span::styled(
                        format!("{:>5}  ", stats.assistant_messages),
                        Style::default().fg(t().text),
                    ),
                ]),
                Line::from(ai_bar),
                Line::from(vec![
                    Span::styled("SHELL:       ", Style::default().fg(t().accent2).bold()),
                    Span::styled(
                        format!("{:>5}  ", stats.shell_commands),
                        Style::default().fg(t().text),
                    ),
                ]),
                Line::from(shell_bar),
                Line::from(""),
                Line::from(vec![
                    Span::styled("PROVIDERS:   ", Style::default().fg(t().accent2).bold()),
                    Span::styled(provider_str, Style::default().fg(t().text)),
                ]),
            ];

            let spinner_idx = ((phase * 4.0) as usize) % 4;
            let spinner = ["-", "\\", "|", "/"][spinner_idx];
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{}] live analytics feed", spinner),
                    Style::default().fg(t().muted),
                ),
            ]));

            frame.render_widget(
                Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().bg(t().panel_bg)),
                inner,
            );
        } else {
            frame.render_widget(
                Paragraph::new("analytics offline: no database connection")
                    .style(Style::default().fg(t().muted).bg(t().panel_bg))
                    .alignment(Alignment::Center),
                inner,
            );
        }
    }
}

fn count_query(conn: &rusqlite::Connection, sql: &str) -> usize {
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .unwrap_or(0) as usize
}

fn make_bar(value: usize, total: usize, width: usize, color: Color) -> Vec<Span<'static>> {
    let pct = value as f32 / total.max(1) as f32;
    let filled = (pct * width as f32) as usize;
    let empty = width.saturating_sub(filled);
    vec![
        Span::styled("             ", Style::default()),
        Span::styled(
            "\u{2588}".repeat(filled),
            Style::default().fg(color),
        ),
        Span::styled(
            "\u{2591}".repeat(empty),
            Style::default().fg(Color::Rgb(40, 50, 60)),
        ),
        Span::styled(
            format!(" {:.0}%", pct * 100.0),
            Style::default().fg(color),
        ),
    ]
}
