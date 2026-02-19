use super::*;

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

fn push_system_lines(lines: &mut Vec<Line<'static>>, text: &str, style: Style) {
    let mut parts = text.split('\n');
    let first = parts.next().unwrap_or_default();
    let first_content = if first.is_empty() { " " } else { first };
    lines.push(Line::from(vec![Span::styled(
        format!("[sys] {first_content}"),
        style,
    )]));

    for part in parts {
        let content = if part.is_empty() { " " } else { part };
        lines.push(Line::from(vec![Span::styled(
            format!("      {content}"),
            style,
        )]));
    }
}

impl App {
    pub(super) fn render_entries_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.render_entries_lines_range(width, 0, self.entries.len())
    }

    pub(super) fn render_entries_lines_range(
        &self,
        width: u16,
        start: usize,
        end: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::<Line>::new();
        let palette = self.theme.palette();
        let active_entry_indices: HashSet<usize> = self.agent_entries.values().copied().collect();
        let max_label_width = Provider::all()
            .iter()
            .map(|p| UnicodeWidthStr::width(p.as_str()))
            .max()
            .unwrap_or(6);

        for (idx, entry) in self.entries[start..end]
            .iter()
            .enumerate()
            .map(|(i, e)| (i + start, e))
        {
            let line_count_before_entry = lines.len();
            let entry_provider =
                extract_agent_name(&entry.text).and_then(|n| provider_from_name(&n));
            let is_current_entry =
                self.assistant_idx == Some(idx) || active_entry_indices.contains(&idx);
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
                    let label_style = Style::default()
                        .fg(provider_color)
                        .add_modifier(Modifier::BOLD);
                    let cleaned_text = cleaned_assistant_text(entry);
                    let raw_text = if cleaned_text.trim().is_empty() {
                        if entry.text.contains(WORKING_PLACEHOLDER) {
                            WORKING_PLACEHOLDER.to_string()
                        } else {
                            String::new()
                        }
                    } else {
                        cleaned_text
                    };
                    let base_style = palette.body_style();

                    // Label column: agent name occupies a fixed-width column.
                    // Continuation and wrapped lines are indented to keep content
                    // aligned and prevent text from invading the label column.
                    // Use the longest provider name to keep all labels the same
                    // width so that content columns stay aligned across agents.
                    let label_col_width = max_label_width + 2; // "label" + " |"
                    let padded_label = format!("{:width$}", label, width = max_label_width);
                    let label_sep = format!("{} {}", padded_label, ASSISTANT_DIVIDER);
                    let indent = " ".repeat(label_col_width.saturating_sub(1));
                    let indent_sep = format!("{}{}", indent, ASSISTANT_DIVIDER); // "       |"
                    let content_width = (width as usize).saturating_sub(label_col_width + 1); // +1 for space after divider

                    if raw_text.is_empty() {
                        if !(self.running && is_current_entry) {
                            lines.push(Line::from(vec![Span::styled(
                                label_sep.clone(),
                                label_style,
                            )]));
                        }
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
                                    // Use the same label color for the | so it
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
                                StartupBannerRow::Title(value) => {
                                    (format!(" {value}"), palette.title_style())
                                }
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
                            StartupBannerRow::Title(value) => {
                                (value.to_string(), palette.title_style())
                            }
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
                    push_system_lines(&mut lines, &entry.text, palette.secondary_style());
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
            if lines.len() == line_count_before_entry {
                continue;
            }

            let next_idx = idx + 1;
            let next_entry = if next_idx < end {
                self.entries.get(next_idx)
            } else {
                None
            };
            let should_connect_assistant_blocks = matches!(entry.kind, EntryKind::Assistant)
                && next_entry.is_some_and(|next| matches!(next.kind, EntryKind::Assistant))
                && next_entry.is_some_and(|next| {
                    let current_provider = entry_provider.unwrap_or(self.primary_provider);
                    let next_provider = extract_agent_name(&next.text)
                        .and_then(|n| provider_from_name(&n))
                        .unwrap_or(self.primary_provider);
                    current_provider == next_provider
                });

            let is_running_stream_entry =
                self.running && matches!(entry.kind, EntryKind::Assistant) && is_current_entry;
            if should_connect_assistant_blocks {
                let provider = entry_provider.unwrap_or(self.primary_provider);
                let provider_color = match provider {
                    Provider::Claude => palette.claude_label,
                    Provider::Codex => palette.codex_label,
                };
                let label_style = Style::default()
                    .fg(provider_color)
                    .add_modifier(Modifier::BOLD);
                let label_col_width = max_label_width + 2;
                let indent = " ".repeat(label_col_width.saturating_sub(1));
                let indent_sep = format!("{}{}", indent, ASSISTANT_DIVIDER);
                lines.push(Line::from(vec![Span::styled(indent_sep, label_style)]));
            } else if !is_running_stream_entry {
                lines.push(Line::from(""));
            }
        }

        lines
    }

    /// Render all entries except those at the given indices (streaming assistant entries).
    pub(super) fn render_entries_lines_filtered(
        &self,
        width: u16,
        skip_indices: &std::collections::HashSet<usize>,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::<Line>::new();
        let palette = self.theme.palette();
        let active_entry_indices: HashSet<usize> = self.agent_entries.values().copied().collect();
        let max_label_width = Provider::all()
            .iter()
            .map(|p| UnicodeWidthStr::width(p.as_str()))
            .max()
            .unwrap_or(6);

        for (idx, entry) in self.entries.iter().enumerate() {
            if skip_indices.contains(&idx) {
                continue;
            }
            let line_count_before_entry = lines.len();
            let entry_provider =
                extract_agent_name(&entry.text).and_then(|n| provider_from_name(&n));
            let is_current_entry =
                self.assistant_idx == Some(idx) || active_entry_indices.contains(&idx);
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
                    let label_style = Style::default()
                        .fg(provider_color)
                        .add_modifier(Modifier::BOLD);
                    let cleaned_text = cleaned_assistant_text(entry);
                    let raw_text = if cleaned_text.trim().is_empty() {
                        if entry.text.contains(WORKING_PLACEHOLDER) {
                            WORKING_PLACEHOLDER.to_string()
                        } else {
                            String::new()
                        }
                    } else {
                        cleaned_text
                    };
                    let base_style = palette.body_style();
                    let label_col_width = max_label_width + 2;
                    let padded_label = format!("{:width$}", label, width = max_label_width);
                    let label_sep = format!("{} {}", padded_label, ASSISTANT_DIVIDER);
                    let indent = " ".repeat(label_col_width.saturating_sub(1));
                    let indent_sep = format!("{}{}", indent, ASSISTANT_DIVIDER);
                    let content_width = (width as usize).saturating_sub(label_col_width + 1);
                    if raw_text.is_empty() {
                        if !(self.running && is_current_entry) {
                            lines.push(Line::from(vec![Span::styled(
                                label_sep.clone(),
                                label_style,
                            )]));
                        }
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
                        let next_non_skipped =
                            (idx + 1..self.entries.len()).find(|i| !skip_indices.contains(i));
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
                                StartupBannerRow::Title(value) => {
                                    (format!(" {value}"), palette.title_style())
                                }
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
                            StartupBannerRow::Title(value) => {
                                (value.to_string(), palette.title_style())
                            }
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
                    push_system_lines(&mut lines, &entry.text, palette.secondary_style());
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
            if lines.len() == line_count_before_entry {
                continue;
            }

            let next_non_skipped =
                (idx + 1..self.entries.len()).find(|i| !skip_indices.contains(i));
            let should_connect_assistant_blocks = matches!(entry.kind, EntryKind::Assistant)
                && next_non_skipped
                    .and_then(|i| self.entries.get(i))
                    .is_some_and(|next| matches!(next.kind, EntryKind::Assistant))
                && next_non_skipped
                    .and_then(|i| self.entries.get(i))
                    .is_some_and(|next| {
                        let current_provider = entry_provider.unwrap_or(self.primary_provider);
                        let next_provider = extract_agent_name(&next.text)
                            .and_then(|n| provider_from_name(&n))
                            .unwrap_or(self.primary_provider);
                        current_provider == next_provider
                    });

            let is_running_stream_entry =
                self.running && matches!(entry.kind, EntryKind::Assistant) && is_current_entry;
            if should_connect_assistant_blocks {
                let provider = entry_provider.unwrap_or(self.primary_provider);
                let provider_color = match provider {
                    Provider::Claude => palette.claude_label,
                    Provider::Codex => palette.codex_label,
                };
                let label_style = Style::default()
                    .fg(provider_color)
                    .add_modifier(Modifier::BOLD);
                let label_col_width = max_label_width + 2;
                let indent = " ".repeat(label_col_width.saturating_sub(1));
                let indent_sep = format!("{}{}", indent, ASSISTANT_DIVIDER);
                lines.push(Line::from(vec![Span::styled(indent_sep, label_style)]));
            } else if !is_running_stream_entry {
                lines.push(Line::from(""));
            }
        }

        lines
    }

    pub(super) fn render_log_lines_inner(&self, width: u16) -> Vec<Line<'static>> {
        self.render_entries_lines(width)
    }

    /// Render only the currently-active streaming entries for the live TUI area.
    /// Returns empty when not running or no real content has arrived yet.
    pub(super) fn render_active_streaming_lines(&self, width: u16) -> Vec<Line<'static>> {
        if !self.running {
            return Vec::new();
        }

        let active_set: HashSet<usize> = {
            let mut s: HashSet<usize> = self.agent_entries.values().copied().collect();
            if let Some(idx) = self.assistant_idx {
                s.insert(idx);
            }
            s
        };

        if active_set.is_empty() {
            return Vec::new();
        }

        // Only show once at least one active entry has real (non-placeholder) content.
        let has_content = active_set.iter().any(|&idx| {
            self.entries.get(idx).is_some_and(|entry| {
                matches!(entry.kind, EntryKind::Assistant)
                    && !cleaned_assistant_text(entry).trim().is_empty()
            })
        });
        if !has_content {
            return Vec::new();
        }

        // Render only the active entries by skipping everything else.
        let skip: HashSet<usize> = (0..self.entries.len())
            .filter(|i| !active_set.contains(i))
            .collect();
        self.render_entries_lines_filtered(width, &skip)
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
                result.push(vec![Span::styled("───".to_string(), palette.muted_style())]);
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
                    Span::styled(format!("{} ", prefix), palette.muted_style()),
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
