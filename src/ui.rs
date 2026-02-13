use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::{App, Mode, Provider, ThemePalette};
use crate::{input_cursor_position, providers_label, truncate};

pub(super) fn draw(f: &mut Frame, app: &App) {
    let frame_area = f.area();
    let theme = app.theme_palette();
    let prompt_prefix = "> ";
    let prompt_width = UnicodeWidthStr::width(prompt_prefix) as u16;
    let activity_h: u16 = if app.running {
        let agent_count = app.agent_entries.len().max(1) as u16;
        1 + agent_count // empty line + N agent lines
    } else {
        0
    };
    let hints_h: u16 = if app.inline_hints().is_empty() { 0 } else { 1 };
    // Reserve rows for: activity(0 or 3) + gaps(2~3) + hints(0~1) + status(1).
    let fixed_rows = activity_h + 2 + hints_h + 1;
    let max_input_height = frame_area.height.saturating_sub(fixed_rows).max(3);
    let input_height = app
        .input_height(frame_area.width.max(1), prompt_width)
        .saturating_add(2)
        .min(max_input_height);
    let prompt_style = Style::default()
        .fg(theme.prompt)
        .add_modifier(Modifier::BOLD);
    let input_lines = build_input_lines(app, prompt_prefix, prompt_style, theme);
    // Composer-only viewport: transcript is appended above via insert_before.
    // Layout: [gap] [activity] [gap] [input] [hints] [gap] [status]
    let mut constraints = Vec::new();
    if activity_h > 0 {
        constraints.push(Constraint::Length(1)); // gap above activity
        constraints.push(Constraint::Length(activity_h));
    }
    constraints.push(Constraint::Length(1)); // gap above input
    constraints.push(Constraint::Length(input_height));
    if hints_h > 0 {
        constraints.push(Constraint::Length(hints_h));
    }
    constraints.push(Constraint::Length(1)); // gap above status
    constraints.push(Constraint::Length(1)); // status bar

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame_area);

    let mut section_idx = 0usize;
    let activity_chunk = if activity_h > 0 {
        section_idx += 1; // skip gap
        let c = chunks[section_idx];
        section_idx += 1;
        Some(c)
    } else {
        None
    };
    section_idx += 1; // skip gap above input
    let input_chunk = chunks[section_idx];
    section_idx += 1;
    let hint_chunk = if hints_h > 0 {
        let c = chunks[section_idx];
        section_idx += 1;
        Some(c)
    } else {
        None
    };
    section_idx += 1; // skip gap above status
    let status_chunk = chunks[section_idx];

    // Real-time activity area between transcript and composer.
    if let Some(area) = activity_chunk {
        let activity_lines = build_activity_lines(app, theme);
        let activity_panel = Paragraph::new(Text::from(activity_lines));
        f.render_widget(activity_panel, area);
    }

    // Input area
    let input = Paragraph::new(Text::from(input_lines))
        .style(Style::default().bg(theme.input_bg))
        .wrap(Wrap { trim: false });
    f.render_widget(input, input_chunk);

    // Hints
    if let Some(area) = hint_chunk {
        let hint_line = build_hint_line(app, theme);
        let hint_panel = Paragraph::new(Text::from(vec![hint_line]));
        f.render_widget(hint_panel, area);
    }

    // Cursor
    if matches!(app.mode, Mode::Normal) {
        let top_padding: u16 = 1;
        let (cx, cy) =
            input_cursor_position(&app.input, app.cursor, input_chunk.width, prompt_width);
        let cursor_x = input_chunk.x + cx.min(input_chunk.width.saturating_sub(1));
        let cursor_y = input_chunk.y
            + cy.saturating_add(top_padding)
                .min(input_chunk.height.saturating_sub(1));
        f.set_cursor_position((cursor_x, cursor_y));
    }

    // Status bar: compact, only essential info
    let cancel_hint = if app.running { " | Esc cancel" } else { "" };
    let status = Paragraph::new(format!(
        " {} | {}{} | Ctrl+K cmds | Ctrl+C exit",
        app.primary_provider.as_str(),
        providers_label(&app.available_providers),
        cancel_hint,
    ))
    .style(Style::default().fg(theme.status_text));
    f.render_widget(status, status_chunk);

    if matches!(app.mode, Mode::CommandPalette) {
        draw_palette(f, app, theme);
    }
    if matches!(app.mode, Mode::HistorySearch) {
        draw_history(f, app, theme);
    }
    if matches!(app.mode, Mode::Approval) {
        draw_approval(f, app, theme);
    }
}

pub(super) fn draw_exit(f: &mut Frame, app: &App) {
    let _ = app;
    f.render_widget(Clear, f.area());
}

fn build_input_lines(
    app: &App,
    prompt_prefix: &str,
    prompt_style: Style,
    theme: ThemePalette,
) -> Vec<Line<'static>> {
    if app.input.is_empty() {
        return vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(prompt_prefix.to_string(), prompt_style),
                Span::styled(
                    "Type message. Enter send, Shift+Enter newline",
                    Style::default().fg(theme.muted_text),
                ),
            ]),
            Line::from(""),
        ];
    }

    let mut lines = vec![Line::from("")];
    let indent = " ".repeat(prompt_prefix.chars().count());
    for (idx, part) in app.input.split('\n').enumerate() {
        if idx == 0 {
            lines.push(Line::from(vec![
                Span::styled(prompt_prefix.to_string(), prompt_style),
                Span::styled(part.to_string(), Style::default().fg(theme.input_text)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(indent.clone(), prompt_style),
                Span::styled(part.to_string(), Style::default().fg(theme.input_text)),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines
}

fn build_hint_line(app: &App, theme: ThemePalette) -> Line<'static> {
    let hints = app.inline_hints();
    if hints.is_empty() {
        return Line::from(" ");
    }

    let label = if app.active_mention_span().is_some() {
        " agent suggestions (Tab cycle): "
    } else {
        " slash suggestions (Tab cycle): "
    };
    let mut spans = vec![Span::styled(label, Style::default().fg(theme.muted_text))];
    let selected = app.slash_hint_idx.min(hints.len().saturating_sub(1));
    for (i, hint) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        if i == selected {
            spans.push(Span::styled(
                hint.clone(),
                Style::default()
                    .fg(theme.highlight_fg)
                    .bg(theme.highlight_bg)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                hint.clone(),
                Style::default().fg(theme.muted_text),
            ));
        }
    }
    Line::from(spans)
}

// Breathing intensity (8 frames, cycling for ~1s period at 120ms tick).
const BREATH_SCALE_PCT: [u16; 8] = [58, 70, 82, 94, 108, 94, 82, 70];

const ACTIVITY_VERBS: &[&str] = &[
    "Thinking",
    "Pondering",
    "Ruminating",
    "Conjuring",
    "Perambulating",
    "Contemplating",
    "Synthesizing",
    "Weaving",
    "Assembling",
    "Composing",
    "Crafting",
    "Exploring",
];

fn scale_rgb(value: u8, pct: u16) -> u8 {
    ((value as u16 * pct) / 100).min(255) as u8
}

fn color_with_breath(base: Color, frame: usize) -> Color {
    let pct = BREATH_SCALE_PCT[frame % BREATH_SCALE_PCT.len()];
    match base {
        Color::Rgb(r, g, b) => Color::Rgb(scale_rgb(r, pct), scale_rgb(g, pct), scale_rgb(b, pct)),
        _ => base,
    }
}

fn spinner_base_color(provider: Provider, theme: ThemePalette) -> Color {
    match provider {
        Provider::Claude => theme.claude_label,
        Provider::Codex => theme.codex_label,
    }
}

fn format_chars(n: usize) -> String {
    if n >= 1000 {
        format!("\u{2191} {:.1}k", n as f64 / 1000.0)
    } else {
        format!("\u{2191} {}", n)
    }
}

fn build_activity_lines(app: &App, theme: ThemePalette) -> Vec<Line<'static>> {
    if app.running {
        let frame = app.spinner_idx % BREATH_SCALE_PCT.len();

        let mut lines = vec![Line::from("")];

        // Collect active agents from agent_entries, sorted deterministically.
        let mut agents: Vec<_> = app.agent_entries.keys().copied().collect();
        agents.sort_by_key(|p| match p {
            super::Provider::Claude => 0,
            super::Provider::Codex => 1,
        });

        if agents.is_empty() {
            // Fallback before any AgentStart event arrives.
            let active_provider = app.active_provider.unwrap_or(app.primary_provider);
            let active = active_provider.as_str().to_string();
            let dot_color = color_with_breath(spinner_base_color(active_provider, theme), frame);
            let elapsed_secs = app.running_elapsed_secs();
            let elapsed = format!("{:02}:{:02}", elapsed_secs / 60, elapsed_secs % 60);
            let activity = if app.last_tool_event.trim().is_empty() {
                let verb_idx = (app.spinner_idx / 64) % ACTIVITY_VERBS.len();
                ACTIVITY_VERBS[verb_idx].to_string()
            } else {
                truncate(&app.last_tool_event, 68)
            };
            let line_color = spinner_base_color(active_provider, theme);
            lines.push(Line::from(vec![
                Span::styled(
                    " \u{25cf} ",
                    Style::default()
                        .fg(dot_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {} | {} | {} ", active, activity, elapsed),
                    Style::default()
                        .fg(line_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            for &provider in &agents {
                let dot_color = color_with_breath(spinner_base_color(provider, theme), frame);
                let line_color = spinner_base_color(provider, theme);
                let elapsed_secs = app
                    .agent_started_at
                    .get(&provider)
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(0);
                let elapsed = format!("{:02}:{:02}", elapsed_secs / 60, elapsed_secs % 60);

                let initial_idx = app.agent_verb_idx.get(&provider).copied().unwrap_or(0);
                let verb_idx =
                    (initial_idx + (elapsed_secs as usize / 8)) % ACTIVITY_VERBS.len();
                let verb = ACTIVITY_VERBS[verb_idx];
                let activity = if app.last_tool_event.trim().is_empty() {
                    verb.to_string()
                } else {
                    truncate(&app.last_tool_event, 56)
                };

                let chars = app.agent_chars.get(&provider).copied().unwrap_or(0);
                let chars_str = format_chars(chars);

                lines.push(Line::from(vec![
                    Span::styled(
                        " \u{25cf} ",
                        Style::default()
                            .fg(dot_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            " {} | {} | {} | {} ",
                            provider.as_str(),
                            activity,
                            elapsed,
                            chars_str
                        ),
                        Style::default()
                            .fg(line_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
        }

        lines
    } else {
        vec![Line::from(" ")]
    }
}

fn draw_palette(f: &mut Frame, app: &App, theme: ThemePalette) {
    let area = centered_rect(70, 58, f.area());
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
            .bg(theme.panel_bg)
            .fg(theme.panel_fg)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(Clear, area);
    f.render_widget(panel, area);
}

fn draw_history(f: &mut Frame, app: &App, theme: ThemePalette) {
    let area = centered_rect(70, 58, f.area());
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
            .bg(theme.panel_bg)
            .fg(theme.panel_fg)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(Clear, area);
    f.render_widget(panel, area);
}

fn draw_approval(f: &mut Frame, app: &App, theme: ThemePalette) {
    let area = centered_rect(64, 40, f.area());
    let pending = app.approval.as_ref();
    let lines = if let Some(p) = pending {
        vec![
            Line::from(Span::styled(
                "Approval Required",
                Style::default()
                    .fg(theme.approval_title)
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
            .bg(theme.panel_bg)
            .fg(theme.approval_title)
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
