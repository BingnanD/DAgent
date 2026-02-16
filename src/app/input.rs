use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    pub(super) fn handle_paste_event(&mut self, raw: &str) {
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

    pub(super) fn consume_pending_pastes(&mut self, text: &str) -> String {
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

    pub(super) fn clear_input_buffer(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.pending_pastes.clear();
    }

    pub(super) fn filtered_history(&self) -> Vec<String> {
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

    pub(super) fn slash_hints(&self) -> Vec<String> {
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

    pub(super) fn mention_query(token: &str) -> Option<String> {
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

    pub(super) fn active_mention_span(&self) -> Option<(usize, usize, String)> {
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

    pub(super) fn agent_hints(&self) -> Vec<String> {
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

    pub(super) fn agent_hint_options(&self) -> Vec<String> {
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

    pub(super) fn inline_hints(&self) -> Vec<String> {
        if self.input.starts_with("/") {
            self.slash_hints()
        } else if self.active_mention_span().is_some() {
            self.agent_hints()
        } else {
            Vec::new()
        }
    }

    pub(super) fn apply_selected_inline_hint(&mut self) -> bool {
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
                self.input.insert(cursor, ' ');
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
    pub(super) fn sync_inline_hint_idx(&mut self) {
        let len = self.inline_hints().len();
        if len == 0 {
            self.slash_hint_idx = 0;
            return;
        }
        if self.slash_hint_idx >= len {
            self.slash_hint_idx = len - 1;
        }
    }

    pub(super) fn cycle_inline_hint_next(&mut self) -> bool {
        let len = self.inline_hints().len();
        if len == 0 {
            return false;
        }
        self.slash_hint_idx = (self.slash_hint_idx + 1) % len;
        true
    }

    pub(super) fn cycle_inline_hint_prev(&mut self) -> bool {
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

    pub(super) fn history_prev(&mut self) {
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

    pub(super) fn history_next(&mut self) {
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

    pub(super) fn insert_char(&mut self, c: char) {
        if self.cursor >= self.input.len() {
            self.input.push(c);
        } else {
            self.input.insert(self.cursor, c);
        }
        self.cursor += c.len_utf8();
    }

    pub(super) fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
        if self.input.starts_with("/") || self.active_mention_span().is_some() {
            self.slash_hint_idx = 0;
            self.sync_inline_hint_idx();
        }
    }

    pub(super) fn backspace(&mut self) {
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

    pub(super) fn backspace_word(&mut self) {
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

    pub(super) fn delete(&mut self) {
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

    pub(super) fn move_left(&mut self) {
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

    pub(super) fn move_right(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let mut iter = self.input[self.cursor..].char_indices();
        if let Some((_, ch)) = iter.next() {
            self.cursor += ch.len_utf8();
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Approval => self.handle_approval_key(key),
            Mode::HistorySearch => self.handle_history_key(key),
            Mode::Normal => self.handle_normal_key(key),
        }
    }

    pub(super) fn handle_approval_key(&mut self, key: KeyEvent) {
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

    pub(super) fn handle_history_key(&mut self, key: KeyEvent) {
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

    pub(super) fn handle_normal_key(&mut self, key: KeyEvent) {
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
                    self.cycle_inline_hint_prev();
                } else {
                    self.cycle_inline_hint_next();
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
                            if entry.text.contains(WORKING_PLACEHOLDER) {
                                entry.text =
                                    entry.text.replacen(WORKING_PLACEHOLDER, "(cancelled)", 1);
                            }
                        }
                    }
                    if let Some(idx) = self.assistant_idx {
                        if let Some(entry) = self.entries.get_mut(idx) {
                            if entry.text.trim() == WORKING_PLACEHOLDER {
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
}
