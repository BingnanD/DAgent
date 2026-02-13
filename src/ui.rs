use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::{App, Mode, Provider, ThemePalette};
use crate::{input_cursor_position, providers_label, truncate};

const PANEL_PADDING_X: u16 = 1;
const PANEL_PADDING_Y: u16 = 0;
const PANEL_HORIZONTAL_INSET: u16 = 2 + PANEL_PADDING_X * 2;
const PANEL_VERTICAL_INSET: u16 = 2 + PANEL_PADDING_Y * 2;

pub(super) fn draw(f: &mut Frame, app: &App) {
    let frame_area = f.area();
    let theme = app.theme_palette();
    let prompt_prefix = "> ";
    let prompt_width = UnicodeWidthStr::width(prompt_prefix) as u16;
    let composer_width = frame_area.width.saturating_sub(PANEL_HORIZONTAL_INSET).max(1);

    let activity_rows = if app.running {
        app.agent_entries.len().max(1) as u16
    } else {
        0
    };
    let activity_h = if activity_rows > 0 {
        activity_rows.saturating_add(PANEL_VERTICAL_INSET)
    } else {
        0
    };
    let hints_h = if app.inline_hints().is_empty() {
        0
    } else {
        1u16.saturating_add(PANEL_VERTICAL_INSET)
    };
    let status_h: u16 = 1 + PANEL_VERTICAL_INSET;
    let fixed_rows = activity_h + hints_h + status_h;
    let max_input_height = frame_area.height.saturating_sub(fixed_rows).max(3);
    let input_height = app
        .input_height(composer_width, prompt_width)
        .saturating_add(PANEL_VERTICAL_INSET)
        .min(max_input_height);
    let prompt_style = theme.prompt_style();
    let input_lines = build_input_lines(app, prompt_prefix, prompt_style, theme);

    // Composer-only viewport: transcript is appended above via insert_before.
    let mut constraints = Vec::new();
    if activity_h > 0 {
        constraints.push(Constraint::Length(activity_h));
    }
    constraints.push(Constraint::Length(input_height));
    if hints_h > 0 {
        constraints.push(Constraint::Length(hints_h));
    }
    constraints.push(Constraint::Length(status_h));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame_area);

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

    // Real-time activity area between transcript and composer.
    if let Some(area) = activity_chunk {
        let activity_lines = build_activity_lines(app, theme);
        let activity_panel = Paragraph::new(Text::from(activity_lines))
            .style(theme.panel_surface_style())
            .block(panel_block(theme, "activity"))
            .wrap(Wrap { trim: false });
        f.render_widget(activity_panel, area);
    }

    // Input area
    let input = Paragraph::new(Text::from(input_lines))
        .style(theme.input_surface_style())
        .block(panel_block(theme, "compose"))
        .wrap(Wrap { trim: false });
    f.render_widget(input, input_chunk);

    // Hints
    if let Some(area) = hint_chunk {
        let hint_line = build_hint_line(app, theme);
        let hint_panel = Paragraph::new(Text::from(vec![hint_line]))
            .style(theme.panel_surface_style())
            .block(panel_block(theme, "suggestions"));
        f.render_widget(hint_panel, area);
    }

    // Cursor
    if matches!(app.mode, Mode::Normal) {
        let content_width = input_chunk
            .width
            .saturating_sub(PANEL_HORIZONTAL_INSET)
            .max(1);
        let content_height = input_chunk
            .height
            .saturating_sub(PANEL_VERTICAL_INSET)
            .max(1);
        let (cx, cy) = input_cursor_position(&app.input, app.cursor, content_width, prompt_width);
        let cursor_x =
            input_chunk.x + 1 + PANEL_PADDING_X + cx.min(content_width.saturating_sub(1));
        let cursor_y =
            input_chunk.y + 1 + PANEL_PADDING_Y + cy.min(content_height.saturating_sub(1));
        f.set_cursor_position((cursor_x, cursor_y));
    }

    // Status bar
    let cancel_hint = if app.running { " | Esc cancel" } else { "" };
    let status = Paragraph::new(format!(
        "{} | {}{} | Ctrl+R history | Ctrl+C exit",
        app.primary_provider.as_str(),
        providers_label(&app.available_providers),
        cancel_hint,
    ))
    .style(theme.status_style())
    .block(panel_block(theme, "status"));
    f.render_widget(status, status_chunk);

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

fn panel_block(theme: ThemePalette, title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.panel_border_style())
        .title(Span::styled(format!(" {} ", title), theme.title_style()))
        .padding(Padding::new(
            PANEL_PADDING_X,
            PANEL_PADDING_X,
            PANEL_PADDING_Y,
            PANEL_PADDING_Y,
        ))
        .style(theme.panel_surface_style())
}

fn modal_block(theme: ThemePalette, title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.panel_border_style())
        .title(Span::styled(format!(" {} ", title), theme.title_style()))
        .padding(Padding::new(1, 1, 0, 0))
        .style(theme.panel_surface_style())
}

fn build_input_lines(
    app: &App,
    prompt_prefix: &str,
    prompt_style: Style,
    theme: ThemePalette,
) -> Vec<Line<'static>> {
    if app.input.is_empty() {
        return vec![Line::from(vec![
            Span::styled(prompt_prefix.to_string(), prompt_style),
            Span::styled(
                "Type message. Enter send, Shift+Enter newline",
                theme.muted_style(),
            ),
        ])];
    }

    let mut lines = Vec::new();
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
    let mut spans = vec![Span::styled(label, theme.muted_style())];
    let selected = app.slash_hint_idx.min(hints.len().saturating_sub(1));
    for (i, hint) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        if i == selected {
            spans.push(Span::styled(hint.clone(), theme.hint_selected_style()));
        } else {
            spans.push(Span::styled(hint.clone(), theme.muted_style()));
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

        let mut lines = Vec::new();

        // Collect active agents from agent_entries, sorted deterministically.
        let mut agents: Vec<_> = app.agent_entries.keys().copied().collect();
        agents.sort_by_key(|p| match p {
            super::Provider::Claude => 0,
            super::Provider::Codex => 1,
        });

        // Fixed-width label so spinner lines stay aligned across providers.
        let label_width = super::Provider::all()
            .iter()
            .map(|p| p.as_str().len())
            .max()
            .unwrap_or(6);

        if agents.is_empty() {
            // Fallback before any AgentStart event arrives.
            // Always use primary_provider here so the spinner label matches
            // the provider the user selected (active_provider may have been
            // overwritten by an AgentStart event from a secondary agent).
            let active_provider = app.primary_provider;
            let active = format!("{:width$}", active_provider.as_str(), width = label_width);
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
                let agent_event = app.agent_tool_event.get(&provider).map(|s| s.as_str()).unwrap_or("");
                let activity = if agent_event.trim().is_empty() {
                    verb.to_string()
                } else {
                    truncate(agent_event, 56)
                };

                let chars = app.agent_chars.get(&provider).copied().unwrap_or(0);
                let chars_str = format_chars(chars);
                let padded_name = format!("{:width$}", provider.as_str(), width = label_width);

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
                            padded_name,
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
        Vec::new()
    }
}

fn draw_history(f: &mut Frame, app: &App, theme: ThemePalette) {
    let area = centered_rect(70, 58, f.area());
    let items = app.filtered_history();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("query: ", theme.muted_style()),
            Span::styled(app.history_query.clone(), theme.secondary_style()),
        ]),
        Line::from(""),
    ];
    if items.is_empty() {
        lines.push(Line::from(Span::styled("(no match)", theme.muted_style())));
    } else {
        for (i, item) in items.iter().enumerate() {
            if i == app.history_idx {
                lines.push(Line::from(Span::styled(
                    format!("> {}", item),
                    theme.hint_selected_style(),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("  {}", item),
                    theme.body_style(),
                )));
            }
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter apply | Esc close",
        theme.muted_style(),
    )));

    let panel = Paragraph::new(lines)
        .style(theme.panel_surface_style())
        .block(modal_block(theme, "history search"))
        .wrap(Wrap { trim: false });
    f.render_widget(Clear, area);
    f.render_widget(panel, area);
}

fn draw_approval(f: &mut Frame, app: &App, theme: ThemePalette) {
    let area = centered_rect(64, 40, f.area());
    let pending = app.approval.as_ref();
    let lines = if let Some(p) = pending {
        vec![
            Line::from(vec![
                Span::styled("tool: ", theme.muted_style()),
                Span::styled(
                    p.tool.clone(),
                    Style::default()
                        .fg(theme.approval_title)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(p.reason.clone(), theme.body_style())),
            Line::from(""),
            Line::from(vec![
                Span::styled("cmd: ", theme.muted_style()),
                Span::styled(truncate(&p.line, 90), theme.secondary_style()),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "[y] approve once   [a] always allow   [n] deny",
                theme.muted_style(),
            )),
        ]
    } else {
        vec![Line::from(Span::styled(
            "No pending approval",
            theme.muted_style(),
        ))]
    };
    let panel = Paragraph::new(lines)
        .style(theme.panel_surface_style())
        .block(modal_block(theme, "approval required"))
        .wrap(Wrap { trim: false });
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
