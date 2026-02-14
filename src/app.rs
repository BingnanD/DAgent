use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Stdout;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use crossterm::terminal::{Clear as TermClear, ClearType};
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    cleaned_assistant_text, default_commands, detect_available_providers, execute_line,
    extract_agent_name, high_risk_check, input_cursor_position, kill_pid, memory::MemoryStore,
    ordered_providers, provider_from_name, providers_label, truncate, DispatchTarget,
    THINKING_PLACEHOLDER,
};

const COLLAPSED_PASTE_CHAR_THRESHOLD: usize = 800;
const COLLAPSED_PASTE_LINE_THRESHOLD: usize = 12;
const MEM_SHOW_DEFAULT_LIMIT: usize = 20;
const MEM_SHOW_MAX_LIMIT: usize = 200;
const MEM_FIND_DEFAULT_LIMIT: usize = 12;
const MEM_PRUNE_DEFAULT_KEEP: usize = 200;
const STARTUP_BANNER_PREFIX: &str = "__startup_banner__:";

#[path = "ui.rs"]
pub(crate) mod ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    pub(crate) fn binary(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    pub(crate) fn all() -> [Provider; 2] {
        [Provider::Claude, Provider::Codex]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ThemePreset {
    Fjord,
    Graphite,
    Solarized,
    Aurora,
    Ember,
}

impl ThemePreset {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            ThemePreset::Fjord => "fjord",
            ThemePreset::Graphite => "graphite",
            ThemePreset::Solarized => "solarized",
            ThemePreset::Aurora => "aurora",
            ThemePreset::Ember => "ember",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "fjord" | "nord" | "blue" => Some(ThemePreset::Fjord),
            "graphite" | "slate" | "gray" => Some(ThemePreset::Graphite),
            "solarized" | "sand" | "amber" => Some(ThemePreset::Solarized),
            "aurora" | "mint" | "teal" => Some(ThemePreset::Aurora),
            "ember" | "warm" | "copper" => Some(ThemePreset::Ember),
            _ => None,
        }
    }

    fn palette(self) -> ThemePalette {
        match self {
            ThemePreset::Fjord => ThemePalette {
                prompt: Color::Rgb(145, 145, 150),
                input_bg: Color::Rgb(16, 16, 18),
                input_text: Color::Rgb(188, 188, 192),
                muted_text: Color::Rgb(58, 58, 62),
                highlight_fg: Color::Rgb(210, 210, 214),
                highlight_bg: Color::Rgb(34, 34, 38),
                activity_badge_fg: Color::Rgb(210, 210, 214),
                activity_badge_bg: Color::Rgb(48, 48, 54),
                activity_text: Color::Rgb(125, 125, 132),
                status_text: Color::Rgb(58, 58, 62),
                user_fg: Color::Rgb(210, 210, 214),
                user_bg: Color::Rgb(22, 22, 24),
                claude_label: Color::Rgb(255, 102, 51),
                codex_label: Color::Rgb(120, 120, 128),
                processing_label: Color::Rgb(110, 110, 118),
                assistant_text: Color::Rgb(185, 185, 190),
                assistant_processing_text: Color::Rgb(130, 130, 138),
                system_text: Color::Rgb(58, 58, 62),
                tool_icon: Color::Rgb(85, 85, 92),
                tool_text: Color::Rgb(95, 95, 102),
                error_label: Color::Rgb(160, 70, 70),
                error_text: Color::Rgb(165, 85, 78),
                banner_title: Color::Rgb(150, 150, 156),
                panel_bg: Color::Rgb(10, 10, 12),
                panel_fg: Color::Rgb(185, 185, 190),
                approval_title: Color::Rgb(165, 120, 80),
                code_fg: Color::Rgb(170, 170, 178),
                code_bg: Color::Rgb(6, 6, 8),
                inline_code_fg: Color::Rgb(140, 138, 132),
                inline_code_bg: Color::Rgb(18, 18, 22),
                bullet: Color::Rgb(85, 85, 92),
            },
            ThemePreset::Graphite => ThemePalette {
                prompt: Color::Rgb(138, 138, 142),
                input_bg: Color::Rgb(14, 14, 16),
                input_text: Color::Rgb(182, 182, 186),
                muted_text: Color::Rgb(55, 55, 58),
                highlight_fg: Color::Rgb(205, 205, 208),
                highlight_bg: Color::Rgb(32, 32, 36),
                activity_badge_fg: Color::Rgb(205, 205, 208),
                activity_badge_bg: Color::Rgb(45, 45, 50),
                activity_text: Color::Rgb(118, 118, 125),
                status_text: Color::Rgb(52, 52, 56),
                user_fg: Color::Rgb(205, 205, 208),
                user_bg: Color::Rgb(20, 20, 22),
                claude_label: Color::Rgb(255, 102, 51),
                codex_label: Color::Rgb(115, 115, 122),
                processing_label: Color::Rgb(105, 105, 112),
                assistant_text: Color::Rgb(178, 178, 184),
                assistant_processing_text: Color::Rgb(125, 125, 132),
                system_text: Color::Rgb(52, 52, 58),
                tool_icon: Color::Rgb(80, 80, 88),
                tool_text: Color::Rgb(88, 88, 96),
                error_label: Color::Rgb(155, 55, 55),
                error_text: Color::Rgb(160, 72, 62),
                banner_title: Color::Rgb(148, 148, 155),
                panel_bg: Color::Rgb(8, 8, 10),
                panel_fg: Color::Rgb(178, 178, 184),
                approval_title: Color::Rgb(160, 110, 68),
                code_fg: Color::Rgb(165, 165, 172),
                code_bg: Color::Rgb(4, 4, 6),
                inline_code_fg: Color::Rgb(135, 132, 128),
                inline_code_bg: Color::Rgb(16, 16, 20),
                bullet: Color::Rgb(80, 80, 88),
            },
            ThemePreset::Solarized => ThemePalette {
                prompt: Color::Rgb(142, 142, 148),
                input_bg: Color::Rgb(15, 15, 17),
                input_text: Color::Rgb(190, 190, 195),
                muted_text: Color::Rgb(60, 60, 65),
                highlight_fg: Color::Rgb(212, 212, 216),
                highlight_bg: Color::Rgb(35, 35, 40),
                activity_badge_fg: Color::Rgb(212, 212, 216),
                activity_badge_bg: Color::Rgb(46, 46, 52),
                activity_text: Color::Rgb(122, 122, 130),
                status_text: Color::Rgb(56, 56, 60),
                user_fg: Color::Rgb(212, 212, 216),
                user_bg: Color::Rgb(21, 21, 24),
                claude_label: Color::Rgb(255, 102, 51),
                codex_label: Color::Rgb(118, 118, 125),
                processing_label: Color::Rgb(108, 108, 115),
                assistant_text: Color::Rgb(186, 186, 192),
                assistant_processing_text: Color::Rgb(132, 132, 140),
                system_text: Color::Rgb(56, 56, 62),
                tool_icon: Color::Rgb(82, 82, 90),
                tool_text: Color::Rgb(92, 92, 100),
                error_label: Color::Rgb(158, 58, 58),
                error_text: Color::Rgb(162, 78, 68),
                banner_title: Color::Rgb(152, 152, 158),
                panel_bg: Color::Rgb(9, 9, 11),
                panel_fg: Color::Rgb(186, 186, 192),
                approval_title: Color::Rgb(162, 115, 72),
                code_fg: Color::Rgb(172, 172, 180),
                code_bg: Color::Rgb(5, 5, 7),
                inline_code_fg: Color::Rgb(138, 135, 130),
                inline_code_bg: Color::Rgb(17, 17, 21),
                bullet: Color::Rgb(82, 82, 90),
            },
            ThemePreset::Aurora => ThemePalette {
                prompt: Color::Rgb(148, 148, 152),
                input_bg: Color::Rgb(17, 17, 19),
                input_text: Color::Rgb(192, 192, 196),
                muted_text: Color::Rgb(62, 62, 66),
                highlight_fg: Color::Rgb(215, 215, 218),
                highlight_bg: Color::Rgb(36, 36, 40),
                activity_badge_fg: Color::Rgb(215, 215, 218),
                activity_badge_bg: Color::Rgb(50, 50, 56),
                activity_text: Color::Rgb(128, 128, 135),
                status_text: Color::Rgb(58, 58, 62),
                user_fg: Color::Rgb(215, 215, 218),
                user_bg: Color::Rgb(24, 24, 26),
                claude_label: Color::Rgb(255, 102, 51),
                codex_label: Color::Rgb(122, 122, 128),
                processing_label: Color::Rgb(112, 112, 118),
                assistant_text: Color::Rgb(190, 190, 195),
                assistant_processing_text: Color::Rgb(135, 135, 142),
                system_text: Color::Rgb(58, 58, 64),
                tool_icon: Color::Rgb(88, 88, 95),
                tool_text: Color::Rgb(98, 98, 105),
                error_label: Color::Rgb(162, 68, 68),
                error_text: Color::Rgb(168, 82, 72),
                banner_title: Color::Rgb(155, 155, 160),
                panel_bg: Color::Rgb(11, 11, 13),
                panel_fg: Color::Rgb(190, 190, 195),
                approval_title: Color::Rgb(168, 122, 78),
                code_fg: Color::Rgb(175, 175, 182),
                code_bg: Color::Rgb(7, 7, 9),
                inline_code_fg: Color::Rgb(142, 140, 135),
                inline_code_bg: Color::Rgb(19, 19, 23),
                bullet: Color::Rgb(88, 88, 95),
            },
            ThemePreset::Ember => ThemePalette {
                prompt: Color::Rgb(152, 152, 155),
                input_bg: Color::Rgb(18, 18, 20),
                input_text: Color::Rgb(195, 195, 198),
                muted_text: Color::Rgb(65, 65, 68),
                highlight_fg: Color::Rgb(218, 218, 220),
                highlight_bg: Color::Rgb(38, 38, 42),
                activity_badge_fg: Color::Rgb(218, 218, 220),
                activity_badge_bg: Color::Rgb(52, 52, 58),
                activity_text: Color::Rgb(132, 132, 138),
                status_text: Color::Rgb(60, 60, 64),
                user_fg: Color::Rgb(218, 218, 220),
                user_bg: Color::Rgb(25, 25, 28),
                claude_label: Color::Rgb(255, 102, 51),
                codex_label: Color::Rgb(125, 125, 130),
                processing_label: Color::Rgb(115, 115, 120),
                assistant_text: Color::Rgb(192, 192, 198),
                assistant_processing_text: Color::Rgb(138, 138, 145),
                system_text: Color::Rgb(60, 60, 66),
                tool_icon: Color::Rgb(92, 92, 98),
                tool_text: Color::Rgb(100, 100, 108),
                error_label: Color::Rgb(165, 72, 72),
                error_text: Color::Rgb(170, 88, 78),
                banner_title: Color::Rgb(158, 158, 162),
                panel_bg: Color::Rgb(12, 12, 14),
                panel_fg: Color::Rgb(192, 192, 198),
                approval_title: Color::Rgb(170, 125, 82),
                code_fg: Color::Rgb(178, 178, 185),
                code_bg: Color::Rgb(8, 8, 10),
                inline_code_fg: Color::Rgb(145, 142, 138),
                inline_code_bg: Color::Rgb(20, 20, 24),
                bullet: Color::Rgb(92, 92, 98),
            },
        }
    }
}

fn default_theme() -> ThemePreset {
    ThemePreset::Graphite
}

#[derive(Clone, Copy)]
pub(crate) struct ThemePalette {
    pub(crate) prompt: Color,
    pub(crate) input_bg: Color,
    pub(crate) input_text: Color,
    pub(crate) muted_text: Color,
    pub(crate) highlight_fg: Color,
    pub(crate) highlight_bg: Color,
    #[allow(dead_code)]
    pub(crate) activity_badge_fg: Color,
    #[allow(dead_code)]
    pub(crate) activity_badge_bg: Color,
    pub(crate) activity_text: Color,
    pub(crate) status_text: Color,
    pub(crate) user_fg: Color,
    pub(crate) user_bg: Color,
    pub(crate) claude_label: Color,
    pub(crate) codex_label: Color,
    pub(crate) processing_label: Color,
    pub(crate) assistant_text: Color,
    pub(crate) assistant_processing_text: Color,
    pub(crate) system_text: Color,
    pub(crate) tool_icon: Color,
    pub(crate) tool_text: Color,
    pub(crate) error_label: Color,
    pub(crate) error_text: Color,
    pub(crate) banner_title: Color,
    pub(crate) panel_bg: Color,
    pub(crate) panel_fg: Color,
    pub(crate) approval_title: Color,
    pub(crate) code_fg: Color,
    pub(crate) code_bg: Color,
    pub(crate) inline_code_fg: Color,
    pub(crate) inline_code_bg: Color,
    pub(crate) bullet: Color,
}

impl ThemePalette {
    pub(crate) fn prompt_style(self) -> Style {
        Style::default()
            .fg(self.prompt)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn title_style(self) -> Style {
        Style::default()
            .fg(self.banner_title)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn body_style(self) -> Style {
        Style::default().fg(self.assistant_text)
    }

    pub(crate) fn body_processing_style(self) -> Style {
        Style::default().fg(self.assistant_processing_text)
    }

    pub(crate) fn secondary_style(self) -> Style {
        Style::default().fg(self.system_text)
    }

    pub(crate) fn muted_style(self) -> Style {
        Style::default().fg(self.muted_text)
    }

    pub(crate) fn status_style(self) -> Style {
        Style::default().fg(self.status_text)
    }

    pub(crate) fn panel_surface_style(self) -> Style {
        Style::default().bg(self.panel_bg).fg(self.panel_fg)
    }

    pub(crate) fn panel_border_style(self) -> Style {
        Style::default().fg(self.highlight_bg)
    }

    pub(crate) fn input_surface_style(self) -> Style {
        Style::default().bg(self.input_bg).fg(self.input_text)
    }

    pub(crate) fn hint_selected_style(self) -> Style {
        Style::default()
            .fg(self.highlight_fg)
            .bg(self.highlight_bg)
            .add_modifier(Modifier::BOLD)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum EntryKind {
    User,
    Assistant,
    System,
    Tool,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LogEntry {
    pub(crate) kind: EntryKind,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) elapsed_secs: Option<u64>,
}

#[derive(Clone, Debug)]
struct PendingApproval {
    line: String,
    tool: String,
    reason: String,
}

#[derive(Clone, Debug)]
struct PendingPaste {
    marker: String,
    content: String,
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Normal,
    HistorySearch,
    Approval,
}

#[derive(Debug)]
pub(crate) enum WorkerEvent {
    Done(String),
    AgentStart(Provider),
    AgentChunk { provider: Provider, chunk: String },
    AgentDone(Provider),
    Tool(String),
    /// Lightweight progress update – only shown in the spinner area,
    /// never written to the transcript.
    Progress(String),
    PromotePrimary { to: Provider, reason: String },
    Error(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionSnapshot {
    primary_provider: Provider,
    #[serde(default = "default_theme")]
    theme: ThemePreset,
    entries: Vec<LogEntry>,
    history: Vec<String>,
    #[serde(default = "default_session_id")]
    session_id: String,
}

fn default_session_id() -> String {
    "default".to_string()
}

fn restore_transcript_on_start(memory_available: bool) -> bool {
    if !memory_available {
        return true;
    }

    match std::env::var("DAGENT_RESTORE_TRANSCRIPT") {
        Ok(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Cached rendering state to avoid recomputing log lines and scroll bounds every frame.
struct RenderCache {
    /// Generation counter at the time of last cache build.
    generation: u64,
    /// Viewport width used for the cached lines.
    width: u16,
    /// Viewport height used for the cached scroll_max.
    height: u16,
    /// The cached rendered lines.
    lines: Vec<Line<'static>>,
    /// The cached maximum scroll offset.
    scroll_max: u16,
}

impl RenderCache {
    fn new() -> Self {
        Self {
            generation: u64::MAX, // force first rebuild
            width: 0,
            height: 0,
            lines: Vec::new(),
            scroll_max: 0,
        }
    }
}

pub(crate) fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let mut app = App::new();
    const ACTIVE_POLL_MS: u64 = 33;
    const IDLE_POLL_MS: u64 = 100;
    const SPINNER_TICK_MS: u64 = 120;
    const RUNNING_DRAW_INTERVAL_MS: u64 = 33;
    const MAX_EVENTS_PER_FRAME: u16 = 64;
    let mut last_spinner_tick = Instant::now();
    let mut last_draw_at = Instant::now()
        .checked_sub(Duration::from_millis(RUNNING_DRAW_INTERVAL_MS))
        .unwrap_or_else(Instant::now);
    let mut needs_draw = true;
    let mut flushed_log_lines: Vec<Line<'static>> = Vec::new();

    loop {
        let mut state_changed = false;
        if app.poll_worker() {
            state_changed = true;
        }
        if app.running
            && last_spinner_tick.elapsed() >= Duration::from_millis(SPINNER_TICK_MS)
        {
            app.spinner_idx = (app.spinner_idx + 1) % 8;
            last_spinner_tick = Instant::now();
            state_changed = true;
        }
        if state_changed {
            needs_draw = true;
        }

        if app.needs_screen_clear {
            app.needs_screen_clear = false;
            flushed_log_lines.clear();
            // Clear the entire terminal including scrollback, not just the ratatui viewport.
            crossterm::execute!(
                std::io::stdout(),
                TermClear(ClearType::Purge),
                crossterm::cursor::MoveTo(0, 0)
            )?;
            terminal.clear()?;
            needs_draw = true;
        }

        if needs_draw {
            if app.running
                && last_draw_at.elapsed() < Duration::from_millis(RUNNING_DRAW_INTERVAL_MS)
            {
                // Hold briefly to batch incoming chunks and avoid per-frame flashing.
            } else {
                if let Ok(area) = terminal.size() {
                    app.update_viewport(area.width, area.height);
                }
                app.ensure_render_cache();
                flush_new_log_lines(terminal, &app, &mut flushed_log_lines)?;
                terminal.draw(|f| ui::draw(f, &app))?;
                last_draw_at = Instant::now();
                needs_draw = false;
            }
        }

        if app.should_quit {
            break;
        }

        let timeout = if app.running {
            Duration::from_millis(ACTIVE_POLL_MS)
        } else {
            Duration::from_millis(IDLE_POLL_MS)
        };
        if !event::poll(timeout).context("event poll")? {
            continue;
        }

        let mut wheel_delta: i32 = 0;
        let mut drained_events: u16 = 0;
        let mut input_changed = false;

        loop {
            match event::read().context("event read")? {
                Event::Key(key) => {
                    if !matches!(key.kind, KeyEventKind::Release) {
                        app.handle_key(key);
                        input_changed = true;
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => wheel_delta -= 1,
                    MouseEventKind::ScrollDown => wheel_delta += 1,
                    _ => {}
                },
                Event::Paste(text) => {
                    app.handle_paste_event(&text);
                    input_changed = true;
                }
                Event::Resize(_, _) => {
                    input_changed = true;
                }
                _ => {}
            }

            drained_events = drained_events.saturating_add(1);
            if drained_events >= MAX_EVENTS_PER_FRAME {
                break;
            }
            if !event::poll(Duration::from_millis(0)).context("event poll drain")? {
                break;
            }
        }

        if wheel_delta < 0 {
            app.scroll_up(wheel_delta.abs().min(64) as u16);
            input_changed = true;
        } else if wheel_delta > 0 {
            app.scroll_down(wheel_delta.min(64) as u16);
            input_changed = true;
        }

        if input_changed {
            needs_draw = true;
        }
    }

    app.persist_session();

    // Clear input/status bars while keeping transcript in terminal scrollback.
    terminal.draw(|f| ui::draw_exit(f, &app))?;
    Ok(())
}

fn flush_new_log_lines(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &App,
    flushed_log_lines: &mut Vec<Line<'static>>,
) -> Result<()> {
    // When running, only flush stable (non-streaming) entries so that the user
    // message remains visible in scrollback. When idle, flush everything.
    let lines: &[Line<'static>];
    let stable_buf;
    if app.running {
        let w = terminal.size().map(|s| s.width).unwrap_or(80).max(1);
        stable_buf = app.stable_log_lines(w);
        lines = &stable_buf;
    } else {
        lines = app.cached_log_lines();
    }
    if lines.is_empty() {
        flushed_log_lines.clear();
        return Ok(());
    }

    // Compare using plain text to avoid false mismatches caused by style-only updates
    // (for example, running -> done color changes).
    let flushed_plain = flatten_lines_to_plain(flushed_log_lines);
    let current_plain = flatten_lines_to_plain(lines);

    let append_ranges = compute_append_ranges(&flushed_plain, &current_plain);
    if append_ranges.is_empty() {
        *flushed_log_lines = lines.to_vec();
        return Ok(());
    }
    let mut new_lines = Vec::new();
    for (start, end) in append_ranges {
        new_lines.extend(lines[start..end].iter().cloned());
    }
    let width = terminal
        .size()
        .context("terminal size for insert")?
        .width
        .max(1);
    let probe = Paragraph::new(Text::from(new_lines.clone())).wrap(Wrap { trim: false });
    let rendered_lines = probe.line_count(width).min(u16::MAX as usize);
    let height = rendered_lines as u16;
    if height == 0 {
        return Ok(());
    }

    let insert_result = catch_unwind(AssertUnwindSafe(|| {
        terminal.insert_before(height, |buf| {
            let paragraph = Paragraph::new(Text::from(new_lines)).wrap(Wrap { trim: false });
            paragraph.render(buf.area, buf);
        })
    }));
    match insert_result {
        Ok(res) => {
            res.context("insert transcript lines")?;
        }
        Err(_) => {
            // Fallback: force a replay on next frame instead of silently dropping updates.
            flushed_log_lines.clear();
            return Ok(());
        }
    }

    *flushed_log_lines = lines.to_vec();
    Ok(())
}

fn flatten_lines_to_plain(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(flatten_line_to_plain).collect()
}

fn flatten_line_to_plain(line: &Line<'static>) -> String {
    let mut out = String::new();
    for span in &line.spans {
        out.push_str(span.content.as_ref());
    }
    out
}

fn compute_append_ranges(
    flushed_plain: &[String],
    current_plain: &[String],
) -> Vec<(usize, usize)> {
    if flushed_plain == current_plain {
        return Vec::new();
    }
    if flushed_plain.len() > current_plain.len() {
        return Vec::new();
    }
    if current_plain[..flushed_plain.len()] == *flushed_plain {
        return vec![(flushed_plain.len(), current_plain.len())];
    }
    // Existing lines changed in place. We cannot patch scrollback safely.
    Vec::new()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupBannerRow<'a> {
    Title(&'a str),
    Agents(&'a str),
    Cwd(&'a str),
    Keys(&'a str),
}

fn parse_startup_banner_row(text: &str) -> Option<StartupBannerRow<'_>> {
    let payload = text.strip_prefix(STARTUP_BANNER_PREFIX)?;
    let (kind, value) = payload.split_once('|')?;
    match kind {
        "title" => Some(StartupBannerRow::Title(value)),
        "agents" => Some(StartupBannerRow::Agents(value)),
        "cwd" => Some(StartupBannerRow::Cwd(value)),
        "keys" => Some(StartupBannerRow::Keys(value)),
        _ => None,
    }
}

fn is_startup_banner_entry(entry: &LogEntry) -> bool {
    matches!(entry.kind, EntryKind::System) && parse_startup_banner_row(&entry.text).is_some()
}

fn banner_card_outer_width(viewport_width: u16) -> usize {
    let max_outer = viewport_width.max(1) as usize;
    if max_outer >= 26 {
        max_outer.min(76)
    } else {
        max_outer
    }
}

fn truncate_display_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > max_width {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out
}

fn fit_to_display_width(text: &str, width: usize) -> String {
    let mut fitted = truncate_display_width(text, width);
    let used = UnicodeWidthStr::width(fitted.as_str());
    if used < width {
        fitted.push_str(&" ".repeat(width - used));
    }
    fitted
}

struct App {
    primary_provider: Provider,
    available_providers: Vec<Provider>,
    running: bool,
    should_quit: bool,
    spinner_idx: usize,
    mode: Mode,

    input: String,
    cursor: usize,
    pending_pastes: Vec<PendingPaste>,
    entries: Vec<LogEntry>,
    scroll: u16,
    autoscroll: bool,
    viewport_width: u16,
    viewport_height: u16,

    history: Vec<String>,
    history_pos: Option<usize>,
    history_query: String,
    history_idx: usize,

    commands: Vec<String>,
    slash_hint_idx: usize,

    approval: Option<PendingApproval>,
    allow_high_risk_tools: HashSet<String>,
    theme: ThemePreset,

    rx: Option<Receiver<WorkerEvent>>,
    assistant_idx: Option<usize>,
    stream_had_chunk: bool,
    agent_entries: HashMap<Provider, usize>,
    agent_had_chunk: HashMap<Provider, bool>,
    active_provider: Option<Provider>,
    run_started_at: Option<Instant>,
    run_target: String,
    last_tool_event: String,
    finished_at: Option<Instant>,
    finished_elapsed_secs: u64,
    finished_provider_name: String,
    agent_chars: HashMap<Provider, usize>,
    agent_verb_idx: HashMap<Provider, usize>,
    agent_started_at: HashMap<Provider, Instant>,
    agent_tool_event: HashMap<Provider, String>,
    /// Recent activity log entries shown in the activity area during runs.
    activity_log: std::collections::VecDeque<String>,

    last_status: String,
    session_id: String,
    memory: Option<MemoryStore>,
    child_pids: Arc<Mutex<Vec<u32>>>,

    /// Set by /clear to tell the main loop to wipe the terminal scrollback.
    needs_screen_clear: bool,

    /// Monotonically increasing counter bumped whenever entries change.
    render_generation: u64,
    /// Cached rendering output to avoid expensive recomputation every frame.
    render_cache: RenderCache,
}

impl App {
    fn new() -> Self {
        let available_providers = detect_available_providers();
        let primary_provider = if available_providers.contains(&Provider::Claude) {
            Provider::Claude
        } else {
            *available_providers.first().unwrap_or(&Provider::Claude)
        };
        let memory = if cfg!(test) {
            None
        } else {
            MemoryStore::open_default().ok()
        };
        let mut app = Self {
            primary_provider,
            available_providers,
            running: false,
            should_quit: false,
            spinner_idx: 0,
            mode: Mode::Normal,
            input: String::new(),
            cursor: 0,
            pending_pastes: Vec::new(),
            entries: Vec::new(),
            scroll: 0,
            autoscroll: true,
            viewport_width: 120,
            viewport_height: 36,
            history: Vec::new(),
            history_pos: None,
            history_query: String::new(),
            history_idx: 0,
            commands: default_commands(),
            slash_hint_idx: 0,
            approval: None,
            allow_high_risk_tools: HashSet::new(),
            theme: default_theme(),
            rx: None,
            assistant_idx: None,
            stream_had_chunk: false,
            agent_entries: HashMap::new(),
            agent_had_chunk: HashMap::new(),
            active_provider: None,
            run_started_at: None,
            run_target: String::new(),
            last_tool_event: String::new(),
            finished_at: None,
            finished_elapsed_secs: 0,
            finished_provider_name: String::new(),
            agent_chars: HashMap::new(),
            agent_verb_idx: HashMap::new(),
            agent_started_at: HashMap::new(),
            agent_tool_event: HashMap::new(),
            activity_log: std::collections::VecDeque::new(),
            last_status: "ready".to_string(),
            session_id: default_session_id(),
            memory,
            child_pids: Arc::new(Mutex::new(Vec::new())),
            needs_screen_clear: false,
            render_generation: 0,
            render_cache: RenderCache::new(),
        };
        app.restore_session();
        app.maybe_show_startup_banner();
        app
    }

    /// Bump the render generation to invalidate the render cache.
    fn invalidate_render_cache(&mut self) {
        self.render_generation = self.render_generation.wrapping_add(1);
    }

    fn start_running_state(&mut self, target: String) {
        self.running = true;
        self.finished_at = None;
        self.run_started_at = Some(Instant::now());
        self.run_target = target;
    }

    fn clear_running_state(&mut self) {
        self.running = false;
        self.rx = None;
        self.assistant_idx = None;
        self.stream_had_chunk = false;
        self.agent_entries.clear();
        self.agent_had_chunk.clear();
        self.agent_chars.clear();
        self.agent_verb_idx.clear();
        self.agent_started_at.clear();
        self.agent_tool_event.clear();
        self.activity_log.clear();
        self.active_provider = None;
        self.run_started_at = None;
        self.run_target.clear();
        if let Ok(mut pids) = self.child_pids.lock() {
            pids.clear();
        }
    }

    #[allow(dead_code)]
    pub(super) fn theme_name(&self) -> &'static str {
        self.theme.as_str()
    }

    pub(super) fn theme_palette(&self) -> ThemePalette {
        self.theme.palette()
    }

    pub(super) fn running_elapsed_secs(&self) -> u64 {
        self.run_started_at
            .map(|started| started.elapsed().as_secs())
            .unwrap_or(0)
    }

    fn push_entry(&mut self, kind: EntryKind, text: impl Into<String>) {
        self.entries.push(LogEntry {
            kind,
            text: text.into(),
            elapsed_secs: None,
        });
        self.follow_scroll();
    }

    fn last_system_entry_is(&self, text: &str) -> bool {
        self.entries
            .last()
            .is_some_and(|entry| matches!(entry.kind, EntryKind::System) && entry.text == text)
    }

    /// Invalidate render cache and update scroll to follow content.
    /// Call after any mutation of entries (push, in-place text change, etc.).
    fn follow_scroll(&mut self) {
        self.invalidate_render_cache();
        if self.autoscroll {
            self.scroll = self.scroll_max();
        } else {
            self.scroll = self.scroll.min(self.scroll_max());
        }
    }

    /// Ensure the render cache is up-to-date for the current state.
    /// Returns true if the cache was rebuilt.
    fn ensure_render_cache(&mut self) -> bool {
        let need_rebuild = self.render_cache.generation != self.render_generation
            || self.render_cache.width != self.viewport_width
            || self.render_cache.height != self.viewport_height;
        if !need_rebuild {
            return false;
        }

        let w = self.viewport_width.max(1);
        let h = self.viewport_height;
        let lines = self.render_log_lines_inner(w);

        // Compute scroll_max
        let prompt_width = UnicodeWidthStr::width("> ") as u16;
        let max_input_height = h.saturating_sub(6).max(1);
        let input_height = self
            .input_height(w, prompt_width)
            .saturating_add(2)
            .min(max_input_height);
        // fixed height: input + activity line + hints + status
        let fixed_h = input_height.saturating_add(3);
        let available_for_log = h.saturating_sub(fixed_h);
        let paragraph = Paragraph::new(Text::from(lines.clone())).wrap(Wrap { trim: false });
        let rendered_line_count = paragraph.line_count(w) as u16;
        let scroll_max = rendered_line_count.saturating_sub(available_for_log);

        self.render_cache = RenderCache {
            generation: self.render_generation,
            width: self.viewport_width,
            height: self.viewport_height,
            lines,
            scroll_max,
        };
        true
    }

    fn scroll_max(&mut self) -> u16 {
        self.ensure_render_cache();
        self.render_cache.scroll_max
    }

    pub(super) fn cached_log_lines(&self) -> &[Line<'static>] {
        &self.render_cache.lines
    }

    /// Render only the entries that are stable (not currently being streamed).
    /// This includes Tool/System/Error entries that appear after streaming
    /// Assistant entries, so the user can see tool activity in real-time.
    fn stable_log_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        // Collect indices of streaming assistant entries to skip.
        let mut streaming_indices = std::collections::HashSet::new();
        if let Some(idx) = self.assistant_idx {
            streaming_indices.insert(idx);
        }
        for &idx in self.agent_entries.values() {
            streaming_indices.insert(idx);
        }
        if streaming_indices.is_empty() {
            // No streaming entries; everything is stable.
            return self.render_entries_lines(width);
        }
        self.render_entries_lines_filtered(width, &streaming_indices)
    }
    fn update_viewport(&mut self, width: u16, height: u16) {
        self.viewport_width = width.max(1);
        self.viewport_height = height.max(1);
        let max_scroll = self.scroll_max();
        if self.autoscroll {
            self.scroll = max_scroll;
        } else {
            self.scroll = self.scroll.min(max_scroll);
        }
    }
    fn scroll_up(&mut self, n: u16) {
        let from = if self.autoscroll {
            self.scroll_max()
        } else {
            self.scroll
        };
        self.autoscroll = false;
        self.scroll = from.saturating_sub(n);
    }

    fn scroll_down(&mut self, n: u16) {
        let max_scroll = self.scroll_max();
        self.scroll = self.scroll.saturating_add(n).min(max_scroll);
        if self.scroll >= max_scroll {
            self.autoscroll = true;
        }
    }

    fn input_height(&self, width: u16, prompt_width: u16) -> u16 {
        if self.input.is_empty() {
            return 1;
        }
        let (_, end_y) = input_cursor_position(&self.input, self.input.len(), width, prompt_width);
        end_y.saturating_add(1).max(1)
    }

    /// Returns the vertical scroll offset needed to keep the cursor visible
    /// within the input area of the given `visible_rows` height.
    fn input_scroll_offset(&self, width: u16, prompt_width: u16, visible_rows: u16) -> u16 {
        if self.input.is_empty() {
            return 0;
        }
        let (_, cursor_y) = input_cursor_position(&self.input, self.cursor, width, prompt_width);
        // Scroll so that the cursor line is always within the visible area.
        cursor_y.saturating_sub(visible_rows.saturating_sub(1))
    }

    fn handle_paste_event(&mut self, raw: &str) {
        let normalized = if raw.contains('\r') {
            raw.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            raw.to_string()
        };
        if normalized.is_empty() {
            return;
        }

        let char_count = normalized.chars().count();
        let line_count = normalized.lines().count();
        let should_collapse = char_count >= COLLAPSED_PASTE_CHAR_THRESHOLD
            || line_count >= COLLAPSED_PASTE_LINE_THRESHOLD;

        if should_collapse {
            let marker = format!("[Pasted Content {} chars]", char_count);
            self.pending_pastes.push(PendingPaste {
                marker: marker.clone(),
                content: normalized,
            });
            self.insert_str(&marker);
            self.last_status = format!("pasted {} chars (collapsed)", char_count);
        } else {
            self.insert_str(&normalized);
        }
    }

    fn consume_pending_pastes(&mut self, text: &str) -> String {
        if self.pending_pastes.is_empty() {
            return text.to_string();
        }
        let mut merged = text.to_string();
        let mut search_from = 0usize;
        for pending in self.pending_pastes.drain(..) {
            if let Some(rel) = merged[search_from..].find(&pending.marker) {
                let start = search_from + rel;
                let end = start + pending.marker.len();
                merged.replace_range(start..end, &pending.content);
                search_from = start + pending.content.len();
            }
        }
        merged
    }

    fn clear_input_buffer(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.pending_pastes.clear();
    }

    fn maybe_show_startup_banner(&mut self) {
        if !self.entries.is_empty() {
            return;
        }
        let cwd = std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        self.push_entry(
            EntryKind::System,
            format!(
                "{STARTUP_BANNER_PREFIX}title|DAgent {} ready",
                env!("CARGO_PKG_VERSION")
            ),
        );
        self.push_entry(
            EntryKind::System,
            format!(
                "{STARTUP_BANNER_PREFIX}agents|agents: {} | primary: {}",
                providers_label(&self.available_providers),
                self.primary_provider.as_str()
            ),
        );
        self.push_entry(
            EntryKind::System,
            format!("{STARTUP_BANNER_PREFIX}cwd|cwd: {}", cwd),
        );
        self.push_entry(
            EntryKind::System,
            format!(
                "{STARTUP_BANNER_PREFIX}keys|keys: Enter send | Shift+Enter newline | Ctrl+R history"
            ),
        );
        self.last_status = "ready".to_string();
    }

    fn build_contextual_prompt(&self, prompt: &str) -> String {
        if let Some(memory) = &self.memory {
            if let Ok(text) = memory.build_context(&self.session_id, prompt) {
                return text;
            }
        }
        self.build_contextual_prompt_from_entries(prompt)
    }

    fn build_contextual_prompt_from_entries(&self, prompt: &str) -> String {
        const CONTEXT_ENTRY_LIMIT: usize = 18;
        const CONTEXT_CHAR_LIMIT: usize = 6000;

        let mut lines = Vec::<String>::new();
        let mut skipped_current_prompt = false;
        for entry in self.entries.iter().rev() {
            if lines.len() >= CONTEXT_ENTRY_LIMIT {
                break;
            }
            match entry.kind {
                EntryKind::User => {
                    let text = entry.text.trim();
                    if !skipped_current_prompt && text == prompt.trim() {
                        skipped_current_prompt = true;
                        continue;
                    }
                    if !text.is_empty() {
                        lines.push(format!("user: {}", text));
                    }
                }
                EntryKind::Assistant => {
                    let text = cleaned_assistant_text(entry);
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    let actor = extract_agent_name(&entry.text)
                        .map(|name| format!("assistant({name})"))
                        .unwrap_or_else(|| "assistant".to_string());
                    lines.push(format!("{actor}: {text}"));
                }
                _ => {}
            }
        }

        if lines.is_empty() {
            return prompt.to_string();
        }

        lines.reverse();
        let mut selected = Vec::<String>::new();
        let mut used = 0usize;
        for line in lines.into_iter().rev() {
            let delta = line.len().saturating_add(1);
            if used + delta > CONTEXT_CHAR_LIMIT && !selected.is_empty() {
                break;
            }
            used += delta;
            selected.push(line);
        }
        selected.reverse();

        format!(
            "Conversation context from this DAgent session:\n{}\n\nCurrent user request:\n{}",
            selected.join("\n"),
            prompt
        )
    }

    fn session_file_path() -> PathBuf {
        if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(".dagent").join("session.json")
        } else {
            PathBuf::from(".dagent").join("session.json")
        }
    }

    fn restore_session(&mut self) {
        let path = Self::session_file_path();
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&raw) else {
            return;
        };

        if self
            .available_providers
            .contains(&snapshot.primary_provider)
        {
            self.primary_provider = snapshot.primary_provider;
        }
        self.theme = snapshot.theme;
        if restore_transcript_on_start(self.memory.is_some()) {
            self.entries = snapshot.entries;
        } else {
            self.entries = Vec::new();
        }
        self.history = snapshot.history;
        self.session_id = if snapshot.session_id.trim().is_empty() {
            default_session_id()
        } else {
            snapshot.session_id
        };
        self.history_pos = None;
        self.autoscroll = true;
        self.scroll = self.scroll_max();
        if !self.entries.is_empty() {
            self.last_status = "resumed previous session".to_string();
        }
    }

    fn persist_session(&self) {
        const MAX_PERSISTED_ENTRIES: usize = 600;
        const MAX_PERSISTED_HISTORY: usize = 400;

        let path = Self::session_file_path();
        if let Some(parent) = path.parent() {
            if fs::create_dir_all(parent).is_err() {
                return;
            }
        }

        let entries = if self.entries.len() > MAX_PERSISTED_ENTRIES {
            self.entries[self.entries.len().saturating_sub(MAX_PERSISTED_ENTRIES)..].to_vec()
        } else {
            self.entries.clone()
        };
        let history = if self.history.len() > MAX_PERSISTED_HISTORY {
            self.history[self.history.len().saturating_sub(MAX_PERSISTED_HISTORY)..].to_vec()
        } else {
            self.history.clone()
        };
        let snapshot = SessionSnapshot {
            primary_provider: self.primary_provider,
            theme: self.theme,
            entries,
            history,
            session_id: self.session_id.clone(),
        };

        let Ok(serialized) = serde_json::to_string_pretty(&snapshot) else {
            return;
        };
        let _ = fs::write(path, serialized);
    }

    fn interrupt_running_task(&mut self, reason: &str) {
        if !self.running {
            return;
        }

        if let Ok(mut pids) = self.child_pids.lock() {
            for &pid in pids.iter() {
                kill_pid(pid);
            }
            pids.clear();
        }

        for &idx in self.agent_entries.values() {
            if let Some(entry) = self.entries.get_mut(idx) {
                if entry.text.contains(THINKING_PLACEHOLDER) {
                    entry.text = entry
                        .text
                        .replacen(THINKING_PLACEHOLDER, "(interrupted)", 1);
                } else if entry.text.trim().is_empty() {
                    entry.text = "(interrupted)".to_string();
                }
            }
        }
        if let Some(i) = self.assistant_idx {
            if let Some(entry) = self.entries.get_mut(i) {
                if entry.text.contains(THINKING_PLACEHOLDER) {
                    entry.text = entry
                        .text
                        .replacen(THINKING_PLACEHOLDER, "(interrupted)", 1);
                } else if entry.text.trim().is_empty() {
                    entry.text = "(interrupted)".to_string();
                }
            }
        }

        self.clear_running_state();
        self.last_tool_event = "task interrupted".to_string();
        self.last_status = "interrupted".to_string();
        self.push_entry(EntryKind::System, reason.to_string());
    }

    fn poll_worker(&mut self) -> bool {
        if let Some(rx) = self.rx.clone() {
            let mut processed_any = false;
            let mut render_changed = false;
            loop {
                match rx.try_recv() {
                    Ok(WorkerEvent::AgentStart(provider)) => {
                        processed_any = true;
                        self.active_provider = Some(provider);
                        self.agent_chars.insert(provider, 0);
                        let seed = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos() as usize)
                            .unwrap_or(self.spinner_idx);
                        self.agent_verb_idx.insert(provider, seed % 12);
                        self.agent_started_at.insert(provider, Instant::now());
                        let event_msg = format!("agent {} started", provider.as_str());
                        self.agent_tool_event.insert(provider, event_msg.clone());
                        self.last_tool_event = event_msg;
                        self.last_status = format!("{} working", provider.as_str());
                    }
                    Ok(WorkerEvent::AgentChunk { provider, chunk }) => {
                        processed_any = true;
                        let chunk = sanitize_runtime_text(&chunk);
                        if chunk.trim().is_empty() {
                            continue;
                        }
                        *self.agent_chars.entry(provider).or_insert(0) += chunk.len();
                        if let Some(i) = self.agent_entries.get(&provider).copied() {
                            if let Some(entry) = self.entries.get_mut(i) {
                                let had_chunk = self
                                    .agent_had_chunk
                                    .get(&provider)
                                    .copied()
                                    .unwrap_or(false);
                                if !had_chunk && entry.text.contains(THINKING_PLACEHOLDER) {
                                    entry.text = entry.text.replacen(THINKING_PLACEHOLDER, "", 1);
                                }
                                if provider == Provider::Codex
                                    && had_chunk
                                    && !entry.text.ends_with('\n')
                                    && !chunk.starts_with('\n')
                                {
                                    entry.text.push('\n');
                                }
                                entry.text.push_str(&chunk);
                                self.agent_had_chunk.insert(provider, true);
                                render_changed = true;
                            }
                        }
                        self.last_status = format!("{} streaming", provider.as_str());
                    }
                    Ok(WorkerEvent::AgentDone(provider)) => {
                        processed_any = true;
                        render_changed = true;
                        let elapsed_secs = self.running_elapsed_secs();
                        if let Some(i) = self.agent_entries.get(&provider).copied() {
                            let had_chunk = self
                                .agent_had_chunk
                                .get(&provider)
                                .copied()
                                .unwrap_or(false);
                            if let Some(entry) = self.entries.get_mut(i) {
                                entry.elapsed_secs = Some(elapsed_secs);
                                if !had_chunk {
                                    if entry.text.contains(THINKING_PLACEHOLDER) {
                                        entry.text = entry.text.replacen(
                                            THINKING_PLACEHOLDER,
                                            "(no output)",
                                            1,
                                        );
                                    } else if entry.text.trim().is_empty() {
                                        entry.text = "(no output)".to_string();
                                    }
                                }
                            }
                        }
                        if let Some(i) = self.agent_entries.get(&provider).copied() {
                            if let Some(entry) = self.entries.get(i) {
                                let text = cleaned_assistant_text(entry).trim().to_string();
                                if !text.is_empty()
                                    && text != "(no output)"
                                    && text != "(failed)"
                                    && text != "(interrupted)"
                                    && text != "(cancelled)"
                                    && text != "(disconnected)"
                                {
                                    if let Some(memory) = &self.memory {
                                        if let Err(err) = memory.append_message(
                                            &self.session_id,
                                            "assistant",
                                            Some(provider.as_str()),
                                            &text,
                                        ) {
                                            self.push_entry(
                                                EntryKind::System,
                                                format!(
                                                    "memory write failed: {}",
                                                    truncate(&err.to_string(), 80)
                                                ),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        if self.active_provider == Some(provider) {
                            self.active_provider = None;
                        }
                        let event_msg = format!(
                            "agent {} completed ({:02}:{:02})",
                            provider.as_str(),
                            elapsed_secs / 60,
                            elapsed_secs % 60
                        );
                        self.agent_tool_event.insert(provider, event_msg.clone());
                        self.last_tool_event = event_msg;
                        self.last_status = format!("{} done", provider.as_str());
                    }
                    Ok(WorkerEvent::Done(final_text)) => {
                        processed_any = true;
                        render_changed = true;
                        let elapsed_secs = self.running_elapsed_secs();
                        self.finished_elapsed_secs = elapsed_secs;
                        self.finished_provider_name = self.primary_provider.as_str().to_string();
                        if self.assistant_idx.is_some() {
                            if !self.stream_had_chunk {
                                let final_text = final_text.trim();
                                if let Some(i) = self.assistant_idx {
                                    if let Some(entry) = self.entries.get_mut(i) {
                                        if entry.elapsed_secs.is_none() {
                                            entry.elapsed_secs = Some(elapsed_secs);
                                        }
                                        if final_text.is_empty() {
                                            if entry.text.trim().is_empty()
                                                || entry.text.trim() == THINKING_PLACEHOLDER
                                            {
                                                entry.text = "(no output)".to_string();
                                            }
                                        } else if entry.text.trim() == THINKING_PLACEHOLDER {
                                            entry.text = final_text.to_string();
                                        } else {
                                            entry.text.push_str(final_text);
                                        }
                                    }
                                }
                            } else if let Some(i) = self.assistant_idx {
                                if let Some(entry) = self.entries.get_mut(i) {
                                    if entry.elapsed_secs.is_none() {
                                        entry.elapsed_secs = Some(elapsed_secs);
                                    }
                                    if entry.text.trim().is_empty() {
                                        entry.text = "(no output)".to_string();
                                    }
                                }
                            }
                        } else if !final_text.trim().is_empty() {
                            if let Some(primary_idx) =
                                self.agent_entries.get(&self.primary_provider).copied()
                            {
                                if let Some(entry) = self.entries.get_mut(primary_idx) {
                                    if entry.elapsed_secs.is_none() {
                                        entry.elapsed_secs = Some(elapsed_secs);
                                    }
                                    if entry.text.contains(THINKING_PLACEHOLDER) {
                                        entry.text =
                                            entry.text.replacen(THINKING_PLACEHOLDER, "", 1);
                                    }
                                    entry.text.push_str(final_text.trim());
                                }
                            }
                        }
                        self.clear_running_state();
                        self.finished_at = Some(Instant::now());
                        if self.last_tool_event.is_empty() {
                            self.last_tool_event = "run completed".to_string();
                        }
                        self.last_status = "done".to_string();
                        break;
                    }
                    Ok(WorkerEvent::Tool(msg)) => {
                        processed_any = true;
                        let msg = sanitize_runtime_text(&msg);
                        if msg.trim().is_empty() {
                            continue;
                        }
                        if let Some(ap) = self.active_provider {
                            self.agent_tool_event.insert(ap, msg.clone());
                        }
                        self.last_tool_event = msg.clone();
                        self.last_status = format!("tool: {}", truncate(&msg, 48));
                        // Tool events only go to the live activity area (not transcript).
                        const MAX_ACTIVITY_LOG: usize = 5;
                        self.activity_log.push_back(msg);
                        while self.activity_log.len() > MAX_ACTIVITY_LOG {
                            self.activity_log.pop_front();
                        }
                        render_changed = true;
                    }
                    Ok(WorkerEvent::Progress(msg)) => {
                        processed_any = true;
                        let msg = sanitize_runtime_text(&msg);
                        if msg.trim().is_empty() {
                            continue;
                        }
                        if let Some(ap) = self.active_provider {
                            self.agent_tool_event.insert(ap, msg.clone());
                        }
                        self.last_tool_event = msg.clone();
                        // Add progress to activity log.
                        const MAX_ACTIVITY_LOG_P: usize = 5;
                        self.activity_log.push_back(msg.clone());
                        while self.activity_log.len() > MAX_ACTIVITY_LOG_P {
                            self.activity_log.pop_front();
                        }
                        self.last_status = format!("progress: {}", truncate(&msg, 48));
                        // Progress events only update the spinner area;
                        // they are NOT written to the transcript.
                    }
                    Ok(WorkerEvent::PromotePrimary { to, reason }) => {
                        processed_any = true;
                        if self.primary_provider != to {
                            self.primary_provider = to;
                            self.push_entry(
                                EntryKind::System,
                                format!("primary auto-switched to {} ({})", to.as_str(), reason),
                            );
                            self.last_status = format!("primary -> {}", to.as_str());
                        }
                    }
                    Ok(WorkerEvent::Error(err)) => {
                        processed_any = true;
                        render_changed = true;
                        if let Some(provider) = self.active_provider {
                            if let Some(i) = self.agent_entries.get(&provider).copied() {
                                if let Some(entry) = self.entries.get_mut(i) {
                                    if entry.text.contains(THINKING_PLACEHOLDER) {
                                        entry.text = entry.text.replacen(
                                            THINKING_PLACEHOLDER,
                                            "(failed)",
                                            1,
                                        );
                                    }
                                }
                            }
                        } else if let Some(i) = self.assistant_idx {
                            if let Some(entry) = self.entries.get_mut(i) {
                                if entry.text.trim().is_empty()
                                    || entry.text.trim() == THINKING_PLACEHOLDER
                                {
                                    entry.text = "(failed)".to_string();
                                }
                            }
                        }
                        self.push_entry(EntryKind::Error, err);
                        self.clear_running_state();
                        self.last_tool_event = "run failed".to_string();
                        self.last_status = "error".to_string();
                        break;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        processed_any = true;
                        render_changed = true;
                        if let Some(provider) = self.active_provider {
                            if let Some(i) = self.agent_entries.get(&provider).copied() {
                                if let Some(entry) = self.entries.get_mut(i) {
                                    if entry.text.contains(THINKING_PLACEHOLDER) {
                                        entry.text = entry.text.replacen(
                                            THINKING_PLACEHOLDER,
                                            "(disconnected)",
                                            1,
                                        );
                                    }
                                }
                            }
                        } else if let Some(i) = self.assistant_idx {
                            if let Some(entry) = self.entries.get_mut(i) {
                                if entry.text.trim().is_empty()
                                    || entry.text.trim() == THINKING_PLACEHOLDER
                                {
                                    entry.text = "(disconnected)".to_string();
                                }
                            }
                        }
                        self.clear_running_state();
                        self.last_tool_event = "worker disconnected".to_string();
                        break;
                    }
                }
            }
            if render_changed {
                self.follow_scroll();
            }
            processed_any
        } else {
            false
        }
    }

    fn submit_current_line(&mut self, force: bool) {
        let typed_line = self.input.trim().to_string();
        if typed_line.is_empty() {
            return;
        }

        if typed_line == "/exit" || typed_line == "/quit" {
            self.should_quit = true;
            return;
        }

        if self.running {
            let msg = "task is running, wait...";
            if !self.last_system_entry_is(msg) {
                self.push_entry(EntryKind::System, msg);
            }
            return;
        }

        let mut dispatch_target = DispatchTarget::Primary;
        let mut line = typed_line.clone();
        if !typed_line.starts_with('/') {
            match parse_dispatch_override(&typed_line) {
                Ok(Some((target, prompt))) => {
                    dispatch_target = target;
                    line = prompt;
                }
                Ok(None) => {}
                Err(err) => {
                    self.push_entry(EntryKind::Error, err);
                    self.clear_input_buffer();
                    return;
                }
            }
        }

        if let Some((tool, reason)) = high_risk_check(&line) {
            if !force && !self.allow_high_risk_tools.contains(&tool) {
                self.approval = Some(PendingApproval { line, tool, reason });
                self.mode = Mode::Approval;
                return;
            }
        }

        if line == "/clear" {
            self.entries.clear();
            self.invalidate_render_cache();
            self.needs_screen_clear = true;
            if let Some(memory) = &self.memory {
                if let Err(err) = memory.clear_session(&self.session_id) {
                    self.push_entry(
                        EntryKind::System,
                        format!("memory clear failed: {}", truncate(&err.to_string(), 80)),
                    );
                }
            }
            self.clear_input_buffer();
            self.last_status = "cleared".to_string();
            return;
        }

        if let Some(rest) = line.strip_prefix("/mem") {
            self.handle_memory_command(rest.trim());
            self.clear_input_buffer();
            return;
        }

        if let Some(rest) = line.strip_prefix("/theme") {
            self.handle_theme_change(rest.trim());
            self.clear_input_buffer();
            return;
        }

        if let Some(rest) = line.strip_prefix("/provider") {
            let target = rest.trim();
            self.handle_primary_change(target);
            self.clear_input_buffer();
            return;
        }

        if let Some(rest) = line.strip_prefix("/primary") {
            let target = rest.trim();
            self.handle_primary_change(target);
            self.clear_input_buffer();
            return;
        }

        self.history.push(typed_line.clone());
        self.history_pos = None;

        let is_slash = line.starts_with('/');
        let providers = if is_slash {
            Vec::new()
        } else {
            match &dispatch_target {
                DispatchTarget::Primary => {
                    if self.available_providers.contains(&self.primary_provider) {
                        vec![self.primary_provider]
                    } else {
                        Vec::new()
                    }
                }
                DispatchTarget::All => {
                    ordered_providers(self.primary_provider, &self.available_providers)
                }
                DispatchTarget::Provider(provider) => {
                    if self.available_providers.contains(provider) {
                        vec![*provider]
                    } else {
                        Vec::new()
                    }
                }
                DispatchTarget::Providers(targets) => targets
                    .iter()
                    .copied()
                    .filter(|provider| self.available_providers.contains(provider))
                    .fold(Vec::new(), |mut acc, provider| {
                        if !acc.contains(&provider) {
                            acc.push(provider);
                        }
                        acc
                    }),
            }
        };
        if !is_slash && providers.is_empty() {
            let msg = match &dispatch_target {
                DispatchTarget::Primary => format!(
                    "primary agent {} not available on PATH",
                    self.primary_provider.as_str()
                ),
                DispatchTarget::All => {
                    "no available agent found (need claude and/or codex on PATH)".to_string()
                }
                DispatchTarget::Provider(provider) => {
                    format!("{} not available on PATH", provider.as_str())
                }
                DispatchTarget::Providers(targets) => {
                    let missing = targets
                        .iter()
                        .filter(|provider| !self.available_providers.contains(provider))
                        .map(|provider| provider.as_str())
                        .collect::<Vec<_>>();
                    if missing.is_empty() {
                        "requested agents not available on PATH".to_string()
                    } else {
                        format!(
                            "requested agents not available on PATH: {}",
                            missing.join(",")
                        )
                    }
                }
            };
            self.push_entry(EntryKind::Error, msg);
            self.clear_input_buffer();
            return;
        }

        line = self.consume_pending_pastes(&line);
        self.push_entry(EntryKind::User, typed_line);
        if !is_slash {
            if let Some(memory) = &self.memory {
                if let Err(err) = memory.append_message(&self.session_id, "user", None, &line) {
                    self.push_entry(
                        EntryKind::System,
                        format!("memory write failed: {}", truncate(&err.to_string(), 80)),
                    );
                }
            }
        }
        self.assistant_idx = None;
        self.agent_entries.clear();
        self.agent_had_chunk.clear();
        self.active_provider = None;
        let run_target = if is_slash {
            "command".to_string()
        } else {
            providers_label(&providers)
        };
        if is_slash {
            self.push_entry(EntryKind::Assistant, THINKING_PLACEHOLDER.to_string());
            self.assistant_idx = Some(self.entries.len() - 1);
        } else {
            for provider in providers.iter().copied() {
                self.push_entry(
                    EntryKind::Assistant,
                    format!("[{}]\n{}", provider.as_str(), THINKING_PLACEHOLDER),
                );
                self.agent_entries.insert(provider, self.entries.len() - 1);
                self.agent_had_chunk.insert(provider, false);
                if self.active_provider.is_none() {
                    self.active_provider = Some(provider);
                }
            }
        }
        self.autoscroll = true;
        self.scroll = self.scroll_max();
        self.stream_had_chunk = false;
        self.start_running_state(run_target.clone());
        self.last_tool_event.clear();
        self.last_status = format!("dispatching {}", run_target);
        self.clear_input_buffer();

        let line_for_worker = if line.starts_with("/") {
            line.clone()
        } else {
            self.build_contextual_prompt(&line)
        };

        let provider = self.primary_provider;
        let available = self.available_providers.clone();
        let child_pids: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        self.child_pids = child_pids.clone();
        let (tx, rx) = unbounded::<WorkerEvent>();
        let dispatch_target_for_worker = dispatch_target.clone();
        std::thread::spawn(move || {
            execute_line(
                provider,
                available,
                line_for_worker,
                dispatch_target_for_worker,
                tx,
                child_pids,
            )
        });
        self.rx = Some(rx);
    }

    fn handle_primary_change(&mut self, target: &str) {
        if target.is_empty() {
            self.push_entry(
                EntryKind::System,
                format!(
                    "primary: {} | agents: {}",
                    self.primary_provider.as_str(),
                    providers_label(&self.available_providers)
                ),
            );
            return;
        }

        let selected = match target {
            "claude" => Provider::Claude,
            "codex" => Provider::Codex,
            _ => {
                self.push_entry(EntryKind::Error, "usage: /primary [claude|codex]");
                return;
            }
        };

        if !self.available_providers.contains(&selected) {
            self.push_entry(
                EntryKind::Error,
                format!("{} not available on PATH", selected.as_str()),
            );
            return;
        }

        self.primary_provider = selected;
        self.push_entry(
            EntryKind::System,
            format!("primary set to {}", self.primary_provider.as_str()),
        );
    }

    fn handle_theme_change(&mut self, target: &str) {
        if target.is_empty() {
            self.push_entry(
                EntryKind::System,
                format!(
                    "theme: {} | options: fjord, graphite, solarized, aurora, ember",
                    self.theme.as_str()
                ),
            );
            return;
        }
        let Some(theme) = ThemePreset::parse(target) else {
            self.push_entry(
                EntryKind::Error,
                "usage: /theme [fjord|graphite|solarized|aurora|ember]",
            );
            return;
        };
        self.theme = theme;
        self.invalidate_render_cache();
        self.last_status = format!("theme {}", self.theme.as_str());
        self.push_entry(
            EntryKind::System,
            format!("theme set to {}", self.theme.as_str()),
        );
    }

    fn handle_memory_command(&mut self, args: &str) {
        let usage = [
            "memory commands",
            "  /mem                     show summary",
            "  /mem show [n]            show latest n records (default 20)",
            "  /mem find <query>        search memory in this session",
            "  /mem prune [keep]        keep latest N records (default 200)",
            "  /mem clear               clear memory only (keep transcript)",
        ]
        .join("\n");

        let Some(memory) = self.memory.as_ref() else {
            self.push_entry(EntryKind::Error, "memory backend unavailable");
            self.last_status = "memory unavailable".to_string();
            return;
        };

        let mut parts = args.split_whitespace();
        let sub = parts.next().unwrap_or("");

        if sub.is_empty() || sub == "help" {
            match memory.session_message_count(&self.session_id) {
                Ok(count) => {
                    self.push_entry(
                        EntryKind::System,
                        format!("session memory: {} records\n{}", count, usage),
                    );
                    self.last_status = format!("memory {} records", count);
                }
                Err(err) => {
                    self.push_entry(
                        EntryKind::Error,
                        format!("memory read failed: {}", truncate(&err.to_string(), 80)),
                    );
                    self.last_status = "memory error".to_string();
                }
            }
            return;
        }

        match sub {
            "show" => {
                let limit = match parts.next() {
                    Some(raw) => match raw.parse::<usize>() {
                        Ok(v) if v > 0 => v.min(MEM_SHOW_MAX_LIMIT),
                        _ => {
                            self.push_entry(
                                EntryKind::Error,
                                "usage: /mem show [positive-number]",
                            );
                            self.last_status = "memory usage".to_string();
                            return;
                        }
                    },
                    None => MEM_SHOW_DEFAULT_LIMIT,
                };
                if parts.next().is_some() {
                    self.push_entry(EntryKind::Error, "usage: /mem show [n]");
                    self.last_status = "memory usage".to_string();
                    return;
                }

                match memory.list_session_lines(&self.session_id, limit) {
                    Ok(lines) => {
                        if lines.is_empty() {
                            self.push_entry(EntryKind::System, "memory is empty");
                            self.last_status = "memory empty".to_string();
                        } else {
                            self.push_entry(
                                EntryKind::System,
                                format!("memory (latest {}):\n{}", lines.len(), lines.join("\n")),
                            );
                            self.last_status = format!("memory show {}", lines.len());
                        }
                    }
                    Err(err) => {
                        self.push_entry(
                            EntryKind::Error,
                            format!("memory read failed: {}", truncate(&err.to_string(), 80)),
                        );
                        self.last_status = "memory error".to_string();
                    }
                }
            }
            "find" => {
                let query = args
                    .strip_prefix("find")
                    .map(str::trim)
                    .unwrap_or_default();
                if query.is_empty() {
                    self.push_entry(EntryKind::Error, "usage: /mem find <query>");
                    self.last_status = "memory usage".to_string();
                    return;
                }

                match memory.search_session_lines(&self.session_id, query, MEM_FIND_DEFAULT_LIMIT) {
                    Ok(lines) => {
                        if lines.is_empty() {
                            self.push_entry(EntryKind::System, "memory search: no match");
                            self.last_status = "memory no match".to_string();
                        } else {
                            self.push_entry(
                                EntryKind::System,
                                format!(
                                    "memory search results ({}):\n{}",
                                    lines.len(),
                                    lines.join("\n")
                                ),
                            );
                            self.last_status = format!("memory match {}", lines.len());
                        }
                    }
                    Err(err) => {
                        self.push_entry(
                            EntryKind::Error,
                            format!("memory search failed: {}", truncate(&err.to_string(), 80)),
                        );
                        self.last_status = "memory error".to_string();
                    }
                }
            }
            "prune" | "trim" => {
                let keep = match parts.next() {
                    Some(raw) => match raw.parse::<usize>() {
                        Ok(v) => v,
                        Err(_) => {
                            self.push_entry(
                                EntryKind::Error,
                                "usage: /mem prune [non-negative-number]",
                            );
                            self.last_status = "memory usage".to_string();
                            return;
                        }
                    },
                    None => MEM_PRUNE_DEFAULT_KEEP,
                };
                if parts.next().is_some() {
                    self.push_entry(EntryKind::Error, "usage: /mem prune [keep]");
                    self.last_status = "memory usage".to_string();
                    return;
                }

                let prune_result = memory.prune_session_keep_recent(&self.session_id, keep);
                let count_result = memory.session_message_count(&self.session_id);
                match (prune_result, count_result) {
                    (Ok(removed), Ok(remaining)) => {
                        self.push_entry(
                            EntryKind::System,
                            format!(
                                "memory pruned: removed {}, remaining {}",
                                removed, remaining
                            ),
                        );
                        self.last_status = format!("memory pruned {}", removed);
                    }
                    (Err(err), _) | (_, Err(err)) => {
                        self.push_entry(
                            EntryKind::Error,
                            format!("memory prune failed: {}", truncate(&err.to_string(), 80)),
                        );
                        self.last_status = "memory error".to_string();
                    }
                }
            }
            "clear" => match memory.clear_session(&self.session_id) {
                Ok(()) => {
                    self.push_entry(EntryKind::System, "memory cleared for current session");
                    self.last_status = "memory cleared".to_string();
                }
                Err(err) => {
                    self.push_entry(
                        EntryKind::Error,
                        format!("memory clear failed: {}", truncate(&err.to_string(), 80)),
                    );
                    self.last_status = "memory error".to_string();
                }
            },
            _ => {
                self.push_entry(EntryKind::Error, "usage: /mem [show|find|prune|clear]");
                self.last_status = "memory usage".to_string();
            }
        }
    }

    fn filtered_history(&self) -> Vec<String> {
        let q = self.history_query.to_lowercase();
        if q.trim().is_empty() {
            return self.history.iter().rev().take(14).cloned().collect();
        }
        self.history
            .iter()
            .rev()
            .filter(|h| h.to_lowercase().contains(&q))
            .take(14)
            .cloned()
            .collect()
    }

    fn slash_hints(&self) -> Vec<String> {
        if !self.input.starts_with('/') {
            return Vec::new();
        }
        let query = self.input.trim();
        let mut matches: Vec<String> = self
            .commands
            .iter()
            .filter(|cmd| cmd.starts_with(query))
            .cloned()
            .collect();
        if matches.is_empty() && query == "/" {
            matches = self.commands.clone();
        }
        matches.into_iter().take(6).collect()
    }

    fn mention_query(token: &str) -> Option<String> {
        if !token.starts_with("@") {
            return None;
        }

        let mut query = String::from("@");
        for ch in token.chars().skip(1) {
            if ch.is_ascii_alphanumeric() || "_-".contains(ch) {
                query.push(ch);
            } else {
                break;
            }
        }
        Some(query)
    }

    fn active_mention_span(&self) -> Option<(usize, usize, String)> {
        if self.input.is_empty() {
            return None;
        }

        let cursor = self.cursor.min(self.input.len());
        let prefix = &self.input[..cursor];
        let start = prefix
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(i, ch)| i + ch.len_utf8())
            .unwrap_or(0);

        if start >= self.input.len() {
            return None;
        }

        let token_prefix = &self.input[start..cursor];
        if token_prefix.is_empty() || !token_prefix.starts_with("@") {
            return None;
        }

        let suffix = &self.input[cursor..];
        let end_rel = suffix
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(suffix.len());
        let end = cursor + end_rel;

        Some((start, end, self.input[start..end].to_string()))
    }

    fn agent_hints(&self) -> Vec<String> {
        let Some((_, _, active_token)) = self.active_mention_span() else {
            return Vec::new();
        };

        let query = Self::mention_query(&active_token).unwrap_or_else(|| "@".to_string());
        let already_selected: HashSet<&str> = self
            .input
            .split_whitespace()
            .filter(|token| token.starts_with("@") && *token != active_token.as_str())
            .collect();

        self.agent_hint_options()
            .into_iter()
            .filter(|opt| opt.starts_with(&query))
            .filter(|opt| !already_selected.contains(opt.as_str()))
            .take(6)
            .collect()
    }

    fn agent_hint_options(&self) -> Vec<String> {
        let mut options = vec!["@all".to_string()];
        for provider in ordered_providers(self.primary_provider, &self.available_providers) {
            options.push(format!("@{}", provider.as_str()));
        }
        for provider in Provider::all() {
            let mention = format!("@{}", provider.as_str());
            if !options.contains(&mention) {
                options.push(mention);
            }
        }
        options
    }

    fn inline_hints(&self) -> Vec<String> {
        if self.input.starts_with("/") {
            self.slash_hints()
        } else if self.active_mention_span().is_some() {
            self.agent_hints()
        } else {
            Vec::new()
        }
    }

    fn apply_selected_inline_hint(&mut self) -> bool {
        let hints = self.inline_hints();
        if hints.is_empty() {
            return false;
        }

        let idx = self.slash_hint_idx.min(hints.len().saturating_sub(1));
        let Some(selected) = hints.get(idx).cloned() else {
            return false;
        };

        if let Some((start, end, _)) = self.active_mention_span() {
            self.input.replace_range(start..end, &selected);
            let mut cursor = start + selected.len();
            let needs_space = self
                .input
                .get(cursor..)
                .and_then(|s| s.chars().next())
                .map(|c| !c.is_whitespace())
                .unwrap_or(true);
            if needs_space {
                self.input.insert_str(cursor, " ");
                cursor += 1;
            }
            self.cursor = cursor;
            return true;
        }

        if self.input.starts_with("/") {
            self.input = selected;
            self.cursor = self.input.len();
            return true;
        }

        false
    }
    fn sync_inline_hint_idx(&mut self) {
        let len = self.inline_hints().len();
        if len == 0 {
            self.slash_hint_idx = 0;
            return;
        }
        if self.slash_hint_idx >= len {
            self.slash_hint_idx = len - 1;
        }
    }

    fn cycle_inline_hint_next(&mut self) -> bool {
        let len = self.inline_hints().len();
        if len == 0 {
            return false;
        }
        self.slash_hint_idx = (self.slash_hint_idx + 1) % len;
        true
    }

    fn cycle_inline_hint_prev(&mut self) -> bool {
        let len = self.inline_hints().len();
        if len == 0 {
            return false;
        }
        if self.slash_hint_idx == 0 {
            self.slash_hint_idx = len - 1;
        } else {
            self.slash_hint_idx -= 1;
        }
        true
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_pos {
            None => self.history.len().saturating_sub(1),
            Some(i) => i.saturating_sub(1),
        };
        self.history_pos = Some(next);
        self.input = self.history[next].clone();
        self.cursor = self.input.len();
    }

    fn history_next(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let Some(i) = self.history_pos else {
            return;
        };
        if i + 1 >= self.history.len() {
            self.history_pos = None;
            self.input.clear();
            self.cursor = 0;
            return;
        }
        let next = i + 1;
        self.history_pos = Some(next);
        self.input = self.history[next].clone();
        self.cursor = self.input.len();
    }

    fn insert_char(&mut self, c: char) {
        if self.cursor >= self.input.len() {
            self.input.push(c);
        } else {
            self.input.insert(self.cursor, c);
        }
        self.cursor += c.len_utf8();
    }

    fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
        if self.input.starts_with("/") || self.active_mention_span().is_some() {
            self.slash_hint_idx = 0;
            self.sync_inline_hint_idx();
        }
    }

    fn backspace(&mut self) {
        if self.cursor == 0 || self.input.is_empty() {
            return;
        }
        if let Some(prev_idx) = self.input[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
        {
            self.input.drain(prev_idx..self.cursor);
            self.cursor = prev_idx;
        }
    }

    fn backspace_word(&mut self) {
        if self.cursor == 0 {
            return;
        }
        while self.cursor > 0 && self.input[..self.cursor].ends_with(' ') {
            self.backspace();
        }
        while self.cursor > 0 && !self.input[..self.cursor].ends_with(' ') {
            self.backspace();
        }
    }

    fn delete(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let mut iter = self.input[self.cursor..].char_indices();
        let Some((_, ch)) = iter.next() else {
            return;
        };
        let end = self.cursor + ch.len_utf8();
        self.input.drain(self.cursor..end);
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some(prev_idx) = self.input[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
        {
            self.cursor = prev_idx;
        }
    }

    fn move_right(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let mut iter = self.input[self.cursor..].char_indices();
        if let Some((_, ch)) = iter.next() {
            self.cursor += ch.len_utf8();
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Approval => self.handle_approval_key(key),
            Mode::HistorySearch => self.handle_history_key(key),
            Mode::Normal => self.handle_normal_key(key),
        }
    }

    fn handle_approval_key(&mut self, key: KeyEvent) {
        if let Some(pending) = self.approval.clone() {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.approval = None;
                    self.mode = Mode::Normal;
                    self.input = pending.line;
                    self.cursor = self.input.len();
                    self.submit_current_line(true);
                }
                KeyCode::Char('a') => {
                    self.allow_high_risk_tools.insert(pending.tool);
                    self.approval = None;
                    self.mode = Mode::Normal;
                    self.input = pending.line;
                    self.cursor = self.input.len();
                    self.submit_current_line(true);
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.push_entry(EntryKind::System, "approval denied");
                    self.approval = None;
                    self.mode = Mode::Normal;
                }
                _ => {}
            }
        } else {
            self.mode = Mode::Normal;
        }
    }

    fn handle_history_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Up => {
                if self.history_idx > 0 {
                    self.history_idx -= 1;
                }
            }
            KeyCode::Down => {
                let len = self.filtered_history().len();
                if len > 0 && self.history_idx + 1 < len {
                    self.history_idx += 1;
                }
            }
            KeyCode::Enter => {
                let items = self.filtered_history();
                if let Some(selected) = items.get(self.history_idx) {
                    self.input = selected.clone();
                    self.cursor = self.input.len();
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                self.history_query.pop();
                self.history_idx = 0;
            }
            KeyCode::Char(c) => {
                self.history_query.push(c);
                self.history_idx = 0;
            }
            _ => {}
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.interrupt_running_task("task interrupted (Ctrl+C)");
                    self.persist_session();
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('l') => {
                    self.entries.clear();
                    self.invalidate_render_cache();
                    self.last_status = "cleared".to_string();
                    return;
                }
                KeyCode::Char('a') => {
                    self.cursor = 0;
                    return;
                }
                KeyCode::Char('e') => {
                    self.cursor = self.input.len();
                    return;
                }
                KeyCode::Char('k') => {
                    // Ctrl+K command palette removed; keep as no-op.
                    return;
                }
                KeyCode::Char('r') => {
                    self.mode = Mode::HistorySearch;
                    self.history_query.clear();
                    self.history_idx = 0;
                    return;
                }
                KeyCode::Char('j') => {
                    self.insert_char('\n');
                    return;
                }
                KeyCode::Char('p') => {
                    self.history_prev();
                    return;
                }
                KeyCode::Char('n') => {
                    self.history_next();
                    return;
                }
                _ => {}
            }
        }

        if key.modifiers.contains(KeyModifiers::ALT) && matches!(key.code, KeyCode::Backspace) {
            self.backspace_word();
            return;
        }

        match key.code {
            KeyCode::PageUp => {
                self.scroll_up(5);
            }
            KeyCode::PageDown => {
                self.scroll_down(5);
            }
            KeyCode::Up => {
                let hints = self.inline_hints();
                if !hints.is_empty() {
                    if self.slash_hint_idx == 0 {
                        self.slash_hint_idx = hints.len() - 1;
                    } else {
                        self.slash_hint_idx -= 1;
                    }
                    return;
                }
                self.history_prev();
            }
            KeyCode::Down => {
                let hints = self.inline_hints();
                if !hints.is_empty() {
                    self.slash_hint_idx = (self.slash_hint_idx + 1) % hints.len();
                    return;
                }
                self.history_next();
            }
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    if self.cycle_inline_hint_prev() {
                        return;
                    }
                } else if self.cycle_inline_hint_next() {
                    return;
                }
            }
            KeyCode::Enter => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.insert_char('\n');
                } else {
                    let hint_count = self.inline_hints().len();
                    let should_apply_hint = hint_count > 1 || self.active_mention_span().is_some();
                    if should_apply_hint && self.apply_selected_inline_hint() {
                        return;
                    }
                    self.submit_current_line(false);
                }
            }
            KeyCode::Backspace => {
                self.backspace();
                self.sync_inline_hint_idx();
            }
            KeyCode::Delete => {
                self.delete();
                self.sync_inline_hint_idx();
            }
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Esc => {
                if self.running {
                    // Kill all child processes
                    if let Ok(pids) = self.child_pids.lock() {
                        for &pid in pids.iter() {
                            kill_pid(pid);
                        }
                    }
                    // Mark running entries as cancelled
                    for (_provider, &idx) in self.agent_entries.iter() {
                        if let Some(entry) = self.entries.get_mut(idx) {
                            if entry.text.contains(THINKING_PLACEHOLDER) {
                                entry.text =
                                    entry.text.replacen(THINKING_PLACEHOLDER, "(cancelled)", 1);
                            }
                        }
                    }
                    if let Some(idx) = self.assistant_idx {
                        if let Some(entry) = self.entries.get_mut(idx) {
                            if entry.text.trim() == THINKING_PLACEHOLDER {
                                entry.text = "(cancelled)".to_string();
                            }
                        }
                    }
                    self.clear_running_state();
                    self.last_tool_event = "task cancelled".to_string();
                    self.last_status = "cancelled".to_string();
                    self.push_entry(EntryKind::System, "task cancelled (Esc)");
                }
            }
            KeyCode::Char(c) => {
                self.insert_char(c);
                if self.input.starts_with("/") || self.active_mention_span().is_some() {
                    self.slash_hint_idx = 0;
                    self.sync_inline_hint_idx();
                }
            }
            _ => {}
        }
    }

    fn render_entries_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.render_entries_lines_range(width, 0, self.entries.len())
    }

    fn render_entries_lines_range(&self, width: u16, start: usize, end: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::<Line>::new();
        let palette = self.theme.palette();

        for (idx, entry) in self.entries[start..end].iter().enumerate().map(|(i, e)| (i + start, e)) {
            let entry_provider =
                extract_agent_name(&entry.text).and_then(|n| provider_from_name(&n));
            let is_current_entry =
                self.assistant_idx == Some(idx) || self.agent_entries.values().any(|&i| i == idx);
            let is_processing =
                self.running && matches!(entry.kind, EntryKind::Assistant) && is_current_entry;
            // Extra spacing before assistant entries for visual separation.
            if matches!(entry.kind, EntryKind::Assistant) && idx > 0 {
                lines.push(Line::from(""));
            }
            match entry.kind {
                EntryKind::User => {
                    let parts: Vec<&str> = entry.text.split('\n').collect();
                    let w = width as usize;
                    let user_style = Style::default()
                        .fg(palette.user_fg)
                        .bg(palette.user_bg)
                        .add_modifier(Modifier::BOLD);
                    for part in parts {
                        let content = if part.is_empty() { " " } else { part };
                        let mut text = format!(" {} ", content);
                        if w > 0 {
                            let text_w = UnicodeWidthStr::width(text.as_str());
                            if text_w < w {
                                text.push_str(&" ".repeat(w - text_w));
                            }
                        }
                        lines.push(Line::from(vec![Span::styled(text, user_style)]));
                    }
                }
                EntryKind::Assistant => {
                    let provider = entry_provider.unwrap_or(self.primary_provider);
                    let provider_color = match provider {
                        Provider::Claude => palette.claude_label,
                        Provider::Codex => palette.codex_label,
                    };
                    let label = provider.as_str().to_string();
                    let label_style = if is_processing {
                        Style::default()
                            .fg(palette.processing_label)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(provider_color)
                            .add_modifier(Modifier::BOLD)
                    };
                    let cleaned_text = cleaned_assistant_text(entry);
                    let raw_text = if cleaned_text.trim().is_empty() {
                        String::new()
                    } else {
                        cleaned_text
                    };
                    let base_style = if is_processing {
                        palette.body_processing_style()
                    } else {
                        palette.body_style()
                    };

                    // Label column: agent name occupies a fixed-width column.
                    // Continuation and wrapped lines are indented to keep content
                    // aligned and prevent text from invading the label column.
                    // Use the longest provider name to keep all labels the same
                    // width so that content columns stay aligned across agents.
                    let max_label_width = Provider::all()
                        .iter()
                        .map(|p| UnicodeWidthStr::width(p.as_str()))
                        .max()
                        .unwrap_or(6);
                    let label_col_width = max_label_width + 2; // "label" + " │"
                    let padded_label = format!("{:width$}", label, width = max_label_width);
                    let label_sep = format!("{} \u{2502}", padded_label);
                    let indent = " ".repeat(label_col_width.saturating_sub(1));
                    let indent_sep = format!("{}\u{2502}", indent); // "       │"
                    let content_width = (width as usize).saturating_sub(label_col_width + 1); // +1 for space after │

                    if raw_text.is_empty() {
                        // No content yet (e.g. still thinking).
                        lines.push(Line::from(vec![
                            Span::styled(label_sep.clone(), label_style),
                        ]));
                    } else {
                        let md_lines = render_markdown(&raw_text, base_style, palette);
                        for (i, md_line) in md_lines.into_iter().enumerate() {
                            // Pre-wrap: split content spans into multiple lines
                            // so each fits within content_width.
                            let wrapped = wrap_spans(md_line, content_width);
                            for (wi, w_line) in wrapped.into_iter().enumerate() {
                                let mut spans = if i == 0 && wi == 0 {
                                    // First line: label column + separator + content
                                    vec![
                                        Span::styled(label_sep.clone(), label_style),
                                        Span::raw(" "),
                                    ]
                                } else {
                                    // Continuation: indent + separator + content
                                    // Use the same label color for the │ so it
                                    // stays visually aligned with the agent.
                                    vec![
                                        Span::styled(indent_sep.clone(), label_style),
                                        Span::raw(" "),
                                    ]
                                };
                                spans.extend(w_line);
                                lines.push(Line::from(spans));
                            }
                        }
                    }
                }
                EntryKind::System => {
                    if let Some(row) = parse_startup_banner_row(&entry.text) {
                        let prev_is_banner = idx > 0
                            && self
                                .entries
                                .get(idx - 1)
                                .is_some_and(is_startup_banner_entry);
                        let next_is_banner = self
                            .entries
                            .get(idx + 1)
                            .is_some_and(is_startup_banner_entry);

                        let border_style = palette.panel_border_style();
                        let outer = banner_card_outer_width(width);
                        if outer >= 6 {
                            let inner = outer.saturating_sub(2);
                            let content_width = inner.saturating_sub(2);
                            if !prev_is_banner {
                                lines.push(Line::from(vec![
                                    Span::styled("┌".to_string(), border_style),
                                    Span::styled("─".repeat(inner), border_style),
                                    Span::styled("┐".to_string(), border_style),
                                ]));
                            }

                            let (raw_text, content_style) = match row {
                                StartupBannerRow::Title(value) => (
                                    format!(" {value}"),
                                    palette.title_style(),
                                ),
                                StartupBannerRow::Agents(value) => {
                                    (format!(" {value}"), palette.secondary_style())
                                }
                                StartupBannerRow::Cwd(value) => {
                                    (format!(" {value}"), palette.secondary_style())
                                }
                                StartupBannerRow::Keys(value) => {
                                    (format!(" {value}"), palette.muted_style())
                                }
                            };
                            let content = fit_to_display_width(&raw_text, content_width);
                            lines.push(Line::from(vec![
                                Span::styled("│ ".to_string(), border_style),
                                Span::styled(content, content_style),
                                Span::styled(" │".to_string(), border_style),
                            ]));

                            if !next_is_banner {
                                lines.push(Line::from(vec![
                                    Span::styled("└".to_string(), border_style),
                                    Span::styled("─".repeat(inner), border_style),
                                    Span::styled("┘".to_string(), border_style),
                                ]));
                                lines.push(Line::from(""));
                            }
                            continue;
                        }

                        let (text, style) = match row {
                            StartupBannerRow::Title(value) => (
                                value.to_string(),
                                palette.title_style(),
                            ),
                            StartupBannerRow::Agents(value) => {
                                (value.to_string(), palette.secondary_style())
                            }
                            StartupBannerRow::Cwd(value) => {
                                (value.to_string(), palette.secondary_style())
                            }
                            StartupBannerRow::Keys(value) => {
                                (value.to_string(), palette.muted_style())
                            }
                        };
                        lines.push(Line::from(vec![Span::styled(text, style)]));
                        if !next_is_banner {
                            lines.push(Line::from(""));
                        }
                        continue;
                    }
                    lines.push(Line::from(vec![Span::styled(
                        format!("[sys] {}", entry.text),
                        palette.secondary_style(),
                    )]));
                }
                EntryKind::Tool => {
                    let is_tool_call = entry.text.contains("calling tool:")
                        || entry.text.contains("tool:")
                        || entry.text.contains("exec:");
                    let is_tool_done =
                        entry.text.contains("finished:") || entry.text.contains("exec done");
                    let (icon, icon_style, text_style) = if is_tool_call {
                        (
                            "  \u{25B6} ",
                            Style::default()
                                .fg(palette.tool_icon)
                                .add_modifier(Modifier::BOLD),
                            Style::default()
                                .fg(palette.activity_text)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if is_tool_done {
                        (
                            "  \u{2714} ",
                            Style::default().fg(palette.processing_label),
                            Style::default().fg(palette.processing_label),
                        )
                    } else {
                        (
                            "  \u{25B8} ",
                            Style::default()
                                .fg(palette.tool_icon)
                                .add_modifier(Modifier::BOLD),
                            Style::default().fg(palette.tool_text),
                        )
                    };
                    lines.push(Line::from(vec![
                        Span::styled(icon, icon_style),
                        Span::styled(entry.text.clone(), text_style),
                    ]));
                }
                EntryKind::Error => {
                    lines.push(Line::from(vec![
                        Span::styled(
                            "[error] ",
                            Style::default()
                                .fg(palette.error_label)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(entry.text.clone(), Style::default().fg(palette.error_text)),
                    ]));
                }
            }
            lines.push(Line::from(""));
        }

        lines
    }

    /// Render all entries except those at the given indices (streaming assistant entries).
    fn render_entries_lines_filtered(&self, width: u16, skip_indices: &std::collections::HashSet<usize>) -> Vec<Line<'static>> {
        let mut lines = Vec::<Line>::new();
        let palette = self.theme.palette();

        for (idx, entry) in self.entries.iter().enumerate() {
            if skip_indices.contains(&idx) {
                continue;
            }
            let entry_provider =
                extract_agent_name(&entry.text).and_then(|n| provider_from_name(&n));
            let is_current_entry =
                self.assistant_idx == Some(idx) || self.agent_entries.values().any(|&i| i == idx);
            let is_processing =
                self.running && matches!(entry.kind, EntryKind::Assistant) && is_current_entry;
            if matches!(entry.kind, EntryKind::Assistant) && idx > 0 {
                lines.push(Line::from(""));
            }
            match entry.kind {
                EntryKind::User => {
                    let parts: Vec<&str> = entry.text.split('\n').collect();
                    let w = width as usize;
                    let user_style = Style::default()
                        .fg(palette.user_fg)
                        .bg(palette.user_bg)
                        .add_modifier(Modifier::BOLD);
                    for part in parts {
                        let content = if part.is_empty() { " " } else { part };
                        let mut text = format!(" {} ", content);
                        if w > 0 {
                            let text_w = UnicodeWidthStr::width(text.as_str());
                            if text_w < w {
                                text.push_str(&" ".repeat(w - text_w));
                            }
                        }
                        lines.push(Line::from(vec![Span::styled(text, user_style)]));
                    }
                }
                EntryKind::Assistant => {
                    let provider = entry_provider.unwrap_or(self.primary_provider);
                    let provider_color = match provider {
                        Provider::Claude => palette.claude_label,
                        Provider::Codex => palette.codex_label,
                    };
                    let label = provider.as_str().to_string();
                    let label_style = if is_processing {
                        Style::default()
                            .fg(palette.processing_label)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(provider_color)
                            .add_modifier(Modifier::BOLD)
                    };
                    let cleaned_text = cleaned_assistant_text(entry);
                    let raw_text = if cleaned_text.trim().is_empty() {
                        String::new()
                    } else {
                        cleaned_text
                    };
                    let base_style = if is_processing {
                        palette.body_processing_style()
                    } else {
                        palette.body_style()
                    };
                    let max_label_width = Provider::all()
                        .iter()
                        .map(|p| UnicodeWidthStr::width(p.as_str()))
                        .max()
                        .unwrap_or(6);
                    let label_col_width = max_label_width + 2;
                    let padded_label = format!("{:width$}", label, width = max_label_width);
                    let label_sep = format!("{} \u{2502}", padded_label);
                    let indent = " ".repeat(label_col_width.saturating_sub(1));
                    let indent_sep = format!("{}\u{2502}", indent);
                    let content_width = (width as usize).saturating_sub(label_col_width + 1);
                    if raw_text.is_empty() {
                        lines.push(Line::from(vec![
                            Span::styled(label_sep.clone(), label_style),
                        ]));
                    } else {
                        let md_lines = render_markdown(&raw_text, base_style, palette);
                        for (i, md_line) in md_lines.into_iter().enumerate() {
                            let wrapped = wrap_spans(md_line, content_width);
                            for (wi, w_line) in wrapped.into_iter().enumerate() {
                                let mut spans = if i == 0 && wi == 0 {
                                    vec![
                                        Span::styled(label_sep.clone(), label_style),
                                        Span::raw(" "),
                                    ]
                                } else {
                                    vec![
                                        Span::styled(indent_sep.clone(), label_style),
                                        Span::raw(" "),
                                    ]
                                };
                                spans.extend(w_line);
                                lines.push(Line::from(spans));
                            }
                        }
                    }
                }
                EntryKind::System => {
                    if let Some(row) = parse_startup_banner_row(&entry.text) {
                        let prev_non_skipped = (0..idx).rev().find(|i| !skip_indices.contains(i));
                        let prev_is_banner = prev_non_skipped
                            .and_then(|i| self.entries.get(i))
                            .is_some_and(is_startup_banner_entry);
                        let next_non_skipped = (idx + 1..self.entries.len()).find(|i| !skip_indices.contains(i));
                        let next_is_banner = next_non_skipped
                            .and_then(|i| self.entries.get(i))
                            .is_some_and(is_startup_banner_entry);

                        let border_style = palette.panel_border_style();
                        let outer = banner_card_outer_width(width);
                        if outer >= 6 {
                            let inner = outer.saturating_sub(2);
                            let content_width = inner.saturating_sub(2);
                            if !prev_is_banner {
                                lines.push(Line::from(vec![
                                    Span::styled("┌".to_string(), border_style),
                                    Span::styled("─".repeat(inner), border_style),
                                    Span::styled("┐".to_string(), border_style),
                                ]));
                            }
                            let (raw_text, content_style) = match row {
                                StartupBannerRow::Title(value) => (
                                    format!(" {value}"),
                                    palette.title_style(),
                                ),
                                StartupBannerRow::Agents(value) => {
                                    (format!(" {value}"), palette.secondary_style())
                                }
                                StartupBannerRow::Cwd(value) => {
                                    (format!(" {value}"), palette.secondary_style())
                                }
                                StartupBannerRow::Keys(value) => {
                                    (format!(" {value}"), palette.muted_style())
                                }
                            };
                            let content = fit_to_display_width(&raw_text, content_width);
                            lines.push(Line::from(vec![
                                Span::styled("│ ".to_string(), border_style),
                                Span::styled(content, content_style),
                                Span::styled(" │".to_string(), border_style),
                            ]));
                            if !next_is_banner {
                                lines.push(Line::from(vec![
                                    Span::styled("└".to_string(), border_style),
                                    Span::styled("─".repeat(inner), border_style),
                                    Span::styled("┘".to_string(), border_style),
                                ]));
                                lines.push(Line::from(""));
                            }
                            continue;
                        }
                        let (text, style) = match row {
                            StartupBannerRow::Title(value) => (
                                value.to_string(),
                                palette.title_style(),
                            ),
                            StartupBannerRow::Agents(value) => {
                                (value.to_string(), palette.secondary_style())
                            }
                            StartupBannerRow::Cwd(value) => {
                                (value.to_string(), palette.secondary_style())
                            }
                            StartupBannerRow::Keys(value) => {
                                (value.to_string(), palette.muted_style())
                            }
                        };
                        lines.push(Line::from(vec![Span::styled(text, style)]));
                        if !next_is_banner {
                            lines.push(Line::from(""));
                        }
                        continue;
                    }
                    lines.push(Line::from(vec![Span::styled(
                        format!("[sys] {}", entry.text),
                        palette.secondary_style(),
                    )]));
                }
                EntryKind::Tool => {
                    let is_tool_call = entry.text.contains("calling tool:")
                        || entry.text.contains("tool:")
                        || entry.text.contains("exec:");
                    let is_tool_done =
                        entry.text.contains("finished:") || entry.text.contains("exec done");
                    let (icon, icon_style, text_style) = if is_tool_call {
                        (
                            "  \u{25B6} ",
                            Style::default()
                                .fg(palette.tool_icon)
                                .add_modifier(Modifier::BOLD),
                            Style::default()
                                .fg(palette.activity_text)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if is_tool_done {
                        (
                            "  \u{2714} ",
                            Style::default().fg(palette.processing_label),
                            Style::default().fg(palette.processing_label),
                        )
                    } else {
                        (
                            "  \u{25B8} ",
                            Style::default()
                                .fg(palette.tool_icon)
                                .add_modifier(Modifier::BOLD),
                            Style::default().fg(palette.tool_text),
                        )
                    };
                    lines.push(Line::from(vec![
                        Span::styled(icon, icon_style),
                        Span::styled(entry.text.clone(), text_style),
                    ]));
                }
                EntryKind::Error => {
                    lines.push(Line::from(vec![
                        Span::styled(
                            "[error] ",
                            Style::default()
                                .fg(palette.error_label)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(entry.text.clone(), Style::default().fg(palette.error_text)),
                    ]));
                }
            }
            lines.push(Line::from(""));
        }

        lines
    }

    fn render_log_lines_inner(&self, width: u16) -> Vec<Line<'static>> {
        self.render_entries_lines(width)
    }
}

/// Pre-wrap a list of spans so that each resulting line fits within `max_width`
/// display columns. Returns a Vec of span-lines; if no wrapping is needed,
/// returns a single-element vec with the original spans.
fn wrap_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Vec<Span<'static>>> {
    if max_width == 0 {
        return vec![spans];
    }
    let mut result: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut current_width: usize = 0;

    for span in spans {
        let span_width = UnicodeWidthStr::width(span.content.as_ref());
        if current_width + span_width <= max_width {
            current_width += span_width;
            current_line.push(span);
        } else {
            // Need to split this span across lines.
            let style = span.style;
            let text = span.content.into_owned();
            let mut remaining = text.as_str();
            while !remaining.is_empty() {
                let avail = max_width.saturating_sub(current_width);
                if avail == 0 {
                    if !current_line.is_empty() {
                        result.push(std::mem::take(&mut current_line));
                    }
                    current_width = 0;
                    continue;
                }
                // Find the split point: fit as many chars as possible within avail columns.
                let mut split_byte = 0;
                let mut cols = 0usize;
                for (byte_idx, ch) in remaining.char_indices() {
                    let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if cols + w > avail {
                        break;
                    }
                    cols += w;
                    split_byte = byte_idx + ch.len_utf8();
                }
                if split_byte == 0 && current_line.is_empty() {
                    // Single char wider than avail (shouldn't happen normally).
                    // Force at least one char to avoid infinite loop.
                    let ch = remaining.chars().next().unwrap();
                    split_byte = ch.len_utf8();
                    cols = UnicodeWidthChar::width(ch).unwrap_or(1);
                }
                if split_byte == 0 {
                    // No room on current line; start a new one.
                    result.push(std::mem::take(&mut current_line));
                    current_width = 0;
                    continue;
                }
                let chunk = &remaining[..split_byte];
                current_line.push(Span::styled(chunk.to_string(), style));
                current_width += cols;
                remaining = &remaining[split_byte..];
                if !remaining.is_empty() {
                    result.push(std::mem::take(&mut current_line));
                    current_width = 0;
                }
            }
        }
    }
    if !current_line.is_empty() {
        result.push(current_line);
    }
    if result.is_empty() {
        result.push(Vec::new());
    }
    result
}

/// Render markdown text into styled spans per line.
/// Supports: headings (#), bold (**), italic (*), inline code (`),
/// fenced code blocks (```), and unordered list bullets (- / *).
fn render_markdown(
    text: &str,
    base_style: Style,
    palette: ThemePalette,
) -> Vec<Vec<Span<'static>>> {
    if !contains_markdown_syntax(text) {
        return text
            .split('\n')
            .map(|line| {
                let content = if line.is_empty() { " " } else { line };
                vec![Span::styled(content.to_string(), base_style)]
            })
            .collect();
    }

    let mut result: Vec<Vec<Span<'static>>> = Vec::new();
    let mut in_code_block = false;

    let code_block_style = Style::default().fg(palette.code_fg).bg(palette.code_bg);
    let heading_style = Style::default()
        .fg(palette.banner_title)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let bold_style = base_style.add_modifier(Modifier::BOLD);
    let italic_style = base_style.add_modifier(Modifier::ITALIC);
    let inline_code_style = Style::default()
        .fg(palette.inline_code_fg)
        .bg(palette.inline_code_bg);
    let bullet_style = base_style.fg(palette.bullet);

    for line in text.split('\n') {
        let trimmed = line.trim();

        // Toggle fenced code blocks
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            // Render the fence line itself in code style
            let lang = trimmed.trim_start_matches('`').trim();
            if lang.is_empty() {
                result.push(vec![Span::styled(
                    "───".to_string(),
                    palette.muted_style(),
                )]);
            } else {
                result.push(vec![
                    Span::styled("─── ".to_string(), palette.muted_style()),
                    Span::styled(
                        lang.to_string(),
                        palette.muted_style().add_modifier(Modifier::ITALIC),
                    ),
                ]);
            }
            continue;
        }

        if in_code_block {
            let content = if line.is_empty() { " " } else { line };
            result.push(vec![Span::styled(content.to_string(), code_block_style)]);
            continue;
        }

        // Headings
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            let heading_text = trimmed[level..].trim_start();
            let prefix = "#".repeat(level);
            if heading_text.is_empty() {
                result.push(vec![Span::styled(prefix, heading_style)]);
            } else {
                result.push(vec![
                    Span::styled(
                        format!("{} ", prefix),
                        palette.muted_style(),
                    ),
                    Span::styled(heading_text.to_string(), heading_style),
                ]);
            }
            continue;
        }

        // Unordered list bullets
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let indent = line.len() - line.trim_start().len();
            let rest = &trimmed[2..];
            let mut spans = Vec::new();
            if indent > 0 {
                spans.push(Span::raw(" ".repeat(indent)));
            }
            spans.push(Span::styled("\u{2022} ".to_string(), bullet_style));
            spans.extend(render_inline_markdown(
                rest,
                base_style,
                bold_style,
                italic_style,
                inline_code_style,
            ));
            result.push(spans);
            continue;
        }

        // Regular line with inline markdown
        let content = if line.is_empty() { " " } else { line };
        result.push(render_inline_markdown(
            content,
            base_style,
            bold_style,
            italic_style,
            inline_code_style,
        ));
    }

    result
}

fn contains_markdown_syntax(text: &str) -> bool {
    if text.contains("```") || text.contains('`') || text.contains("**") {
        return true;
    }
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            return true;
        }
        // Lightweight check for inline emphasis.
        if line.contains('*') {
            return true;
        }
    }
    false
}

fn sanitize_runtime_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_escape = false;
    let mut in_csi = false;

    for ch in text.chars() {
        if in_escape {
            if in_csi {
                // CSI sequence terminates at bytes in range 0x40..0x7E.
                if ('@'..='~').contains(&ch) {
                    in_escape = false;
                    in_csi = false;
                }
                continue;
            }
            if ch == '[' {
                in_csi = true;
                continue;
            }
            in_escape = false;
            continue;
        }

        if ch == '\u{1b}' {
            in_escape = true;
            continue;
        }

        if ch == '\r' {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }

        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }

        out.push(ch);
    }

    out
}

/// Parse inline markdown: **bold**, *italic*, `code`
fn render_inline_markdown(
    text: &str,
    base_style: Style,
    bold_style: Style,
    italic_style: Style,
    code_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Inline code: `...`
        if chars[i] == '`' {
            if !buf.is_empty() {
                spans.push(Span::styled(buf.clone(), base_style));
                buf.clear();
            }
            let start = i + 1;
            if let Some(end) = chars[start..].iter().position(|&c| c == '`') {
                let code_text: String = chars[start..start + end].iter().collect();
                spans.push(Span::styled(format!(" {} ", code_text), code_style));
                i = start + end + 1;
            } else {
                buf.push('`');
                i += 1;
            }
            continue;
        }

        // Bold: **...**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if !buf.is_empty() {
                spans.push(Span::styled(buf.clone(), base_style));
                buf.clear();
            }
            let start = i + 2;
            let mut end = None;
            for j in start..len.saturating_sub(1) {
                if chars[j] == '*' && chars[j + 1] == '*' {
                    end = Some(j);
                    break;
                }
            }
            if let Some(end) = end {
                let bold_text: String = chars[start..end].iter().collect();
                spans.push(Span::styled(bold_text, bold_style));
                i = end + 2;
            } else {
                buf.push('*');
                buf.push('*');
                i += 2;
            }
            continue;
        }

        // Italic: *...*
        if chars[i] == '*' {
            if !buf.is_empty() {
                spans.push(Span::styled(buf.clone(), base_style));
                buf.clear();
            }
            let start = i + 1;
            let mut end = None;
            for j in start..len {
                if chars[j] == '*' && !(j + 1 < len && chars[j + 1] == '*') {
                    end = Some(j);
                    break;
                }
            }
            if let Some(end) = end {
                let italic_text: String = chars[start..end].iter().collect();
                spans.push(Span::styled(italic_text, italic_style));
                i = end + 1;
            } else {
                buf.push('*');
                i += 1;
            }
            continue;
        }

        buf.push(chars[i]);
        i += 1;
    }

    if !buf.is_empty() {
        spans.push(Span::styled(buf, base_style));
    }

    if spans.is_empty() {
        spans.push(Span::styled(" ".to_string(), base_style));
    }

    spans
}

fn parse_dispatch_override(
    line: &str,
) -> std::result::Result<Option<(DispatchTarget, String)>, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() {
        return Ok(None);
    }

    let mentions: Vec<&str> = tokens
        .iter()
        .copied()
        .filter(|token| token.starts_with("@"))
        .collect();
    if mentions.is_empty() {
        return Ok(None);
    }

    let prompt = tokens
        .iter()
        .copied()
        .filter(|token| !token.starts_with("@"))
        .collect::<Vec<_>>()
        .join(" ");
    if prompt.trim().is_empty() {
        return Err(format!("usage: {} <task>", mentions.join(" ")));
    }

    if mentions.iter().any(|m| *m == "@all") {
        return Ok(Some((DispatchTarget::All, prompt)));
    }

    let mut providers = Vec::new();
    for mention in mentions {
        let name = mention.trim_start_matches("@");
        let Some(provider) = provider_from_name(name) else {
            return Err(format!(
                "unknown dispatch target {}; use @claude, @codex or @all",
                mention
            ));
        };
        if !providers.contains(&provider) {
            providers.push(provider);
        }
    }

    let target = if providers.len() == 1 {
        DispatchTarget::Provider(providers[0])
    } else {
        DispatchTarget::Providers(providers)
    };

    if matches!(target, DispatchTarget::Providers(ref ps) if ps.is_empty()) {
        return Err("usage: @claude <task> | @codex <task> | @all <task>".to_string());
    }

    Ok(Some((target, prompt)))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    fn app_with_entries(n: usize) -> App {
        let mut app = App::new();
        for i in 0..n {
            app.push_entry(EntryKind::System, format!("entry {i}"));
        }
        app
    }

    #[test]
    fn pageup_disables_autoscroll_and_moves_up() {
        let mut app = app_with_entries(40);
        let before = app.scroll;
        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));

        assert!(!app.autoscroll);
        assert_eq!(app.scroll, before.saturating_sub(5));
    }

    #[test]
    fn pagedown_near_bottom_reenables_autoscroll() {
        let mut app = app_with_entries(40);
        let max = app.scroll_max();
        app.autoscroll = false;
        app.scroll = max.saturating_sub(1);

        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));

        assert_eq!(app.scroll, max);
        assert!(app.autoscroll);
    }

    #[test]
    fn new_entries_do_not_force_scroll_when_autoscroll_off() {
        let mut app = app_with_entries(20);
        app.autoscroll = false;
        app.scroll = 3;
        app.push_entry(EntryKind::System, "extra");

        assert_eq!(app.scroll, 3);
    }

    #[test]
    fn new_entries_follow_bottom_when_autoscroll_on() {
        let mut app = app_with_entries(20);
        app.autoscroll = true;
        app.push_entry(EntryKind::System, "extra");

        let max = app.scroll_max();
        assert_eq!(app.scroll, max);
    }

    #[test]
    fn tool_events_follow_bottom_when_autoscroll_on() {
        let mut app = app_with_entries(30);
        app.autoscroll = true;
        app.scroll = app.scroll_max();
        let before_max = app.scroll_max();

        let (tx, rx) = unbounded::<WorkerEvent>();
        app.rx = Some(rx);
        tx.send(WorkerEvent::Tool("invoke bash ls -la".to_string()))
            .expect("send tool event");

        assert!(app.poll_worker());
        let after_max = app.scroll_max();
        assert!(after_max >= before_max);
        assert_eq!(app.scroll, after_max);
    }

    #[test]
    fn agent_done_sets_elapsed_only_for_active_entry() {
        let mut app = App::new();
        app.push_entry(EntryKind::Assistant, "[claude]\nold answer");
        app.entries[0].elapsed_secs = Some(7);
        app.push_entry(EntryKind::Assistant, "[claude]\ncurrent answer");
        app.agent_entries.insert(Provider::Claude, 1);
        app.run_started_at = Some(Instant::now());

        let (tx, rx) = unbounded::<WorkerEvent>();
        app.rx = Some(rx);
        tx.send(WorkerEvent::AgentDone(Provider::Claude))
            .expect("send agent done event");

        assert!(app.poll_worker());
        assert_eq!(app.entries[0].elapsed_secs, Some(7));
        assert!(app.entries[1].elapsed_secs.is_some());
    }

    #[test]
    fn assistant_header_text_stable_across_running_done_transition() {
        let mut app = App::new();
        app.entries.clear();
        app.push_entry(EntryKind::Assistant, "[codex]\nanswer");
        app.entries[0].elapsed_secs = Some(12);
        app.agent_entries.insert(Provider::Codex, 0);
        app.running = true;

        let running_header = flatten_line_to_plain(&app.render_entries_lines(80)[0]);

        app.running = false;
        app.agent_entries.clear();
        let done_header = flatten_line_to_plain(&app.render_entries_lines(80)[0]);

        assert_eq!(running_header, done_header);
        assert_eq!(done_header, "codex  \u{2502} answer");
    }

    #[test]
    fn wrapped_stream_growth_keeps_tail_when_autoscroll_on() {
        let mut app = App::new();
        app.update_viewport(24, 12);
        app.autoscroll = true;
        app.push_entry(EntryKind::Assistant, "[claude]\nshort line");

        let before = app.scroll_max();
        let idx = app.entries.len().saturating_sub(1);
        if let Some(entry) = app.entries.get_mut(idx) {
            entry
                .text
                .push_str(" this is a long streaming sentence that should wrap across rows");
        }
        app.follow_scroll();

        let after = app.scroll_max();
        assert!(after >= before);
        assert_eq!(app.scroll, after);
    }

    #[test]
    fn startup_banner_renders_as_card() {
        let mut app = App::new();
        app.entries.clear();
        app.maybe_show_startup_banner();

        let rendered = app
            .render_entries_lines(80)
            .into_iter()
            .map(|line| flatten_line_to_plain(&line))
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.starts_with('┌')));
        assert!(rendered.iter().any(|line| line.starts_with("│ ")));
        assert!(rendered.iter().any(|line| line.starts_with('└')));
        assert!(!rendered.iter().any(|line| line.contains("[sys]")));
    }

    #[test]
    fn parse_dispatch_override_provider() {
        let parsed = parse_dispatch_override("@claude fix this")
            .expect("parse should succeed")
            .expect("dispatch override should exist");

        assert_eq!(parsed.0, DispatchTarget::Provider(Provider::Claude));
        assert_eq!(parsed.1, "fix this");
    }

    #[test]
    fn parse_dispatch_override_unknown_target_errors() {
        let err = parse_dispatch_override("@foo do work").expect_err("should error");
        assert!(err.contains("unknown dispatch target"));
    }

    #[test]
    fn parse_dispatch_override_multiple_agents() {
        let parsed = parse_dispatch_override("@claude @codex investigate")
            .expect("parse should succeed")
            .expect("dispatch override should exist");
        assert_eq!(
            parsed.0,
            DispatchTarget::Providers(vec![Provider::Claude, Provider::Codex])
        );
        assert_eq!(parsed.1, "investigate");
    }

    #[test]
    fn parse_dispatch_override_all_wins() {
        let parsed = parse_dispatch_override("@claude @all investigate")
            .expect("parse should succeed")
            .expect("dispatch override should exist");
        assert_eq!(parsed.0, DispatchTarget::All);
        assert_eq!(parsed.1, "investigate");
    }

    #[test]
    fn parse_dispatch_override_mid_sentence_single_agent() {
        let parsed = parse_dispatch_override("please @codex investigate this bug")
            .expect("parse should succeed")
            .expect("dispatch override should exist");
        assert_eq!(parsed.0, DispatchTarget::Provider(Provider::Codex));
        assert_eq!(parsed.1, "please investigate this bug");
    }

    #[test]
    fn parse_dispatch_override_without_mentions_returns_none() {
        let parsed =
            parse_dispatch_override("please investigate this bug").expect("parse should succeed");
        assert!(parsed.is_none());
    }

    #[test]
    fn compute_append_ranges_ignores_style_only_refresh() {
        let old = vec![
            "user".to_string(),
            "assistant".to_string(),
            "tool a".to_string(),
            "tool b".to_string(),
        ];
        let new = vec![
            "user".to_string(),
            "assistant".to_string(),
            "tool a".to_string(),
            "tool b".to_string(),
        ];
        assert_eq!(
            compute_append_ranges(&old, &new),
            Vec::<(usize, usize)>::new()
        );
    }

    #[test]
    fn compute_append_ranges_rejects_in_place_changes() {
        // When existing lines changed (not just appended), no safe append is possible.
        let old = vec![
            "user".to_string(),
            "assistant (thinking...)".to_string(),
            "tool start".to_string(),
            "tool progress".to_string(),
        ];
        let new = vec![
            "user".to_string(),
            "assistant final answer".to_string(),
            "tool start".to_string(),
            "tool progress".to_string(),
            "tool done".to_string(),
        ];
        assert_eq!(
            compute_append_ranges(&old, &new),
            Vec::<(usize, usize)>::new()
        );
    }

    #[test]
    fn compute_append_ranges_appends_tail_only() {
        // When old is a strict prefix of new, append the tail.
        let old = vec![
            "user".to_string(),
            "assistant".to_string(),
        ];
        let new = vec![
            "user".to_string(),
            "assistant".to_string(),
            "tool start".to_string(),
            "tool done".to_string(),
        ];
        assert_eq!(compute_append_ranges(&old, &new), vec![(2, 4)]);
    }

    #[test]
    fn large_paste_is_collapsed_and_restored_before_dispatch() {
        let mut app = App::new();
        let payload = "line ".repeat(220);
        app.handle_paste_event(&payload);

        assert!(app.input.starts_with("[Pasted Content "));
        assert_eq!(app.pending_pastes.len(), 1);

        let expanded = app.consume_pending_pastes(&app.input.clone());
        assert_eq!(expanded, payload);
        assert!(app.pending_pastes.is_empty());
    }

    #[test]
    fn short_paste_keeps_plain_text() {
        let mut app = App::new();
        app.handle_paste_event("hello\nworld");
        assert_eq!(app.input, "hello\nworld");
        assert!(app.pending_pastes.is_empty());
    }

    #[test]
    fn mem_command_reports_backend_unavailable_in_tests() {
        let mut app = App::new();
        app.input = "/mem show".to_string();
        app.cursor = app.input.len();

        app.submit_current_line(false);

        let last = app.entries.last().expect("expected memory error entry");
        assert!(matches!(last.kind, EntryKind::Error));
        assert!(last.text.contains("memory backend unavailable"));
    }

    #[test]
    fn running_submit_hint_is_not_duplicated() {
        let mut app = App::new();
        app.running = true;
        app.input = "hello".to_string();
        app.cursor = app.input.len();

        app.submit_current_line(false);
        app.submit_current_line(false);

        let wait_count = app
            .entries
            .iter()
            .filter(|entry| {
                matches!(entry.kind, EntryKind::System) && entry.text == "task is running, wait..."
            })
            .count();
        assert_eq!(wait_count, 1);
    }

    #[test]
    fn transcript_restore_defaults_to_hidden_when_memory_is_available() {
        assert!(!restore_transcript_on_start(true));
        assert!(restore_transcript_on_start(false));
    }
}
