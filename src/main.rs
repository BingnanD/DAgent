use std::fs;
use std::io::Stdout;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use crossterm::cursor;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use unicode_width::UnicodeWidthChar;

mod app;
mod memory;
mod orchestrator;
mod providers;

use app::{EntryKind, LogEntry, Provider};

pub(crate) use orchestrator::execute_line;

const APP_VERSION: &str = "0.2.1-rs";
// Breathing dot indicator (kept for potential future use).
#[allow(dead_code)]
const SPINNER: &[&str] = &["●"];
const THINKING_PLACEHOLDER: &str = "(thinking...)";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DispatchTarget {
    Primary,
    All,
    Provider(Provider),
    Providers(Vec<Provider>),
}

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
    let result = app::run_app(&mut terminal);
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    // ratatui::Terminal::insert_before requires at least one line above the viewport.
    // If cursor starts at row 0, move to row 1 first.
    if matches!(cursor::position(), Ok((_, 0))) {
        println!();
    }

    enable_raw_mode().context("enable raw mode")?;

    let term_height = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(24);
    let term_width = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
    let inline_height = compute_inline_height(term_height);

    let mut terminal = match Terminal::with_options(
        CrosstermBackend::new(std::io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(inline_height),
        },
    ) {
        Ok(t) => t,
        Err(inline_err) => {
            // Some terminals/shell wrappers fail cursor-position query required by Inline.
            // Fall back to a fixed bottom viewport to keep app usable.
            let fallback_rect = Rect::new(
                0,
                term_height.saturating_sub(inline_height),
                term_width.max(1),
                inline_height.max(1),
            );
            Terminal::with_options(
                CrosstermBackend::new(std::io::stdout()),
                TerminalOptions {
                    viewport: Viewport::Fixed(fallback_rect),
                },
            )
            .with_context(|| format!("create terminal (inline failed: {inline_err})"))?
        }
    };

    if matches!(supports_keyboard_enhancement(), Ok(true)) {
        crossterm::execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .ok();
    }
    crossterm::execute!(std::io::stdout(), EnableBracketedPaste).ok();

    terminal.hide_cursor().ok();
    Ok(terminal)
}

fn compute_inline_height(term_height: u16) -> u16 {
    let max_allowed = term_height.saturating_sub(1).max(1);
    if let Ok(raw) = std::env::var("DAGENT_INLINE_HEIGHT") {
        if let Ok(parsed) = raw.trim().parse::<u16>() {
            return parsed.clamp(1, max_allowed);
        }
    }

    // Compact composer viewport (activity + input + status + gaps).
    12u16.min(max_allowed).max(6)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    crossterm::execute!(std::io::stdout(), DisableBracketedPaste).ok();
    crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags).ok();
    disable_raw_mode().context("disable raw mode")?;
    terminal.show_cursor().context("show cursor")?;
    println!();
    Ok(())
}

fn default_commands() -> Vec<String> {
    vec![
        "/help".to_string(),
        "/commands".to_string(),
        "/primary claude".to_string(),
        "/primary codex".to_string(),
        "/events on".to_string(),
        "/events off".to_string(),
        "/theme fjord".to_string(),
        "/theme graphite".to_string(),
        "/theme solarized".to_string(),
        "/theme aurora".to_string(),
        "/theme ember".to_string(),
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
        return String::new();
    };
    let cleaned = if extract_agent_marker_from_line(first.trim()).is_some() {
        lines.collect::<Vec<_>>().join("\n")
    } else {
        text.to_string()
    };
    let cleaned_trimmed = cleaned.trim();
    if cleaned_trimmed.is_empty() || cleaned_trimmed == THINKING_PLACEHOLDER {
        String::new()
    } else {
        cleaned
    }
}

fn truncate(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((idx, _)) => format!("{}...", &s[..idx]),
        None => s.to_string(),
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

fn ordered_providers(
    primary_provider: Provider,
    available_providers: &[Provider],
) -> Vec<Provider> {
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

fn kill_pid(pid: u32) {
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
