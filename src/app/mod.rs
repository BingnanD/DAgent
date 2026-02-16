use std::collections::{HashMap, HashSet};
use std::io::Stdout;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use crossterm::event::{self, Event, KeyEventKind, MouseEventKind};
use crossterm::terminal::{Clear as TermClear, ClearType};
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    cleaned_assistant_text, cleaned_assistant_text_for_model, default_commands,
    detect_available_providers, execute_line, extract_agent_name, high_risk_check,
    input_cursor_position, kill_pid, memory::MemoryStore, ordered_providers, provider_from_name,
    providers_label, resolve_dispatch_providers, truncate, DispatchTarget, WORKING_PLACEHOLDER,
};

const COLLAPSED_PASTE_CHAR_THRESHOLD: usize = 800;
const COLLAPSED_PASTE_LINE_THRESHOLD: usize = 12;
const MEM_SHOW_DEFAULT_LIMIT: usize = 20;
const MEM_SHOW_MAX_LIMIT: usize = 200;
const MEM_FIND_DEFAULT_LIMIT: usize = 12;
const MEM_PRUNE_DEFAULT_KEEP: usize = 200;
const MAX_ACTIVITY_LOG_LINES: usize = 7;
const STARTUP_BANNER_PREFIX: &str = "__startup_banner__:";
const ASSISTANT_DIVIDER: char = '│';

mod commands;
mod dispatch;
mod input;
mod render;
mod runtime;
mod session;
#[cfg(test)]
mod tests;
mod text;
mod types;
pub(crate) mod ui;
mod worker;

use dispatch::parse_dispatch_override;
pub(crate) use runtime::run_app;
#[cfg(test)]
use runtime::{
    compute_append_ranges, compute_flush_append_ranges, compute_running_append_ranges,
    flatten_line_to_plain, flatten_lines_to_plain,
};
use text::sanitize_runtime_text;
pub(crate) use types::{
    default_theme, EntryKind, LogEntry, Provider, ThemePalette, ThemePreset, WorkerEvent,
};

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

    /// Render transcript lines for running flushes.
    /// This includes streaming rows so users can see in-progress output.
    fn running_flush_log_lines(&self, width: u16) -> Vec<Line<'static>> {
        if !self.running {
            return self.render_entries_lines(width);
        }

        // Scrollback writes are append-only; if we flush a placeholder row now,
        // the first real chunk can only be appended on a new line later.
        // Skip placeholder-only active assistant entries until content arrives.
        let mut active_entry_indices: HashSet<usize> =
            self.agent_entries.values().copied().collect();
        if let Some(idx) = self.assistant_idx {
            active_entry_indices.insert(idx);
        }
        let skip_indices = active_entry_indices
            .into_iter()
            .filter(|idx| {
                self.entries.get(*idx).is_some_and(|entry| {
                    matches!(entry.kind, EntryKind::Assistant)
                        && entry.text.contains(WORKING_PLACEHOLDER)
                        && cleaned_assistant_text(entry).trim().is_empty()
                })
            })
            .collect::<HashSet<_>>();

        if skip_indices.is_empty() {
            self.render_entries_lines(width)
        } else {
            self.render_entries_lines_filtered(width, &skip_indices)
        }
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
}
