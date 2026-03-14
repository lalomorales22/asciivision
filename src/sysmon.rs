use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, Networks, RefreshKind, System};
use std::time::Instant;

const AMBER: Color = Color::Rgb(241, 189, 105);
#[allow(dead_code)]
const COPPER: Color = Color::Rgb(214, 153, 104);
const TEAL: Color = Color::Rgb(54, 154, 158);
const CYAN: Color = Color::Rgb(118, 214, 226);
const ICE: Color = Color::Rgb(207, 230, 232);
const DANGER: Color = Color::Rgb(225, 92, 84);
const MUTED: Color = Color::Rgb(101, 121, 134);
const PANEL_BG: Color = Color::Rgb(6, 13, 19);

pub struct SystemMonitor {
    sys: System,
    networks: Networks,
    last_refresh: Instant,
    cpu_usage: f32,
    cpu_per_core: Vec<f32>,
    mem_total: u64,
    mem_used: u64,
    swap_total: u64,
    swap_used: u64,
    net_rx_bytes: u64,
    net_tx_bytes: u64,
    load_avg: [f64; 3],
    process_count: usize,
}

impl SystemMonitor {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        let networks = Networks::new_with_refreshed_list();

        Self {
            sys,
            networks,
            last_refresh: Instant::now(),
            cpu_usage: 0.0,
            cpu_per_core: Vec::new(),
            mem_total: 0,
            mem_used: 0,
            swap_total: 0,
            swap_used: 0,
            net_rx_bytes: 0,
            net_tx_bytes: 0,
            load_avg: [0.0; 3],
            process_count: 0,
        }
    }

    pub fn refresh(&mut self) {
        if self.last_refresh.elapsed().as_millis() < 1500 {
            return;
        }
        self.last_refresh = Instant::now();

        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.networks.refresh(true);

        self.cpu_usage = self.sys.global_cpu_usage();
        self.cpu_per_core = self.sys.cpus().iter().map(|c| c.cpu_usage()).collect();
        self.mem_total = self.sys.total_memory();
        self.mem_used = self.sys.used_memory();
        self.swap_total = self.sys.total_swap();
        self.swap_used = self.sys.used_swap();

        let mut rx = 0u64;
        let mut tx = 0u64;
        for (_name, data) in self.networks.iter() {
            rx += data.received();
            tx += data.transmitted();
        }
        self.net_rx_bytes = rx;
        self.net_tx_bytes = tx;

        let load = System::load_average();
        self.load_avg = [load.one, load.five, load.fifteen];
        self.process_count = self.sys.cpus().len();
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, phase: f32, is_focused: bool) {
        let border_color = if is_focused { CYAN } else { TEAL };
        let block = Block::default()
            .title(" SYS MONITOR ")
            .title_style(Style::default().fg(AMBER).bold())
            .borders(Borders::ALL)
            .border_type(if is_focused {
                BorderType::Double
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(border_color));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        if inner.height < 3 || inner.width < 10 {
            return;
        }

        let mut lines: Vec<Line> = Vec::new();

        // CPU
        let cpu_color = if self.cpu_usage > 80.0 {
            DANGER
        } else if self.cpu_usage > 50.0 {
            AMBER
        } else {
            TEAL
        };
        lines.push(Line::from(vec![
            Span::styled("CPU  ", Style::default().fg(AMBER).bold()),
            Span::styled(
                format!("{:5.1}%  ", self.cpu_usage),
                Style::default().fg(cpu_color),
            ),
            Span::styled(
                mini_bar(self.cpu_usage / 100.0, inner.width.saturating_sub(16) as usize),
                Style::default().fg(cpu_color),
            ),
        ]));

        // Per-core sparkline (compact)
        if inner.height > 8 && !self.cpu_per_core.is_empty() {
            let sparks: String = self
                .cpu_per_core
                .iter()
                .take(inner.width.saturating_sub(6) as usize)
                .map(|&u| spark_char(u / 100.0))
                .collect();
            lines.push(Line::from(vec![
                Span::styled("     ", Style::default()),
                Span::styled(sparks, Style::default().fg(CYAN)),
            ]));
        }

        // Memory
        let mem_pct = if self.mem_total > 0 {
            self.mem_used as f32 / self.mem_total as f32
        } else {
            0.0
        };
        let mem_color = if mem_pct > 0.85 {
            DANGER
        } else if mem_pct > 0.65 {
            AMBER
        } else {
            TEAL
        };
        lines.push(Line::from(vec![
            Span::styled("MEM  ", Style::default().fg(AMBER).bold()),
            Span::styled(
                format!(
                    "{:>5}/{:<5} ",
                    fmt_bytes(self.mem_used),
                    fmt_bytes(self.mem_total)
                ),
                Style::default().fg(mem_color),
            ),
            Span::styled(
                mini_bar(mem_pct, inner.width.saturating_sub(20) as usize),
                Style::default().fg(mem_color),
            ),
        ]));

        // Swap
        if self.swap_total > 0 {
            let swap_pct = self.swap_used as f32 / self.swap_total as f32;
            lines.push(Line::from(vec![
                Span::styled("SWAP ", Style::default().fg(AMBER).bold()),
                Span::styled(
                    format!(
                        "{:>5}/{:<5} ",
                        fmt_bytes(self.swap_used),
                        fmt_bytes(self.swap_total)
                    ),
                    Style::default().fg(if swap_pct > 0.5 { DANGER } else { ICE }),
                ),
                Span::styled(
                    mini_bar(swap_pct, inner.width.saturating_sub(20) as usize),
                    Style::default().fg(if swap_pct > 0.5 { DANGER } else { MUTED }),
                ),
            ]));
        }

        // Network
        lines.push(Line::from(vec![
            Span::styled("NET  ", Style::default().fg(AMBER).bold()),
            Span::styled(
                format!(
                    "\u{2191}{} \u{2193}{}",
                    fmt_bytes(self.net_tx_bytes),
                    fmt_bytes(self.net_rx_bytes),
                ),
                Style::default().fg(CYAN),
            ),
        ]));

        // Load average
        lines.push(Line::from(vec![
            Span::styled("LOAD ", Style::default().fg(AMBER).bold()),
            Span::styled(
                format!(
                    "{:.2}  {:.2}  {:.2}",
                    self.load_avg[0], self.load_avg[1], self.load_avg[2]
                ),
                Style::default().fg(ICE),
            ),
        ]));

        // Cores
        lines.push(Line::from(vec![
            Span::styled("CORE ", Style::default().fg(AMBER).bold()),
            Span::styled(
                format!("{} threads", self.cpu_per_core.len()),
                Style::default().fg(ICE),
            ),
        ]));

        // Activity sparkline
        if inner.height > 10 {
            let spinner_idx = ((phase * 6.0) as usize) % 4;
            let spinner = ["\u{2596}", "\u{2598}", "\u{259D}", "\u{2597}"][spinner_idx];
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} sampling", spinner),
                    Style::default().fg(MUTED),
                ),
            ]));
        }

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .style(Style::default().bg(PANEL_BG)),
            inner,
        );
    }
}

fn mini_bar(pct: f32, width: usize) -> String {
    let pct = pct.clamp(0.0, 1.0);
    let filled = (pct * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!(
        "{}{}",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(empty)
    )
}

fn spark_char(pct: f32) -> char {
    let sparks = [' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];
    let idx = (pct.clamp(0.0, 1.0) * 8.0).round() as usize;
    sparks[idx.min(sparks.len() - 1)]
}

fn fmt_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    const TB: u64 = 1024 * 1024 * 1024 * 1024;

    if bytes >= TB {
        format!("{:.1}T", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}
