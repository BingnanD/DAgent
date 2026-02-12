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
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

use crate::{
    cleaned_assistant_text, default_commands, detect_available_providers, execute_line,
    extract_agent_name, high_risk_check, input_cursor_position, kill_pid, memory::MemoryStore,
    ordered_providers, provider_from_name, providers_label, truncate, DispatchTarget, SPINNER,
    THINKING_PLACEHOLDER,
};

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
                prompt: Color::Rgb(136, 192, 208),
                input_bg: Color::Rgb(36, 40, 52),
                input_text: Color::Rgb(229, 233, 240),
                muted_text: Color::Rgb(76, 86, 106),
                highlight_fg: Color::Rgb(236, 239, 244),
                highlight_bg: Color::Rgb(62, 68, 90),
                activity_badge_fg: Color::Rgb(236, 239, 244),
                activity_badge_bg: Color::Rgb(52, 101, 164),
                activity_text: Color::Rgb(143, 214, 227),
                status_text: Color::Rgb(76, 86, 106),
                user_fg: Color::Rgb(236, 239, 244),
                user_bg: Color::Rgb(36, 40, 52),
                claude_label: Color::Rgb(147, 130, 220),
                codex_label: Color::Rgb(86, 182, 194),
                processing_label: Color::Rgb(136, 192, 208),
                assistant_text: Color::Rgb(229, 233, 240),
                assistant_processing_text: Color::Rgb(192, 202, 214),
                system_text: Color::Rgb(76, 86, 106),
                tool_icon: Color::Rgb(94, 129, 172),
                tool_text: Color::Rgb(129, 140, 160),
                error_label: Color::Rgb(191, 97, 106),
                error_text: Color::Rgb(208, 135, 112),
                banner_title: Color::Rgb(136, 192, 208),
                panel_bg: Color::Rgb(30, 34, 42),
                panel_fg: Color::Rgb(229, 233, 240),
                approval_title: Color::Rgb(235, 203, 139),
                code_fg: Color::Rgb(200, 210, 220),
                code_bg: Color::Rgb(25, 28, 36),
                inline_code_fg: Color::Rgb(208, 190, 150),
                inline_code_bg: Color::Rgb(30, 34, 42),
                bullet: Color::Rgb(94, 129, 172),
            },
            ThemePreset::Graphite => ThemePalette {
                prompt: Color::Rgb(122, 214, 197),
                input_bg: Color::Rgb(31, 35, 40),
                input_text: Color::Rgb(229, 232, 235),
                muted_text: Color::Rgb(117, 126, 138),
                highlight_fg: Color::Rgb(242, 245, 248),
                highlight_bg: Color::Rgb(53, 61, 70),
                activity_badge_fg: Color::Rgb(242, 245, 248),
                activity_badge_bg: Color::Rgb(44, 114, 122),
                activity_text: Color::Rgb(153, 223, 209),
                status_text: Color::Rgb(117, 126, 138),
                user_fg: Color::Rgb(239, 242, 245),
                user_bg: Color::Rgb(36, 42, 48),
                claude_label: Color::Rgb(194, 160, 245),
                codex_label: Color::Rgb(92, 198, 208),
                processing_label: Color::Rgb(122, 214, 197),
                assistant_text: Color::Rgb(229, 232, 235),
                assistant_processing_text: Color::Rgb(190, 198, 206),
                system_text: Color::Rgb(117, 126, 138),
                tool_icon: Color::Rgb(96, 164, 182),
                tool_text: Color::Rgb(154, 165, 176),
                error_label: Color::Rgb(216, 124, 138),
                error_text: Color::Rgb(218, 157, 127),
                banner_title: Color::Rgb(122, 214, 197),
                panel_bg: Color::Rgb(26, 30, 35),
                panel_fg: Color::Rgb(229, 232, 235),
                approval_title: Color::Rgb(234, 202, 136),
                code_fg: Color::Rgb(205, 216, 225),
                code_bg: Color::Rgb(23, 26, 31),
                inline_code_fg: Color::Rgb(225, 199, 149),
                inline_code_bg: Color::Rgb(33, 37, 43),
                bullet: Color::Rgb(96, 164, 182),
            },
            ThemePreset::Solarized => ThemePalette {
                prompt: Color::Rgb(42, 161, 152),
                input_bg: Color::Rgb(7, 54, 66),
                input_text: Color::Rgb(238, 232, 213),
                muted_text: Color::Rgb(88, 110, 117),
                highlight_fg: Color::Rgb(253, 246, 227),
                highlight_bg: Color::Rgb(0, 95, 108),
                activity_badge_fg: Color::Rgb(253, 246, 227),
                activity_badge_bg: Color::Rgb(38, 139, 210),
                activity_text: Color::Rgb(88, 224, 206),
                status_text: Color::Rgb(88, 110, 117),
                user_fg: Color::Rgb(253, 246, 227),
                user_bg: Color::Rgb(7, 54, 66),
                claude_label: Color::Rgb(181, 137, 0),
                codex_label: Color::Rgb(42, 161, 152),
                processing_label: Color::Rgb(88, 224, 206),
                assistant_text: Color::Rgb(238, 232, 213),
                assistant_processing_text: Color::Rgb(188, 190, 172),
                system_text: Color::Rgb(88, 110, 117),
                tool_icon: Color::Rgb(38, 139, 210),
                tool_text: Color::Rgb(131, 148, 150),
                error_label: Color::Rgb(220, 50, 47),
                error_text: Color::Rgb(203, 75, 22),
                banner_title: Color::Rgb(42, 161, 152),
                panel_bg: Color::Rgb(0, 43, 54),
                panel_fg: Color::Rgb(238, 232, 213),
                approval_title: Color::Rgb(181, 137, 0),
                code_fg: Color::Rgb(238, 232, 213),
                code_bg: Color::Rgb(0, 43, 54),
                inline_code_fg: Color::Rgb(211, 54, 130),
                inline_code_bg: Color::Rgb(7, 54, 66),
                bullet: Color::Rgb(38, 139, 210),
            },
            ThemePreset::Aurora => ThemePalette {
                prompt: Color::Rgb(141, 211, 199),
                input_bg: Color::Rgb(22, 29, 33),
                input_text: Color::Rgb(225, 234, 231),
                muted_text: Color::Rgb(102, 121, 118),
                highlight_fg: Color::Rgb(244, 250, 247),
                highlight_bg: Color::Rgb(40, 58, 55),
                activity_badge_fg: Color::Rgb(244, 250, 247),
                activity_badge_bg: Color::Rgb(37, 122, 108),
                activity_text: Color::Rgb(165, 233, 217),
                status_text: Color::Rgb(102, 121, 118),
                user_fg: Color::Rgb(242, 248, 245),
                user_bg: Color::Rgb(27, 36, 39),
                claude_label: Color::Rgb(190, 154, 233),
                codex_label: Color::Rgb(99, 213, 180),
                processing_label: Color::Rgb(141, 211, 199),
                assistant_text: Color::Rgb(225, 234, 231),
                assistant_processing_text: Color::Rgb(188, 204, 200),
                system_text: Color::Rgb(102, 121, 118),
                tool_icon: Color::Rgb(90, 170, 150),
                tool_text: Color::Rgb(144, 160, 158),
                error_label: Color::Rgb(220, 118, 118),
                error_text: Color::Rgb(226, 160, 127),
                banner_title: Color::Rgb(141, 211, 199),
                panel_bg: Color::Rgb(18, 24, 27),
                panel_fg: Color::Rgb(225, 234, 231),
                approval_title: Color::Rgb(232, 195, 120),
                code_fg: Color::Rgb(212, 224, 220),
                code_bg: Color::Rgb(15, 20, 23),
                inline_code_fg: Color::Rgb(229, 201, 152),
                inline_code_bg: Color::Rgb(25, 33, 37),
                bullet: Color::Rgb(90, 170, 150),
            },
            ThemePreset::Ember => ThemePalette {
                prompt: Color::Rgb(237, 165, 120),
                input_bg: Color::Rgb(38, 32, 29),
                input_text: Color::Rgb(242, 232, 220),
                muted_text: Color::Rgb(140, 120, 106),
                highlight_fg: Color::Rgb(255, 244, 232),
                highlight_bg: Color::Rgb(78, 60, 50),
                activity_badge_fg: Color::Rgb(255, 244, 232),
                activity_badge_bg: Color::Rgb(176, 93, 58),
                activity_text: Color::Rgb(245, 190, 152),
                status_text: Color::Rgb(140, 120, 106),
                user_fg: Color::Rgb(255, 244, 232),
                user_bg: Color::Rgb(47, 38, 34),
                claude_label: Color::Rgb(196, 153, 235),
                codex_label: Color::Rgb(110, 185, 193),
                processing_label: Color::Rgb(237, 165, 120),
                assistant_text: Color::Rgb(242, 232, 220),
                assistant_processing_text: Color::Rgb(208, 191, 177),
                system_text: Color::Rgb(140, 120, 106),
                tool_icon: Color::Rgb(194, 127, 95),
                tool_text: Color::Rgb(176, 152, 134),
                error_label: Color::Rgb(227, 108, 95),
                error_text: Color::Rgb(226, 150, 118),
                banner_title: Color::Rgb(237, 165, 120),
                panel_bg: Color::Rgb(30, 24, 21),
                panel_fg: Color::Rgb(242, 232, 220),
                approval_title: Color::Rgb(232, 187, 117),
                code_fg: Color::Rgb(230, 214, 198),
                code_bg: Color::Rgb(24, 19, 17),
                inline_code_fg: Color::Rgb(245, 203, 145),
                inline_code_bg: Color::Rgb(40, 32, 29),
                bullet: Color::Rgb(194, 127, 95),
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
    pub(crate) activity_badge_fg: Color,
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
}

#[derive(Clone, Debug)]
struct PendingApproval {
    line: String,
    tool: String,
    reason: String,
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Normal,
    CommandPalette,
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
    PromotePrimary { to: Provider, reason: String },
    Error(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionSnapshot {
    primary_provider: Provider,
    show_tool_events: bool,
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
    const SPINNER_TICK_MS: u64 = 220;
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
        if app.running && last_spinner_tick.elapsed() >= Duration::from_millis(SPINNER_TICK_MS) {
            app.spinner_idx = (app.spinner_idx + 1) % SPINNER.len();
            last_spinner_tick = Instant::now();
            state_changed = true;
        }
        if state_changed {
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
                    app.insert_str(&text);
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
    let lines = app.cached_log_lines();
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
            // Fallback: skip append for this batch instead of crashing the whole app.
            *flushed_log_lines = lines.to_vec();
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
    let mut common_prefix = 0usize;
    while common_prefix < flushed_plain.len()
        && common_prefix < current_plain.len()
        && flushed_plain[common_prefix] == current_plain[common_prefix]
    {
        common_prefix += 1;
    }

    if common_prefix == current_plain.len() && common_prefix == flushed_plain.len() {
        return Vec::new();
    }

    // Try to preserve the largest unchanged tail from already-flushed text, even if
    // new lines were appended after it. This prevents replaying old tool/progress lines
    // when an earlier assistant line is edited and a completion line is appended.
    let mut best_tail_len = 0usize;
    let mut best_tail_pos = 0usize;
    let max_tail = flushed_plain.len().saturating_sub(common_prefix);
    'outer: for tail_len in (1..=max_tail).rev() {
        let old_tail_start = flushed_plain.len() - tail_len;
        for new_pos in common_prefix..=current_plain.len().saturating_sub(tail_len) {
            if current_plain[new_pos..new_pos + tail_len] == flushed_plain[old_tail_start..] {
                best_tail_len = tail_len;
                best_tail_pos = new_pos;
                break 'outer;
            }
        }
    }

    let mut ranges = Vec::new();
    if best_tail_len > 0 {
        if common_prefix < best_tail_pos {
            ranges.push((common_prefix, best_tail_pos));
        }
        let tail_end = best_tail_pos + best_tail_len;
        if tail_end < current_plain.len() {
            ranges.push((tail_end, current_plain.len()));
        }
    } else if common_prefix < current_plain.len() {
        ranges.push((common_prefix, current_plain.len()));
    }
    ranges
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
    palette_query: String,
    palette_idx: usize,
    slash_hint_idx: usize,

    approval: Option<PendingApproval>,
    allow_high_risk_tools: HashSet<String>,
    show_tool_events: bool,
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

    last_status: String,
    session_id: String,
    memory: Option<MemoryStore>,
    child_pids: Arc<Mutex<Vec<u32>>>,

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
            palette_query: String::new(),
            palette_idx: 0,
            slash_hint_idx: 0,
            approval: None,
            allow_high_risk_tools: HashSet::new(),
            show_tool_events: true,
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
            last_status: "ready".to_string(),
            session_id: default_session_id(),
            memory,
            child_pids: Arc::new(Mutex::new(Vec::new())),
            render_generation: 0,
            render_cache: RenderCache::new(),
        };
        app.restore_session();
        app
    }

    /// Bump the render generation to invalidate the render cache.
    fn invalidate_render_cache(&mut self) {
        self.render_generation = self.render_generation.wrapping_add(1);
    }

    fn start_running_state(&mut self, target: String) {
        self.running = true;
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
        self.active_provider = None;
        self.run_started_at = None;
        self.run_target.clear();
        if let Ok(mut pids) = self.child_pids.lock() {
            pids.clear();
        }
    }

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
        });
        self.follow_scroll();
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
        end_y.saturating_add(1).clamp(1, 6)
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
        self.show_tool_events = snapshot.show_tool_events;
        self.theme = snapshot.theme;
        self.entries = snapshot.entries;
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
            show_tool_events: self.show_tool_events,
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
                        let event_msg = format!("agent {} started", provider.as_str());
                        if self.show_tool_events {
                            self.push_entry(EntryKind::Tool, event_msg.clone());
                        }
                        self.last_tool_event = event_msg;
                        self.last_status = format!("{} working", provider.as_str());
                    }
                    Ok(WorkerEvent::AgentChunk { provider, chunk }) => {
                        processed_any = true;
                        let chunk = sanitize_runtime_text(&chunk);
                        if chunk.trim().is_empty() {
                            continue;
                        }
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
                        if let Some(i) = self.agent_entries.get(&provider).copied() {
                            let had_chunk = self
                                .agent_had_chunk
                                .get(&provider)
                                .copied()
                                .unwrap_or(false);
                            if let Some(entry) = self.entries.get_mut(i) {
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
                        let elapsed_secs = self.running_elapsed_secs();
                        let event_msg = format!(
                            "agent {} completed ({:02}:{:02})",
                            provider.as_str(),
                            elapsed_secs / 60,
                            elapsed_secs % 60
                        );
                        if self.show_tool_events {
                            self.push_entry(EntryKind::Tool, event_msg.clone());
                        }
                        self.last_tool_event = event_msg;
                        self.last_status = format!("{} done", provider.as_str());
                    }
                    Ok(WorkerEvent::Done(final_text)) => {
                        processed_any = true;
                        render_changed = true;
                        if self.assistant_idx.is_some() {
                            if !self.stream_had_chunk {
                                let final_text = final_text.trim();
                                if let Some(i) = self.assistant_idx {
                                    if let Some(entry) = self.entries.get_mut(i) {
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
                                    if entry.text.contains(THINKING_PLACEHOLDER) {
                                        entry.text =
                                            entry.text.replacen(THINKING_PLACEHOLDER, "", 1);
                                    }
                                    entry.text.push_str(final_text.trim());
                                }
                            }
                        }
                        self.clear_running_state();
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
                        self.last_tool_event = msg.clone();
                        self.last_status = format!("tool: {}", truncate(&msg, 48));
                        let always_visible = msg.starts_with("codex ");
                        if self.show_tool_events || always_visible {
                            self.push_entry(EntryKind::Tool, msg);
                        }
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
            self.push_entry(EntryKind::System, "task is running, wait...");
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
                    self.input.clear();
                    self.cursor = 0;
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
            if let Some(memory) = &self.memory {
                if let Err(err) = memory.clear_session(&self.session_id) {
                    self.push_entry(
                        EntryKind::System,
                        format!("memory clear failed: {}", truncate(&err.to_string(), 80)),
                    );
                }
            }
            self.push_entry(EntryKind::System, "cleared");
            self.input.clear();
            self.cursor = 0;
            return;
        }

        if line == "/events on" {
            self.show_tool_events = true;
            self.push_entry(EntryKind::System, "tool events: on");
            self.input.clear();
            self.cursor = 0;
            return;
        }

        if line == "/events off" {
            self.show_tool_events = false;
            self.push_entry(EntryKind::System, "tool events: off");
            self.input.clear();
            self.cursor = 0;
            return;
        }

        if let Some(rest) = line.strip_prefix("/theme") {
            self.handle_theme_change(rest.trim());
            self.input.clear();
            self.cursor = 0;
            return;
        }

        if let Some(rest) = line.strip_prefix("/provider") {
            let target = rest.trim();
            self.handle_primary_change(target);
            self.input.clear();
            self.cursor = 0;
            return;
        }

        if let Some(rest) = line.strip_prefix("/primary") {
            let target = rest.trim();
            self.handle_primary_change(target);
            self.input.clear();
            self.cursor = 0;
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
            self.input.clear();
            self.cursor = 0;
            return;
        }

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
        self.input.clear();
        self.cursor = 0;

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

    fn filtered_commands(&self) -> Vec<String> {
        let q = self.palette_query.to_lowercase();
        if q.trim().is_empty() {
            return self.commands.clone();
        }
        self.commands
            .iter()
            .filter(|c| c.to_lowercase().contains(&q))
            .cloned()
            .collect()
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
            Mode::CommandPalette => self.handle_palette_key(key),
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

    fn handle_palette_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Up => {
                if self.palette_idx > 0 {
                    self.palette_idx -= 1;
                }
            }
            KeyCode::Down => {
                let len = self.filtered_commands().len();
                if len > 0 && self.palette_idx + 1 < len {
                    self.palette_idx += 1;
                }
            }
            KeyCode::Enter => {
                let items = self.filtered_commands();
                if let Some(selected) = items.get(self.palette_idx) {
                    self.input = selected.clone();
                    self.cursor = self.input.len();
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                self.palette_query.pop();
                self.palette_idx = 0;
            }
            KeyCode::Char(c) => {
                self.palette_query.push(c);
                self.palette_idx = 0;
            }
            _ => {}
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
                    self.mode = Mode::CommandPalette;
                    self.palette_query.clear();
                    self.palette_idx = 0;
                    return;
                }
                KeyCode::Char('r') => {
                    self.mode = Mode::HistorySearch;
                    self.history_query.clear();
                    self.history_idx = 0;
                    return;
                }
                KeyCode::Char('t') => {
                    self.show_tool_events = !self.show_tool_events;
                    self.last_status = if self.show_tool_events {
                        "tool events on".to_string()
                    } else {
                        "tool events off".to_string()
                    };
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
        let mut lines = Vec::<Line>::new();
        let palette = self.theme.palette();

        for (idx, entry) in self.entries.iter().enumerate() {
            if !self.show_tool_events && matches!(entry.kind, EntryKind::Tool) {
                // Keep Codex progress visible even when generic tool events are hidden.
                if !entry.text.starts_with("codex ") {
                    continue;
                }
            }

            let entry_provider =
                extract_agent_name(&entry.text).and_then(|n| provider_from_name(&n));
            let is_current_entry =
                self.assistant_idx == Some(idx) || self.agent_entries.values().any(|&i| i == idx);
            let is_processing =
                self.running && matches!(entry.kind, EntryKind::Assistant) && is_current_entry;
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
                    lines.push(Line::from(Span::styled(label, label_style)));

                    let raw_text = if entry.text.trim().is_empty() {
                        "…".to_string()
                    } else {
                        cleaned_assistant_text(entry).to_string()
                    };
                    let base_style = if is_processing {
                        Style::default().fg(palette.assistant_processing_text)
                    } else {
                        Style::default().fg(palette.assistant_text)
                    };
                    let md_lines = render_markdown(&raw_text, base_style, palette);
                    for md_line in md_lines {
                        let mut spans = vec![Span::raw("  ")];
                        spans.extend(md_line);
                        lines.push(Line::from(spans));
                    }
                }
                EntryKind::System => {
                    lines.push(Line::from(vec![Span::styled(
                        format!("[sys] {}", entry.text),
                        Style::default().fg(palette.system_text),
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
    let heading_style = base_style
        .fg(palette.banner_title)
        .add_modifier(Modifier::BOLD);
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
                    Style::default().fg(palette.muted_text),
                )]);
            } else {
                result.push(vec![
                    Span::styled("─── ".to_string(), Style::default().fg(palette.muted_text)),
                    Span::styled(
                        lang.to_string(),
                        Style::default()
                            .fg(palette.muted_text)
                            .add_modifier(Modifier::ITALIC),
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
                        Style::default().fg(palette.muted_text),
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
    fn compute_append_ranges_keeps_unchanged_tail_and_appended_end() {
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
        assert_eq!(compute_append_ranges(&old, &new), vec![(1, 2), (4, 5)]);
    }
}
