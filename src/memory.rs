use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

const RECENT_LIMIT: usize = 2;
const SEARCH_LIMIT: usize = 8;
const CONTEXT_CHAR_LIMIT: usize = 2000;
const MAX_LINE_CHARS: usize = 500;
const MAX_PREVIEW_CHARS: usize = 180;

#[derive(Debug, Clone)]
struct MemoryMessage {
    id: i64,
    role: String,
    agent: Option<String>,
    content: String,
}

pub(crate) struct MemoryStore {
    conn: Connection,
}

impl MemoryStore {
    pub(crate) fn open_default() -> Result<Self> {
        let path = memory_file_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create memory dir {}", parent.display()))?;
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("open memory db {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS messages (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              session_id TEXT NOT NULL,
              role TEXT NOT NULL,
              agent TEXT,
              content TEXT NOT NULL,
              created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session_id_id
              ON messages(session_id, id);
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts
              USING fts5(content, tokenize='unicode61');
            ",
        )
        .context("init memory schema")?;

        Ok(Self { conn })
    }

    pub(crate) fn append_message(
        &self,
        session_id: &str,
        role: &str,
        agent: Option<&str>,
        content: &str,
    ) -> Result<()> {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        self.conn
            .execute(
                "INSERT INTO messages(session_id, role, agent, content) VALUES (?1, ?2, ?3, ?4)",
                params![session_id, role, agent, trimmed],
            )
            .context("insert message")?;

        let msg_id = self.conn.last_insert_rowid();
        self.conn
            .execute(
                "INSERT INTO messages_fts(rowid, content) VALUES (?1, ?2)",
                params![msg_id, trimmed],
            )
            .context("insert fts row")?;
        Ok(())
    }

    pub(crate) fn clear_session(&self, session_id: &str) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("begin clear tx")?;
        tx.execute(
            "DELETE FROM messages_fts
             WHERE rowid IN (SELECT id FROM messages WHERE session_id = ?1)",
            params![session_id],
        )
        .context("clear fts rows")?;
        tx.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            params![session_id],
        )
        .context("clear message rows")?;
        tx.commit().context("commit clear tx")?;
        Ok(())
    }

    pub(crate) fn session_message_count(&self, session_id: &str) -> Result<usize> {
        let count = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                params![session_id],
                |row| row.get::<_, i64>(0),
            )
            .context("count session messages")?;
        Ok(count.max(0) as usize)
    }

    pub(crate) fn list_session_lines(&self, session_id: &str, limit: usize) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for item in self.recent_messages(session_id, limit.max(1))? {
            if let Some(line) = format_preview_line(&item) {
                out.push(format!("#{} {}", item.id, line));
            }
        }
        Ok(out)
    }

    pub(crate) fn search_session_lines(
        &self,
        session_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let Some(normalized) = normalize_query(query) else {
            return Ok(Vec::new());
        };

        let mut items = self.search_messages(session_id, &normalized, limit.max(1))?;
        items.sort_by_key(|m| m.id);

        let mut out = Vec::new();
        for item in items {
            if let Some(line) = format_preview_line(&item) {
                out.push(format!("#{} {}", item.id, line));
            }
        }
        Ok(out)
    }

    pub(crate) fn prune_session_keep_recent(&self, session_id: &str, keep: usize) -> Result<usize> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("begin prune tx")?;

        let deleted_rows = if keep == 0 {
            tx.execute(
                "DELETE FROM messages_fts
                 WHERE rowid IN (SELECT id FROM messages WHERE session_id = ?1)",
                params![session_id],
            )
            .context("prune clear fts rows")?;
            tx.execute(
                "DELETE FROM messages WHERE session_id = ?1",
                params![session_id],
            )
            .context("prune clear message rows")?
        } else {
            let cutoff_id = tx
                .query_row(
                    "SELECT id
                     FROM messages
                     WHERE session_id = ?1
                     ORDER BY id DESC
                     LIMIT 1 OFFSET ?2",
                    params![session_id, (keep - 1) as i64],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .context("query prune cutoff")?;

            let Some(cutoff_id) = cutoff_id else {
                tx.commit().context("commit prune tx")?;
                return Ok(0);
            };

            tx.execute(
                "DELETE FROM messages_fts
                 WHERE rowid IN (
                   SELECT id FROM messages
                   WHERE session_id = ?1 AND id < ?2
                 )",
                params![session_id, cutoff_id],
            )
            .context("prune fts rows")?;
            tx.execute(
                "DELETE FROM messages
                 WHERE session_id = ?1 AND id < ?2",
                params![session_id, cutoff_id],
            )
            .context("prune message rows")?
        };

        tx.commit().context("commit prune tx")?;
        Ok(deleted_rows)
    }

    pub(crate) fn build_context(&self, session_id: &str, prompt: &str) -> Result<String> {
        let mut items = self.recent_messages(session_id, RECENT_LIMIT)?;
        let mut seen = items.iter().map(|m| m.id).collect::<HashSet<_>>();

        if let Some(query) = normalize_query(prompt) {
            for hit in self.search_messages(session_id, &query, SEARCH_LIMIT)? {
                if seen.insert(hit.id) {
                    items.push(hit);
                }
            }
        }

        if items.is_empty() {
            return Ok(prompt.to_string());
        }

        items.sort_by_key(|m| m.id);

        let prompt_norm = squash_whitespace(prompt.trim());
        let mut filtered = Vec::new();
        let mut skipped_current_prompt = false;
        for item in items.into_iter().rev() {
            let text_norm = squash_whitespace(item.content.trim());
            if !skipped_current_prompt
                && item.role == "user"
                && !prompt_norm.is_empty()
                && text_norm == prompt_norm
            {
                skipped_current_prompt = true;
                continue;
            }
            filtered.push(item);
        }
        filtered.reverse();

        let mut lines = Vec::new();
        for item in filtered {
            if let Some(line) = format_line(&item) {
                lines.push(line);
            }
        }

        if lines.is_empty() {
            return Ok(prompt.to_string());
        }

        let mut selected = Vec::new();
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

        Ok(format!(
            "Shared session memory:\n{}\n\nCurrent user request:\n{}",
            selected.join("\n"),
            prompt
        ))
    }

    fn recent_messages(&self, session_id: &str, limit: usize) -> Result<Vec<MemoryMessage>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, role, agent, content
                 FROM messages
                 WHERE session_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .context("prepare recent messages")?;

        let mut rows = stmt
            .query(params![session_id, limit as i64])
            .context("query recent messages")?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().context("scan recent row")? {
            out.push(MemoryMessage {
                id: row.get(0).context("recent.id")?,
                role: row.get(1).context("recent.role")?,
                agent: row.get(2).context("recent.agent")?,
                content: row.get(3).context("recent.content")?,
            });
        }
        out.reverse();
        Ok(out)
    }

    fn search_messages(
        &self,
        session_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryMessage>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT m.id, m.role, m.agent, m.content
                 FROM messages_fts f
                 JOIN messages m ON m.id = f.rowid
                 WHERE f.content MATCH ?1 AND m.session_id = ?2
                 ORDER BY bm25(messages_fts), m.id DESC
                 LIMIT ?3",
            )
            .context("prepare search messages")?;

        let mut rows = stmt
            .query(params![query, session_id, limit as i64])
            .context("query search messages")?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().context("scan search row")? {
            out.push(MemoryMessage {
                id: row.get(0).context("search.id")?,
                role: row.get(1).context("search.role")?,
                agent: row.get(2).context("search.agent")?,
                content: row.get(3).context("search.content")?,
            });
        }
        Ok(out)
    }
}

fn memory_file_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".dagent").join("memory.db")
    } else {
        PathBuf::from(".dagent").join("memory.db")
    }
}

fn normalize_query(input: &str) -> Option<String> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    for t in input
        .split(|c: char| !c.is_alphanumeric())
        .map(|s| s.trim().to_lowercase())
    {
        if t.len() < 2 {
            continue;
        }
        if !seen.insert(t.clone()) {
            continue;
        }
        terms.push(t);
        if terms.len() >= 8 {
            break;
        }
    }

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

fn squash_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn squash_block_whitespace(text: &str) -> String {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let line = squash_whitespace(raw.trim());
        if !line.is_empty() {
            lines.push(line);
        }
    }

    if lines.is_empty() {
        squash_whitespace(text.trim())
    } else {
        lines.join("\n")
    }
}

fn clip_chars(mut text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    text = text.chars().take(limit).collect::<String>();
    text.push_str("...");
    text
}

fn prefix_multiline(prefix: &str, text: &str) -> String {
    let mut out = String::new();
    let mut lines = text.split('\n');
    let first = lines.next().unwrap_or_default();
    out.push_str(prefix);
    out.push_str(first);

    let indent = " ".repeat(prefix.chars().count());
    for line in lines {
        out.push('\n');
        out.push_str(&indent);
        out.push_str(line);
    }
    out
}

fn message_prefix(item: &MemoryMessage) -> String {
    match item.role.as_str() {
        "user" => "user: ".to_string(),
        "assistant" => {
            if let Some(agent) = &item.agent {
                format!("assistant({agent}): ")
            } else {
                "assistant: ".to_string()
            }
        }
        "system" => "system: ".to_string(),
        _ => format!("{}: ", item.role),
    }
}

fn format_line(item: &MemoryMessage) -> Option<String> {
    let text = squash_block_whitespace(item.content.trim());
    if text.is_empty() {
        return None;
    }

    let clipped = clip_chars(text, MAX_LINE_CHARS);
    let prefix = message_prefix(item);
    Some(prefix_multiline(&prefix, &clipped))
}

fn format_preview_line(item: &MemoryMessage) -> Option<String> {
    let block = squash_block_whitespace(item.content.trim());
    if block.is_empty() {
        return None;
    }

    let compact = squash_whitespace(&block.replace('\n', " "));
    if compact.is_empty() {
        return None;
    }

    let clipped = clip_chars(compact, MAX_PREVIEW_CHARS);
    Some(format!("{}{}", message_prefix(item), clipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_line_keeps_block_structure_with_indent() {
        let item = MemoryMessage {
            id: 1,
            role: "assistant".to_string(),
            agent: Some("codex".to_string()),
            content: "first line\n\n  second   line ".to_string(),
        };

        let line = format_line(&item).expect("line should exist");
        assert_eq!(
            line,
            "assistant(codex): first line\n                  second line"
        );
    }

    #[test]
    fn format_line_clips_and_marks_ellipsis() {
        let long = "x".repeat(MAX_LINE_CHARS + 5);
        let item = MemoryMessage {
            id: 2,
            role: "user".to_string(),
            agent: None,
            content: long,
        };

        let line = format_line(&item).expect("line should exist");
        assert!(line.starts_with("user: "));
        assert!(line.ends_with("..."));
    }

    #[test]
    fn format_preview_line_flattens_block_and_truncates() {
        let item = MemoryMessage {
            id: 3,
            role: "assistant".to_string(),
            agent: Some("claude".to_string()),
            content: "first line\n\n  second   line".to_string(),
        };

        let line = format_preview_line(&item).expect("line should exist");
        assert_eq!(line, "assistant(claude): first line second line");

        let long = MemoryMessage {
            id: 4,
            role: "user".to_string(),
            agent: None,
            content: "x".repeat(MAX_PREVIEW_CHARS + 8),
        };
        let clipped = format_preview_line(&long).expect("line should exist");
        assert!(clipped.starts_with("user: "));
        assert!(clipped.ends_with("..."));
    }
}
