use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

const RECENT_LIMIT: usize = 16;
const SEARCH_LIMIT: usize = 8;
const CONTEXT_CHAR_LIMIT: usize = 6000;
const MAX_LINE_CHARS: usize = 500;

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
    for t in input
        .split(|c: char| !c.is_alphanumeric())
        .map(|s| s.trim().to_lowercase())
    {
        if t.len() < 2 {
            continue;
        }
        if terms.contains(&t) {
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

fn format_line(item: &MemoryMessage) -> Option<String> {
    let text = squash_whitespace(item.content.trim());
    if text.is_empty() {
        return None;
    }

    let mut clipped = text;
    if clipped.chars().count() > MAX_LINE_CHARS {
        clipped = clipped.chars().take(MAX_LINE_CHARS).collect::<String>();
        clipped.push_str("...");
    }

    match item.role.as_str() {
        "user" => Some(format!("user: {clipped}")),
        "assistant" => {
            if let Some(agent) = &item.agent {
                Some(format!("assistant({agent}): {clipped}"))
            } else {
                Some(format!("assistant: {clipped}"))
            }
        }
        "system" => Some(format!("system: {clipped}")),
        _ => Some(format!("{}: {clipped}", item.role)),
    }
}
