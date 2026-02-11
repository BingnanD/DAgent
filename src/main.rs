use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Stdout};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const APP_VERSION: &str = "0.2.0-rs";
const SPINNER: &[&str] = &["|", "/", "-", "\\"];
const THINKING_PLACEHOLDER: &str = "(thinking...)";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "--version" | "-v" => {
                println!("dagent {}", APP_VERSION);
                return Ok(());
            }
            unknown => {
                eprintln!("unknown argument: {}", unknown);
                std::process::exit(2);
            }
        }
    }

    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal);
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alt screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("create terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leave alt screen")?;
    terminal.show_cursor().context("show cursor")?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn as_str(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    fn binary(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    fn all() -> [Provider; 2] {
        [Provider::Claude, Provider::Codex]
    }
}

#[derive(Clone, Copy, Debug)]
enum EntryKind {
    User,
    Assistant,
    System,
    Tool,
    Error,
}

#[derive(Clone, Debug)]
struct LogEntry {
    kind: EntryKind,
    text: String,
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
enum WorkerEvent {
    Done(String),
    AgentStart(Provider),
    AgentChunk { provider: Provider, chunk: String },
    AgentDone(Provider),
    Tool(String),
    PromotePrimary { to: Provider, reason: String },
    Error(String),
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

    rx: Option<Receiver<WorkerEvent>>,
    assistant_idx: Option<usize>,
    stream_had_chunk: bool,
    agent_entries: HashMap<Provider, usize>,
    agent_had_chunk: HashMap<Provider, bool>,
    active_provider: Option<Provider>,

    last_status: String,
}

impl App {
    fn new() -> Self {
        let available_providers = detect_available_providers();
        let primary_provider = if available_providers.contains(&Provider::Claude) {
            Provider::Claude
        } else {
            *available_providers.first().unwrap_or(&Provider::Claude)
        };
        Self {
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
            show_tool_events: false,
            rx: None,
            assistant_idx: None,
            stream_had_chunk: false,
            agent_entries: HashMap::new(),
            agent_had_chunk: HashMap::new(),
            active_provider: None,
            last_status: "ready".to_string(),
        }
    }

    fn push_entry(&mut self, kind: EntryKind, text: impl Into<String>) {
        self.entries.push(LogEntry {
            kind,
            text: text.into(),
        });
        if self.autoscroll {
            self.scroll = self.scroll_max();
        }
    }

    fn scroll_max(&self) -> u16 {
        self.render_log_lines().len().saturating_sub(1) as u16
    }

    fn has_chat_activity(&self) -> bool {
        self.running
            || !self.history.is_empty()
            || self.entries.iter().any(|entry| {
                matches!(
                    entry.kind,
                    EntryKind::User | EntryKind::Assistant | EntryKind::Tool | EntryKind::Error
                )
            })
    }

    fn input_height(&self, width: u16, prompt_width: u16) -> u16 {
        if self.input.is_empty() {
            return 1;
        }
        let (_, end_y) = input_cursor_position(&self.input, self.input.len(), width, prompt_width);
        end_y.saturating_add(1).clamp(1, 6)
    }

    fn on_tick(&mut self) {
        if self.running {
            self.spinner_idx = (self.spinner_idx + 1) % SPINNER.len();
        }
        self.poll_worker();
    }

    fn poll_worker(&mut self) {
        if let Some(rx) = self.rx.clone() {
            loop {
                match rx.try_recv() {
                    Ok(WorkerEvent::AgentStart(provider)) => {
                        self.active_provider = Some(provider);
                        self.last_status = format!("{} working", provider.as_str());
                    }
                    Ok(WorkerEvent::AgentChunk { provider, chunk }) => {
                        if let Some(i) = self.agent_entries.get(&provider).copied() {
                            if let Some(entry) = self.entries.get_mut(i) {
                                let had_chunk =
                                    self.agent_had_chunk.get(&provider).copied().unwrap_or(false);
                                if !had_chunk && entry.text.contains(THINKING_PLACEHOLDER) {
                                    entry.text = entry.text.replacen(THINKING_PLACEHOLDER, "", 1);
                                }
                                entry.text.push_str(&chunk);
                                self.agent_had_chunk.insert(provider, true);
                            }
                        }
                    }
                    Ok(WorkerEvent::AgentDone(provider)) => {
                        if let Some(i) = self.agent_entries.get(&provider).copied() {
                            let had_chunk =
                                self.agent_had_chunk.get(&provider).copied().unwrap_or(false);
                            if let Some(entry) = self.entries.get_mut(i) {
                                if !had_chunk {
                                    if entry.text.contains(THINKING_PLACEHOLDER) {
                                        entry.text =
                                            entry.text.replacen(THINKING_PLACEHOLDER, "(no output)", 1);
                                    } else if entry.text.trim().is_empty() {
                                        entry.text = "(no output)".to_string();
                                    }
                                }
                            }
                        }
                        if self.active_provider == Some(provider) {
                            self.active_provider = None;
                        }
                        self.last_status = format!("{} done", provider.as_str());
                    }
                    Ok(WorkerEvent::Done(final_text)) => {
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
                        self.running = false;
                        self.rx = None;
                        self.assistant_idx = None;
                        self.stream_had_chunk = false;
                        self.agent_entries.clear();
                        self.agent_had_chunk.clear();
                        self.active_provider = None;
                        self.last_status = "done".to_string();
                        break;
                    }
                    Ok(WorkerEvent::Tool(msg)) => {
                        self.last_status = format!("tool: {}", truncate(&msg, 48));
                        if self.show_tool_events {
                            self.entries.push(LogEntry {
                                kind: EntryKind::Tool,
                                text: msg,
                            });
                            if self.autoscroll && !self.running {
                                self.scroll = self.scroll_max();
                            }
                        }
                    }
                    Ok(WorkerEvent::PromotePrimary { to, reason }) => {
                        if self.primary_provider != to {
                            self.primary_provider = to;
                            self.push_entry(
                                EntryKind::System,
                                format!(
                                    "primary auto-switched to {} ({})",
                                    to.as_str(),
                                    reason
                                ),
                            );
                            self.last_status = format!("primary -> {}", to.as_str());
                        }
                    }
                    Ok(WorkerEvent::Error(err)) => {
                        if let Some(provider) = self.active_provider {
                            if let Some(i) = self.agent_entries.get(&provider).copied() {
                                if let Some(entry) = self.entries.get_mut(i) {
                                    if entry.text.contains(THINKING_PLACEHOLDER) {
                                        entry.text =
                                            entry.text.replacen(THINKING_PLACEHOLDER, "(failed)", 1);
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
                        self.running = false;
                        self.rx = None;
                        self.assistant_idx = None;
                        self.stream_had_chunk = false;
                        self.agent_entries.clear();
                        self.agent_had_chunk.clear();
                        self.active_provider = None;
                        self.last_status = "error".to_string();
                        break;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        if let Some(provider) = self.active_provider {
                            if let Some(i) = self.agent_entries.get(&provider).copied() {
                                if let Some(entry) = self.entries.get_mut(i) {
                                    if entry.text.contains(THINKING_PLACEHOLDER) {
                                        entry.text = entry
                                            .text
                                            .replacen(THINKING_PLACEHOLDER, "(disconnected)", 1);
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
                        self.running = false;
                        self.rx = None;
                        self.assistant_idx = None;
                        self.stream_had_chunk = false;
                        self.agent_entries.clear();
                        self.agent_had_chunk.clear();
                        self.active_provider = None;
                        break;
                    }
                }
            }
        }
    }

    fn submit_current_line(&mut self, force: bool) {
        let line = self.input.trim().to_string();
        if line.is_empty() {
            return;
        }

        if line == "/exit" || line == "/quit" {
            self.should_quit = true;
            return;
        }

        if self.running {
            self.push_entry(EntryKind::System, "task is running, wait...");
            return;
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

        self.history.push(line.clone());
        self.history_pos = None;

        let is_slash = line.starts_with('/');
        let providers = if is_slash {
            Vec::new()
        } else {
            ordered_providers(self.primary_provider, &self.available_providers)
        };
        if !is_slash && providers.is_empty() {
            self.push_entry(
                EntryKind::Error,
                "no available agent found (need claude and/or codex on PATH)",
            );
            self.input.clear();
            self.cursor = 0;
            return;
        }

        self.push_entry(EntryKind::User, line.clone());
        self.assistant_idx = None;
        self.agent_entries.clear();
        self.agent_had_chunk.clear();
        self.active_provider = None;
        if is_slash {
            self.push_entry(EntryKind::Assistant, THINKING_PLACEHOLDER.to_string());
            self.assistant_idx = Some(self.entries.len() - 1);
        } else {
            for provider in providers {
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
        self.running = true;
        self.last_status = "running".to_string();
        self.input.clear();
        self.cursor = 0;

        let provider = self.primary_provider;
        let available = self.available_providers.clone();
        let (tx, rx) = unbounded::<WorkerEvent>();
        std::thread::spawn(move || execute_line(provider, available, line, tx));
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
            format!(
                "primary set to {} (all agents still active: {})",
                self.primary_provider.as_str(),
                providers_label(&self.available_providers)
            ),
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

    fn apply_selected_slash_hint(&mut self) -> bool {
        let hints = self.slash_hints();
        if hints.is_empty() {
            return false;
        }
        let idx = self.slash_hint_idx.min(hints.len().saturating_sub(1));
        if let Some(selected) = hints.get(idx) {
            self.input = selected.clone();
            self.cursor = self.input.len();
            return true;
        }
        false
    }

    fn sync_slash_hint_idx(&mut self) {
        let len = self.slash_hints().len();
        if len == 0 {
            self.slash_hint_idx = 0;
            return;
        }
        if self.slash_hint_idx >= len {
            self.slash_hint_idx = len - 1;
        }
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
        if self.input.starts_with('/') {
            self.slash_hint_idx = 0;
            self.sync_slash_hint_idx();
        }
    }

    fn backspace(&mut self) {
        if self.cursor == 0 || self.input.is_empty() {
            return;
        }
        if let Some(prev_idx) = self.input[..self.cursor].char_indices().last().map(|(i, _)| i) {
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
        if let Some(prev_idx) = self.input[..self.cursor].char_indices().last().map(|(i, _)| i) {
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
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('l') => {
                    self.entries.clear();
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
                self.autoscroll = false;
                self.scroll = self.scroll.saturating_sub(5);
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(5).min(self.scroll_max());
                if self.scroll >= self.scroll_max().saturating_sub(2) {
                    self.autoscroll = true;
                }
            }
            KeyCode::Up => {
                if self.input.starts_with('/') {
                    let hints = self.slash_hints();
                    if !hints.is_empty() {
                        if self.slash_hint_idx == 0 {
                            self.slash_hint_idx = hints.len() - 1;
                        } else {
                            self.slash_hint_idx -= 1;
                        }
                        return;
                    }
                }
                self.history_prev();
            }
            KeyCode::Down => {
                if self.input.starts_with('/') {
                    let hints = self.slash_hints();
                    if !hints.is_empty() {
                        self.slash_hint_idx = (self.slash_hint_idx + 1) % hints.len();
                        return;
                    }
                }
                self.history_next();
            }
            KeyCode::Tab => {
                if self.input.starts_with('/') {
                    if self.apply_selected_slash_hint() {
                        return;
                    }
                }
            }
            KeyCode::Enter => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.insert_char('\n');
                } else {
                    self.submit_current_line(false);
                }
            }
            KeyCode::Backspace => {
                self.backspace();
                self.sync_slash_hint_idx();
            }
            KeyCode::Delete => {
                self.delete();
                self.sync_slash_hint_idx();
            }
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Char(c) => {
                self.insert_char(c);
                if self.input.starts_with('/') {
                    self.slash_hint_idx = 0;
                    self.sync_slash_hint_idx();
                }
            }
            _ => {}
        }
    }

    fn render_log_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::<Line>::new();

        for (idx, entry) in self.entries.iter().enumerate() {
            if !self.show_tool_events && matches!(entry.kind, EntryKind::Tool) {
                continue;
            }

            let entry_provider = extract_agent_name(&entry.text).and_then(|n| provider_from_name(&n));
            let is_processing = self.running
                && matches!(entry.kind, EntryKind::Assistant)
                && (self.assistant_idx == Some(idx) || self.active_provider == entry_provider);
            match entry.kind {
                EntryKind::User => {
                    let parts: Vec<&str> = entry.text.split('\n').collect();
                    for part in parts {
                        let content = if part.is_empty() { " " } else { part };
                        lines.push(Line::from(vec![Span::styled(
                            format!(" {} ", content),
                            Style::default()
                                .fg(Color::White)
                                .bg(Color::Rgb(44, 62, 80))
                                .add_modifier(Modifier::BOLD),
                        )]));
                    }
                }
                EntryKind::Assistant => {
                    let provider = entry_provider.unwrap_or(self.primary_provider);
                    let provider_color = match provider {
                        Provider::Claude => Color::Green,
                        Provider::Codex => Color::LightBlue,
                    };
                    let label = if is_processing {
                        format!("{} [working]", provider.as_str())
                    } else {
                        provider.as_str().to_string()
                    };
                    let label_style = if is_processing {
                        Style::default()
                            .fg(Color::Yellow)
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
                    let text_style = if is_processing {
                        Style::default().fg(Color::LightYellow)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    for part in raw_text.split('\n') {
                        let content = if part.is_empty() { " " } else { part };
                        lines.push(Line::from(vec![Span::raw("  "), Span::styled(content.to_string(), text_style)]));
                    }
                }
                EntryKind::System => {
                    lines.push(Line::from(vec![Span::styled(
                        format!("· {}", entry.text),
                        Style::default().fg(Color::DarkGray),
                    )]));
                }
                EntryKind::Tool => {
                    lines.push(Line::from(vec![
                        Span::styled("工具: ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                        Span::styled(entry.text.clone(), Style::default().fg(Color::White)),
                    ]));
                }
                EntryKind::Error => {
                    lines.push(Line::from(vec![
                        Span::styled("错误: ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                        Span::styled(entry.text.clone(), Style::default().fg(Color::LightRed)),
                    ]));
                }
            }
            lines.push(Line::from(""));
        }

        if lines.is_empty() {
            lines.push(Line::from(""));
        }

        lines
    }
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let mut app = App::new();
    let tick_rate = Duration::from_millis(70);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| draw(f, &app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or(Duration::from_millis(0));

        if event::poll(timeout).context("event poll")? {
            match event::read().context("event read")? {
                Event::Key(key) => {
                    if !matches!(key.kind, KeyEventKind::Release) {
                        app.handle_key(key);
                    }
                }
                Event::Paste(text) => app.insert_str(&text),
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick();
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn draw(f: &mut Frame, app: &App) {
    let prompt_prefix = "> ";
    let prompt_width = UnicodeWidthStr::width(prompt_prefix) as u16;
    let max_input_height = f.size().height.saturating_sub(4).max(1);
    let input_height = app
        .input_height(f.size().width.max(1), prompt_width)
        .min(max_input_height);
    let prompt_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let input_lines = build_input_lines(app, prompt_prefix, prompt_style);
    let hint_line = build_hint_line(app);

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(1), Constraint::Length(1)])
        .split(f.size());

    if !app.has_chat_activity() {
        let welcome = centered_rect(78, 48, root[0]);
        let welcome_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(input_height),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(welcome);

        let hero = Paragraph::new(Text::from(vec![
            Line::from(Span::styled(
                "DAgent",
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Super Agent Console",
                Style::default().fg(Color::Gray),
            )),
        ]))
        .alignment(Alignment::Center);
        f.render_widget(hero, welcome_chunks[0]);

        let tips = Paragraph::new(Text::from(vec![
            Line::from(Span::styled(
                "输入你的任务，DAgent 会并行调度 claude + codex",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "Enter 发送 | Shift+Enter 换行 | Ctrl+K 命令面板",
                Style::default().fg(Color::DarkGray),
            )),
        ]))
        .alignment(Alignment::Center);
        f.render_widget(tips, welcome_chunks[1]);

        let input = Paragraph::new(Text::from(input_lines))
            .style(Style::default().bg(Color::Rgb(28, 32, 40)))
            .wrap(Wrap { trim: false });
        f.render_widget(input, welcome_chunks[2]);

        let hint_panel = Paragraph::new(Text::from(vec![hint_line]));
        f.render_widget(hint_panel, welcome_chunks[3]);

        if matches!(app.mode, Mode::Normal) {
            let (cx, cy) = input_cursor_position(
                &app.input,
                app.cursor,
                welcome_chunks[2].width,
                prompt_width,
            );
            let cursor_x = welcome_chunks[2].x + cx.min(welcome_chunks[2].width.saturating_sub(1));
            let cursor_y = welcome_chunks[2].y + cy.min(welcome_chunks[2].height.saturating_sub(1));
            f.set_cursor(cursor_x, cursor_y);
        }
    } else {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(6),
                Constraint::Length(input_height),
                Constraint::Length(1),
            ])
            .split(root[0]);

        let mut log_lines = app.render_log_lines();
        let body_height = outer[0].height as usize;
        if body_height > 0 && log_lines.len() < body_height {
            let mut padded = vec![Line::from(""); body_height - log_lines.len()];
            padded.extend(log_lines);
            log_lines = padded;
        }
        let max_offset = log_lines.len().saturating_sub(body_height) as u16;
        let scroll_offset = if app.autoscroll {
            max_offset
        } else {
            app.scroll.min(max_offset)
        };
        let body = Paragraph::new(Text::from(log_lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset, 0));
        f.render_widget(body, outer[0]);

        let input = Paragraph::new(Text::from(input_lines))
            .style(Style::default().bg(Color::Rgb(28, 32, 40)))
            .wrap(Wrap { trim: false });
        f.render_widget(input, outer[1]);

        let hint_panel = Paragraph::new(Text::from(vec![hint_line]));
        f.render_widget(hint_panel, outer[2]);

        if matches!(app.mode, Mode::Normal) {
            let (cx, cy) =
                input_cursor_position(&app.input, app.cursor, outer[1].width, prompt_width);
            let cursor_x = outer[1].x + cx.min(outer[1].width.saturating_sub(1));
            let cursor_y = outer[1].y + cy.min(outer[1].height.saturating_sub(1));
            f.set_cursor(cursor_x, cursor_y);
        }
    }

    let meta = Paragraph::new(format!(
        " DAgent {} | primary:{} | agents:{} | {} {} ",
        APP_VERSION,
        app.primary_provider.as_str(),
        providers_label(&app.available_providers),
        if app.running { "running" } else { "idle" },
        if app.running { SPINNER[app.spinner_idx] } else { "" }
    ))
    .style(Style::default().fg(Color::Gray));
    f.render_widget(meta, root[1]);

    let status = Paragraph::new(format!(
        " {} | Ctrl+K cmds | Ctrl+R history | Ctrl+T tool events:{} | Ctrl+A/E home/end | /primary [claude|codex] | / + Up/Down/Tab ",
        app.last_status,
        if app.show_tool_events { "on" } else { "off" }
    ))
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(status, root[2]);

    if matches!(app.mode, Mode::CommandPalette) {
        draw_palette(f, app);
    }
    if matches!(app.mode, Mode::HistorySearch) {
        draw_history(f, app);
    }
    if matches!(app.mode, Mode::Approval) {
        draw_approval(f, app);
    }
}

fn build_input_lines(app: &App, prompt_prefix: &str, prompt_style: Style) -> Vec<Line<'static>> {
    if app.input.is_empty() {
        return vec![Line::from(vec![
            Span::styled(prompt_prefix.to_string(), prompt_style),
            Span::styled("Type message...", Style::default().fg(Color::DarkGray)),
        ])];
    }

    let mut lines = Vec::new();
    let indent = " ".repeat(prompt_prefix.chars().count());
    for (idx, part) in app.input.split('\n').enumerate() {
        if idx == 0 {
            lines.push(Line::from(vec![
                Span::styled(prompt_prefix.to_string(), prompt_style),
                Span::styled(part.to_string(), Style::default().fg(Color::White)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(indent.clone(), prompt_style),
                Span::styled(part.to_string(), Style::default().fg(Color::White)),
            ]));
        }
    }
    lines
}

fn build_hint_line(app: &App) -> Line<'static> {
    let hints = app.slash_hints();
    if hints.is_empty() {
        return Line::from(" ");
    }

    let mut spans = vec![Span::styled(
        " / suggestions: ",
        Style::default().fg(Color::DarkGray),
    )];
    let selected = app.slash_hint_idx.min(hints.len().saturating_sub(1));
    for (i, hint) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        if i == selected {
            spans.push(Span::styled(
                hint.clone(),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(hint.clone(), Style::default().fg(Color::DarkGray)));
        }
    }
    Line::from(spans)
}

fn draw_palette(f: &mut Frame, app: &App) {
    let area = centered_rect(70, 58, f.size());
    let items = app.filtered_commands();
    let mut lines = vec![
        Line::from(Span::styled(
            "Command Palette",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("query: {}", app.palette_query)),
        Line::from(""),
    ];
    if items.is_empty() {
        lines.push(Line::from("(no match)"));
    } else {
        for (i, cmd) in items.iter().take(12).enumerate() {
            let prefix = if i == app.palette_idx { "> " } else { "  " };
            lines.push(Line::from(format!("{}{}", prefix, cmd)));
        }
    }
    let panel = Paragraph::new(lines).style(
        Style::default()
            .bg(Color::Black)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(Clear, area);
    f.render_widget(panel, area);
}

fn draw_history(f: &mut Frame, app: &App) {
    let area = centered_rect(70, 58, f.size());
    let items = app.filtered_history();
    let mut lines = vec![
        Line::from(Span::styled(
            "History Search",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("query: {}", app.history_query)),
        Line::from(""),
    ];
    if items.is_empty() {
        lines.push(Line::from("(no match)"));
    } else {
        for (i, item) in items.iter().enumerate() {
            let prefix = if i == app.history_idx { "> " } else { "  " };
            lines.push(Line::from(format!("{}{}", prefix, item)));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Enter apply | Esc close"));

    let panel = Paragraph::new(lines).style(
        Style::default()
            .bg(Color::Black)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(Clear, area);
    f.render_widget(panel, area);
}

fn draw_approval(f: &mut Frame, app: &App) {
    let area = centered_rect(64, 40, f.size());
    let pending = app.approval.as_ref();
    let lines = if let Some(p) = pending {
        vec![
            Line::from(Span::styled(
                "Approval Required",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!("tool: {}", p.tool)),
            Line::from(p.reason.clone()),
            Line::from(""),
            Line::from(format!("cmd: {}", truncate(&p.line, 90))),
            Line::from(""),
            Line::from("[y] approve once   [a] always allow   [n] deny"),
        ]
    } else {
        vec![Line::from("No pending approval")]
    };
    let panel = Paragraph::new(lines).style(
        Style::default()
            .bg(Color::Black)
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(Clear, area);
    f.render_widget(panel, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn default_commands() -> Vec<String> {
    vec![
        "/help".to_string(),
        "/commands".to_string(),
        "/primary claude".to_string(),
        "/primary codex".to_string(),
        "/events on".to_string(),
        "/events off".to_string(),
        "/tool echo hello".to_string(),
        "/tool time".to_string(),
        "/tool bash ls -la".to_string(),
        "/clear".to_string(),
        "/exit".to_string(),
    ]
}

fn high_risk_check(line: &str) -> Option<(String, String)> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "/tool" {
        return None;
    }
    let tool = parts.next()?.to_string();
    if matches!(tool.as_str(), "bash" | "shell" | "exec") {
        Some((
            tool.clone(),
            format!("high-risk tool '{}' needs approval", tool),
        ))
    } else {
        None
    }
}

fn execute_line(
    primary_provider: Provider,
    available_providers: Vec<Provider>,
    line: String,
    tx: Sender<WorkerEvent>,
) {
    if line.starts_with('/') {
        let res = execute_slash_command(&line, &tx);
        match res {
            Ok(text) => {
                let _ = tx.send(WorkerEvent::Done(text));
            }
            Err(err) => {
                let _ = tx.send(WorkerEvent::Error(err));
            }
        }
        return;
    }

    let providers = ordered_providers(primary_provider, &available_providers);
    if providers.is_empty() {
        let _ = tx.send(WorkerEvent::Error(
            "no available agent found (need claude and/or codex on PATH)".to_string(),
        ));
        return;
    }

    let mut had_success = false;
    for provider in providers {
        let _ = tx.send(WorkerEvent::Tool(format!(
            "agent {} processing",
            provider.as_str()
        )));
        let _ = tx.send(WorkerEvent::AgentStart(provider));
        match run_provider_stream(provider, &line, &tx) {
            Ok(final_text) => {
                had_success = true;
                if !final_text.trim().is_empty() {
                    let _ = tx.send(WorkerEvent::AgentChunk {
                        provider,
                        chunk: final_text.trim().to_string(),
                    });
                }
            }
            Err(err) => {
                if let Some(to) = pick_promoted_provider(provider, &available_providers, &err) {
                    let _ = tx.send(WorkerEvent::PromotePrimary {
                        to,
                        reason: err.clone(),
                    });
                }
                let _ = tx.send(WorkerEvent::AgentChunk {
                    provider,
                    chunk: format!("{} error: {}", provider.as_str(), err),
                });
            }
        }
        let _ = tx.send(WorkerEvent::AgentDone(provider));
    }

    if had_success {
        let _ = tx.send(WorkerEvent::Done(String::new()));
    } else {
        let _ = tx.send(WorkerEvent::Error(
            "all available agents failed for this request".to_string(),
        ));
    }
}

fn run_provider_stream(
    provider: Provider,
    prompt: &str,
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<String, String> {
    match provider {
        Provider::Claude => run_claude_stream(provider, prompt, tx),
        Provider::Codex => run_codex_stream(provider, prompt, tx),
    }
}

fn execute_slash_command(line: &str, tx: &Sender<WorkerEvent>) -> std::result::Result<String, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(String::new());
    }
    match parts[0] {
        "/help" => Ok(help_text()),
        "/commands" => Ok(default_commands().join("\n")),
        "/tool" => run_tool(parts, tx),
        "/provider" => Ok("provider alias enabled; use /primary".to_string()),
        "/primary" => Ok("primary change handled in UI".to_string()),
        "/events" => Ok("events toggle handled in UI".to_string()),
        "/clear" => Ok("clear handled in UI".to_string()),
        _ => Err("unknown command. use /help".to_string()),
    }
}

fn run_tool(parts: Vec<&str>, tx: &Sender<WorkerEvent>) -> std::result::Result<String, String> {
    if parts.len() < 2 {
        return Err("usage: /tool <name> [input]".to_string());
    }
    let tool = parts[1];
    let input = if parts.len() > 2 {
        parts[2..].join(" ")
    } else {
        String::new()
    };

    let _ = tx.send(WorkerEvent::Tool(format!("invoke {} {}", tool, input)));

    match tool {
        "echo" => Ok(input),
        "time" => Ok(format!("{:?}", std::time::SystemTime::now())),
        "bash" => {
            if input.trim().is_empty() {
                return Err("usage: /tool bash <command>".to_string());
            }
            let output = Command::new("bash")
                .arg("-lc")
                .arg(&input)
                .output()
                .map_err(|e| format!("bash failed: {e}"))?;
            let mut text = String::new();
            if !output.stdout.is_empty() {
                text.push_str(&String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&String::from_utf8_lossy(&output.stderr));
            }
            Ok(text.trim().to_string())
        }
        _ => Err(format!("unknown tool: {}", tool)),
    }
}

fn run_claude_stream(
    provider: Provider,
    prompt: &str,
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<String, String> {
    let mut cmd = Command::new("claude");
    cmd.args([
        "--print",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        prompt,
    ]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("claude spawn failed: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "claude stdout missing".to_string())?;
    let reader = BufReader::new(stdout);

    let mut fallback_lines: Vec<String> = Vec::new();
    let mut quota_message = String::new();
    let mut saw_quota_error = false;
    let mut emitted = false;
    let mut emitted_non_quota = false;
    for line in reader.lines() {
        let line = line.map_err(|e| format!("claude stream read failed: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        fallback_lines.push(line.clone());
        if is_quota_error_text(&line) {
            saw_quota_error = true;
        }
        if let Some(chunk) = extract_claude_delta_text(&line) {
            if !chunk.trim().is_empty() {
                if is_quota_error_text(&chunk) {
                    saw_quota_error = true;
                    if quota_message.is_empty() {
                        quota_message = chunk;
                    }
                } else {
                    emitted = true;
                    emitted_non_quota = true;
                    let _ = tx.send(WorkerEvent::AgentChunk { provider, chunk });
                }
            }
        }
    }

    let status = child.wait().map_err(|e| format!("claude wait failed: {e}"))?;
    if status.success() {
        if saw_quota_error && !emitted_non_quota {
            let msg = if quota_message.is_empty() {
                "claude quota/rate limit reached".to_string()
            } else {
                format!("claude quota/rate limit: {}", quota_message)
            };
            return Err(msg);
        }
        if emitted {
            return Ok(String::new());
        }
        if let Some(text) = fallback_lines
            .iter()
            .rev()
            .find_map(|line| extract_claude_fallback_text(line))
        {
            if is_quota_error_text(&text) {
                return Err(format!("claude quota/rate limit: {}", text));
            }
            return Ok(text);
        }
        let last = fallback_lines.last().cloned().unwrap_or_default();
        if is_quota_error_text(&last) {
            return Err(format!("claude quota/rate limit: {}", last));
        }
        return Ok(last);
    }

    let output = Command::new("claude")
        .args(["-p", prompt])
        .output()
        .map_err(|e| format!("claude fallback failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "claude failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let fallback_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if is_quota_error_text(&fallback_text) {
        return Err(format!("claude quota/rate limit: {}", fallback_text));
    }
    Ok(fallback_text)
}

fn run_codex_stream(
    provider: Provider,
    prompt: &str,
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<String, String> {
    let mut cmd = Command::new("codex");
    cmd.args(["exec", "--json", "--skip-git-repo-check", prompt]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("codex spawn failed: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "codex stdout missing".to_string())?;
    let reader = BufReader::new(stdout);

    let mut fallback_lines: Vec<String> = Vec::new();
    let mut emitted = false;
    for line in reader.lines() {
        let line = line.map_err(|e| format!("codex stream read failed: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        fallback_lines.push(line.clone());
        if let Some(chunk) = extract_codex_text(&line) {
            if !chunk.trim().is_empty() {
                emitted = true;
                let _ = tx.send(WorkerEvent::AgentChunk { provider, chunk });
            }
        }
    }

    let status = child.wait().map_err(|e| format!("codex wait failed: {e}"))?;
    if status.success() {
        if emitted {
            return Ok(String::new());
        }
        if let Some(text) = fallback_lines
            .iter()
            .rev()
            .find_map(|line| extract_codex_text(line))
        {
            return Ok(text);
        }
        return Ok(String::new());
    }

    let output = Command::new("codex")
        .args(["exec", "--skip-git-repo-check", prompt])
        .output()
        .map_err(|e| format!("codex fallback failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "codex failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_json_line(line: &str) -> Option<Value> {
    serde_json::from_str(line).ok()
}

fn extract_claude_delta_text(line: &str) -> Option<String> {
    let value = parse_json_line(line)?;
    if value.get("type")?.as_str()? != "stream_event" {
        return None;
    }
    let event = value.get("event")?;
    if event.get("type")?.as_str()? != "content_block_delta" {
        return None;
    }
    let delta = event.get("delta")?;
    if let Some(delta_type) = delta.get("type").and_then(Value::as_str) {
        if delta_type != "text_delta" {
            return None;
        }
    }
    delta.get("text")?.as_str().map(|s| s.to_string())
}

fn extract_claude_fallback_text(line: &str) -> Option<String> {
    let value = parse_json_line(line)?;
    match value.get("type")?.as_str()? {
        "assistant" => value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .and_then(|arr| {
                arr.iter().find_map(|item| {
                    if item.get("type").and_then(Value::as_str) == Some("text") {
                        item.get("text")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            }),
        "result" => value
            .get("result")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
        _ => None,
    }
}

fn extract_codex_text(line: &str) -> Option<String> {
    let value = parse_json_line(line)?;
    if value.get("type")?.as_str()? != "item.completed" {
        return None;
    }
    let item = value.get("item")?;
    if item.get("type")?.as_str()? != "agent_message" {
        return None;
    }
    item.get("text")?.as_str().map(|s| s.to_string())
}

fn help_text() -> String {
    [
        "commands:",
        "  /help",
        "  /commands",
        "  /primary [claude|codex]",
        "  /events [on|off]",
        "  /tool <echo|time|bash> [input]",
        "  /clear",
        "  /exit",
    ]
    .join("\n")
}

fn extract_agent_name(text: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(name) = extract_agent_marker_from_line(t) {
            return Some(name.to_string());
        }
        break;
    }
    None
}

fn provider_from_name(name: &str) -> Option<Provider> {
    match name {
        "claude" => Some(Provider::Claude),
        "codex" => Some(Provider::Codex),
        _ => None,
    }
}

fn extract_agent_marker_from_line(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let name = &rest[..end];
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn cleaned_assistant_text(entry: &LogEntry) -> String {
    if !matches!(entry.kind, EntryKind::Assistant) {
        return entry.text.clone();
    }
    let text = entry.text.trim_end();
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return entry.text.clone();
    };
    if extract_agent_marker_from_line(first.trim()).is_some() {
        let rest = lines.collect::<Vec<_>>().join("\n");
        if rest.trim().is_empty() {
            entry.text.clone()
        } else {
            rest
        }
    } else {
        entry.text.clone()
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}...", &s[..n])
    }
}

fn input_cursor_position(input: &str, cursor: usize, width: u16, prompt_width: u16) -> (u16, u16) {
    let width = width.max(1) as usize;
    let mut x = prompt_width as usize;
    let mut y = 0usize;
    let mut consumed = 0usize;

    for ch in input.chars() {
        let len = ch.len_utf8();
        if consumed + len > cursor {
            break;
        }
        consumed += len;
        if ch == '\n' {
            x = prompt_width as usize;
            y += 1;
            continue;
        }
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if x + ch_width > width {
            x = 0;
            y += 1;
        }
        x += ch_width;
        if x >= width {
            x = 0;
            y += 1;
        }
    }

    (x as u16, y as u16)
}

fn ordered_providers(primary_provider: Provider, available_providers: &[Provider]) -> Vec<Provider> {
    let mut ordered = Vec::new();
    if available_providers.contains(&primary_provider) {
        ordered.push(primary_provider);
    }
    for provider in available_providers {
        if !ordered.contains(provider) {
            ordered.push(*provider);
        }
    }
    ordered
}

fn pick_promoted_provider(
    current: Provider,
    available_providers: &[Provider],
    err: &str,
) -> Option<Provider> {
    if current == Provider::Claude
        && is_quota_error_text(err)
        && available_providers.contains(&Provider::Codex)
    {
        return Some(Provider::Codex);
    }
    None
}

fn is_quota_error_text(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("hit your limit")
        || t.contains("rate_limit")
        || t.contains("rate limit")
        || t.contains("quota")
        || t.contains("credit balance is too low")
        || t.contains("insufficient credits")
        || t.contains("usage limit")
}

fn providers_label(providers: &[Provider]) -> String {
    if providers.is_empty() {
        return "none".to_string();
    }
    providers
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn detect_available_providers() -> Vec<Provider> {
    let mut providers = Vec::new();
    for provider in Provider::all() {
        if command_available(provider.binary()) {
            providers.push(provider);
        }
    }
    providers
}

fn command_available(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        let full = dir.join(bin);
        if is_executable(&full) {
            return true;
        }
    }
    false
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    meta.permissions().mode() & 0o111 != 0
}
