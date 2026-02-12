use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::{App, Mode, ThemePalette};
use crate::{input_cursor_position, providers_label, truncate, SPINNER};

pub(super) fn draw(f: &mut Frame, app: &App) {
    let theme = app.theme_palette();
    let prompt_prefix = "> ";
    let prompt_width = UnicodeWidthStr::width(prompt_prefix) as u16;
    let max_input_height = f.size().height.saturating_sub(2).max(1);
    let input_height = app
        .input_height(f.size().width.max(1), prompt_width)
        .saturating_add(2)
        .min(max_input_height);
    let prompt_style = Style::default()
        .fg(theme.prompt)
        .add_modifier(Modifier::BOLD);
    let input_lines = build_input_lines(app, prompt_prefix, prompt_style, theme);
    let activity_h: u16 = if app.running { 1 } else { 0 };
    let hints_h: u16 = if app.inline_hints().is_empty() { 0 } else { 1 };
    // Composer-only viewport: transcript is appended above via insert_before.
    let mut constraints = Vec::new();
    if activity_h > 0 {
        constraints.push(Constraint::Length(activity_h));
    }
    constraints.push(Constraint::Length(input_height));
    if hints_h > 0 {
        constraints.push(Constraint::Length(hints_h));
    }
    constraints.push(Constraint::Length(1));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.size());

    let mut section_idx = 0usize;
    let activity_chunk = if activity_h > 0 {
        let c = chunks[section_idx];
        section_idx += 1;
        Some(c)
    } else {
        None
    };
    let input_chunk = chunks[section_idx];
    section_idx += 1;
    let hint_chunk = if hints_h > 0 {
        let c = chunks[section_idx];
        section_idx += 1;
        Some(c)
    } else {
        None
    };
    let status_chunk = chunks[section_idx];

    // Real-time activity line between transcript and composer.
    if let Some(area) = activity_chunk {
        let activity_line = build_activity_line(app, theme);
        let activity_panel = Paragraph::new(Text::from(vec![activity_line]));
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
        f.set_cursor(cursor_x, cursor_y);
    }

    let mode = match app.mode {
        Mode::Normal => "normal",
        Mode::CommandPalette => "command",
        Mode::HistorySearch => "history",
        Mode::Approval => "approval",
    };
    let run_state = if app.running {
        format!("running {}", SPINNER[app.spinner_idx])
    } else {
        "idle".to_string()
    };
    let transcript_lines = app.cached_log_lines().len();
    // Status bar (single merged bar)
    let status = Paragraph::new(format!(
        " mode:{} | theme:{} | primary:{} | agents:{} | events:{} | {} | lines:{} | {} | Esc {} | Ctrl+K cmds | Ctrl+R history | Ctrl+C exit",
        mode,
        app.theme_name(),
        app.primary_provider.as_str(),
        providers_label(&app.available_providers),
        if app.show_tool_events { "on" } else { "off" },
        run_state,
        transcript_lines,
        truncate(&app.last_status, 28),
        if app.running { "cancel" } else { "-" },
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
    f.render_widget(Clear, f.size());
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

fn build_activity_line(app: &App, theme: ThemePalette) -> Line<'static> {
    if !app.running {
        return Line::from(" ");
    }

    let active = app
        .active_provider
        .map(|provider| provider.as_str().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| {
            if app.run_target.trim().is_empty() {
                "agent".to_string()
            } else {
                app.run_target.clone()
            }
        });
    let elapsed_secs = app.running_elapsed_secs();
    let elapsed = format!("{:02}:{:02}", elapsed_secs / 60, elapsed_secs % 60);
    let tool_hint = if app.last_tool_event.trim().is_empty() {
        "no tool call yet".to_string()
    } else {
        truncate(&app.last_tool_event, 48)
    };

    Line::from(vec![
        Span::styled(
            format!(" {} ", SPINNER[app.spinner_idx]),
            Style::default()
                .fg(theme.activity_badge_fg)
                .bg(theme.activity_badge_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " {} | elapsed {} | {} | {} ",
                active,
                elapsed,
                truncate(&app.last_status, 32),
                tool_hint
            ),
            Style::default()
                .fg(theme.activity_text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Esc cancel", Style::default().fg(theme.muted_text)),
    ])
}

fn draw_palette(f: &mut Frame, app: &App, theme: ThemePalette) {
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
            .bg(theme.panel_bg)
            .fg(theme.panel_fg)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(Clear, area);
    f.render_widget(panel, area);
}

fn draw_history(f: &mut Frame, app: &App, theme: ThemePalette) {
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
            .bg(theme.panel_bg)
            .fg(theme.panel_fg)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(Clear, area);
    f.render_widget(panel, area);
}

fn draw_approval(f: &mut Frame, app: &App, theme: ThemePalette) {
    let area = centered_rect(64, 40, f.size());
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
