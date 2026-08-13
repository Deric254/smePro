//! Persisted AI chat history.
//!
//! Before this, the AI panel's conversation lived only in React state —
//! closing the panel or restarting the app threw it away, and there was
//! no way to ever have more than one conversation at a time. This gives
//! the assistant real sessions: a business+user's chats are rows in
//! `ai_chat_sessions` / `ai_chat_messages` (see db_migrations.rs v9),
//! survive restarts, and can be listed, resumed, cleared, deleted, and
//! exported like any other real business data.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use rust_xlsxwriter::{Format, Workbook};
use serde_json::{json, Value};
use uuid::Uuid;

/// Creates a new, empty session and returns its id. Title starts as a
/// placeholder ("New chat") and is filled in for real from the first
/// question once one is actually asked (see the title-derivation step
/// inside `record_turn` below) — nothing here guesses at a title from
/// no content.
pub fn create_session(conn: &Connection, business_id: &str, user_id: &str) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO ai_chat_sessions (id, business_id, user_id, title, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'New chat', datetime('now'), datetime('now'))",
        params![id, business_id, user_id],
    )?;
    Ok(id)
}

/// Lists this user's own sessions for this business, most-recently-
/// active first, each with a preview of its last message so a history
/// sidebar can render without a second round-trip per session.
pub fn list_sessions(conn: &Connection, business_id: &str, user_id: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.title, s.created_at, s.updated_at,
                (SELECT content FROM ai_chat_messages m WHERE m.session_id = s.id ORDER BY m.created_at DESC LIMIT 1) AS last_message,
                (SELECT COUNT(*) FROM ai_chat_messages m WHERE m.session_id = s.id) AS message_count
         FROM ai_chat_sessions s
         WHERE s.business_id = ?1 AND s.user_id = ?2
         ORDER BY s.updated_at DESC",
    )?;
    let rows = stmt.query_map(params![business_id, user_id], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "title": r.get::<_, String>(1)?,
            "created_at": r.get::<_, String>(2)?,
            "updated_at": r.get::<_, String>(3)?,
            "last_message": r.get::<_, Option<String>>(4)?,
            "message_count": r.get::<_, i64>(5)?,
        }))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Confirms a session belongs to this business+user before any read or
/// write touches it — every function below calls this first. Without
/// it, a session id alone (guessable-ish as a UUID, but still) would
/// let one user read or delete another user's private conversation,
/// or one business reach into another's.
fn assert_owns_session(conn: &Connection, business_id: &str, user_id: &str, session_id: &str) -> Result<()> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM ai_chat_sessions WHERE id = ?1 AND business_id = ?2 AND user_id = ?3",
            params![session_id, business_id, user_id],
            |r| r.get(0),
        )
        .optional()?;
    match exists {
        Some(_) => Ok(()),
        None => Err(anyhow!("chat session not found")),
    }
}

pub fn get_messages(conn: &Connection, business_id: &str, user_id: &str, session_id: &str) -> Result<Vec<Value>> {
    assert_owns_session(conn, business_id, user_id, session_id)?;
    let mut stmt = conn.prepare(
        "SELECT role, content, created_at FROM ai_chat_messages WHERE session_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![session_id], |r| {
        Ok(json!({
            "role": r.get::<_, String>(0)?,
            "content": r.get::<_, String>(1)?,
            "created_at": r.get::<_, String>(2)?,
        }))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Loads the session's prior turns as `ai_assistant::Turn`s, ready to
/// hand straight to `ask_with_history` — the one place storage format
/// and provider-call format meet. Ownership-checked like every other
/// function here: without this, a forged session id in a request could
/// read a DIFFERENT business's or user's private chat history straight
/// into an AI prompt — and out to a third-party provider — before the
/// later write ever got a chance to reject it. The read has to be
/// gated, not just the write.
pub fn history_for_provider(conn: &Connection, business_id: &str, user_id: &str, session_id: &str) -> Result<Vec<crate::ai_assistant::Turn>> {
    assert_owns_session(conn, business_id, user_id, session_id)?;
    let mut stmt = conn.prepare(
        "SELECT role, content FROM ai_chat_messages WHERE session_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![session_id], |r| {
        Ok(crate::ai_assistant::Turn { role: r.get(0)?, content: r.get(1)? })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Records one question/answer pair and bumps the session's
/// `updated_at` (so the history list re-sorts to the top, same
/// "most recently active" ordering any chat app uses) — and, the
/// first time this session gets a real question, derives a short
/// title from it so "New chat" in the sidebar becomes something
/// actually identifying, exactly the way a person would title it
/// themselves if asked to summarize their own question in a few words.
pub fn record_turn(
    conn: &mut Connection,
    business_id: &str,
    user_id: &str,
    session_id: &str,
    question: &str,
    answer: &str,
) -> Result<()> {
    assert_owns_session(conn, business_id, user_id, session_id)?;
    let tx = conn.transaction()?;

    tx.execute(
        "INSERT INTO ai_chat_messages (id, session_id, role, content, created_at)
         VALUES (?1, ?2, 'user', ?3, datetime('now'))",
        params![Uuid::new_v4().to_string(), session_id, question],
    )?;
    tx.execute(
        "INSERT INTO ai_chat_messages (id, session_id, role, content, created_at)
         VALUES (?1, ?2, 'ai', ?3, datetime('now'))",
        params![Uuid::new_v4().to_string(), session_id, answer],
    )?;

    let is_first_question: bool = tx.query_row(
        "SELECT COUNT(*) = 2 FROM ai_chat_messages WHERE session_id = ?1",
        params![session_id],
        |r| r.get(0),
    )?;
    if is_first_question {
        let title: String = question.chars().take(60).collect();
        let title = if question.chars().count() > 60 { format!("{title}…") } else { title };
        tx.execute(
            "UPDATE ai_chat_sessions SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![title, session_id],
        )?;
    } else {
        tx.execute(
            "UPDATE ai_chat_sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )?;
    }

    tx.commit()?;
    Ok(())
}

/// "Clear chat" — empties a session's messages but keeps the session
/// itself (and its slot in history), so a mis-tap doesn't silently
/// delete a conversation someone actually wanted to keep; that's what
/// `delete_session` is for. Resets the title back to "New chat" since
/// the question that produced the old one is now gone.
pub fn clear_session(conn: &mut Connection, business_id: &str, user_id: &str, session_id: &str) -> Result<()> {
    assert_owns_session(conn, business_id, user_id, session_id)?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM ai_chat_messages WHERE session_id = ?1", params![session_id])?;
    tx.execute(
        "UPDATE ai_chat_sessions SET title = 'New chat', updated_at = datetime('now') WHERE id = ?1",
        params![session_id],
    )?;
    tx.commit()?;
    Ok(())
}

/// Deletes a session entirely, messages included (`ON DELETE CASCADE`
/// — see db_migrations.rs v9) — removes it from history for good,
/// unlike `clear_session` above.
pub fn delete_session(conn: &Connection, business_id: &str, user_id: &str, session_id: &str) -> Result<()> {
    assert_owns_session(conn, business_id, user_id, session_id)?;
    conn.execute("DELETE FROM ai_chat_sessions WHERE id = ?1", params![session_id])?;
    Ok(())
}

/// Exports every one of this user's chat sessions — every question and
/// answer, across every conversation — into one real .xlsx workbook,
/// one row per message, newest session first. This is "all datas make
/// exportable via to excel": a business owner can hand the whole AI
/// conversation history to someone else, archive it, or just read it
/// outside the app, same as every other export in this system.
pub fn export_to_xlsx(conn: &Connection, business_id: &str, user_id: &str) -> Result<Vec<u8>> {
    let sessions = list_sessions(conn, business_id, user_id)?;

    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet().set_name("AI Chat History")?;
    let header_format = Format::new().set_bold().set_background_color("#D9E1F2");

    let headers = ["Session", "Started", "Role", "Message", "Sent at"];
    for (col, h) in headers.iter().enumerate() {
        sheet.write_string_with_format(0, col as u16, *h, &header_format)?;
    }

    let mut row = 1u32;
    for session in &sessions {
        let session_id = session["id"].as_str().unwrap_or_default();
        let title = session["title"].as_str().unwrap_or("New chat");
        let created_at = session["created_at"].as_str().unwrap_or_default();
        let messages = get_messages(conn, business_id, user_id, session_id)?;
        if messages.is_empty() {
            // An empty session (created but never asked anything) still
            // gets a row — an export that silently drops empty
            // sessions would make the row count lie about how many
            // conversations actually exist.
            sheet.write_string(row, 0, title)?;
            sheet.write_string(row, 1, created_at)?;
            sheet.write_string(row, 2, "")?;
            sheet.write_string(row, 3, "(no messages)")?;
            sheet.write_string(row, 4, "")?;
            row += 1;
            continue;
        }
        for message in &messages {
            sheet.write_string(row, 0, title)?;
            sheet.write_string(row, 1, created_at)?;
            sheet.write_string(row, 2, message["role"].as_str().unwrap_or_default())?;
            sheet.write_string(row, 3, message["content"].as_str().unwrap_or_default())?;
            sheet.write_string(row, 4, message["created_at"].as_str().unwrap_or_default())?;
            row += 1;
        }
    }

    sheet.set_column_width(0, 24.0)?;
    sheet.set_column_width(1, 20.0)?;
    sheet.set_column_width(2, 8.0)?;
    sheet.set_column_width(3, 80.0)?;
    sheet.set_column_width(4, 20.0)?;

    Ok(workbook.save_to_buffer()?)
}
