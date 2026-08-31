use super::common::*;
use crate::ai_chat;

#[test]
fn test_create_and_record_turn_sets_title_from_first_question() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let session_id = ai_chat::create_session(&conn, &biz, &uid).unwrap();
    let sessions = ai_chat::list_sessions(&conn, &biz, &uid).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["title"], "New chat");

    ai_chat::record_turn(&mut conn, &biz, &uid, &session_id, "What's low on stock?", "Nothing is low right now.").unwrap();

    let sessions = ai_chat::list_sessions(&conn, &biz, &uid).unwrap();
    assert_eq!(sessions[0]["title"], "What's low on stock?");
    assert_eq!(sessions[0]["message_count"], 2);

    let messages = ai_chat::get_messages(&conn, &biz, &uid, &session_id).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "What's low on stock?");
    assert_eq!(messages[1]["role"], "ai");
    assert_eq!(messages[1]["content"], "Nothing is low right now.");
}

#[test]
fn test_second_turn_keeps_title_and_appends_messages() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let session_id = ai_chat::create_session(&conn, &biz, &uid).unwrap();
    ai_chat::record_turn(&mut conn, &biz, &uid, &session_id, "How were sales today?", "You made $200 today.").unwrap();
    ai_chat::record_turn(&mut conn, &biz, &uid, &session_id, "And yesterday?", "Yesterday was $150.").unwrap();

    let messages = ai_chat::get_messages(&conn, &biz, &uid, &session_id).unwrap();
    assert_eq!(messages.len(), 4);

    let sessions = ai_chat::list_sessions(&conn, &biz, &uid).unwrap();
    // Title stays from the FIRST question, a second question doesn't
    // overwrite it.
    assert_eq!(sessions[0]["title"], "How were sales today?");
    assert_eq!(sessions[0]["message_count"], 4);
}

#[test]
fn test_history_for_provider_returns_turns_in_order() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let session_id = ai_chat::create_session(&conn, &biz, &uid).unwrap();
    ai_chat::record_turn(&mut conn, &biz, &uid, &session_id, "Q1", "A1").unwrap();
    ai_chat::record_turn(&mut conn, &biz, &uid, &session_id, "Q2", "A2").unwrap();

    let history = ai_chat::history_for_provider(&conn, &biz, &uid, &session_id).unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content, "Q1");
    assert_eq!(history[3].role, "ai");
    assert_eq!(history[3].content, "A2");
}

#[test]
fn test_clear_session_empties_messages_but_keeps_the_session() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let session_id = ai_chat::create_session(&conn, &biz, &uid).unwrap();
    ai_chat::record_turn(&mut conn, &biz, &uid, &session_id, "Q1", "A1").unwrap();

    ai_chat::clear_session(&mut conn, &biz, &uid, &session_id).unwrap();

    let messages = ai_chat::get_messages(&conn, &biz, &uid, &session_id).unwrap();
    assert!(messages.is_empty());

    // The session itself is still there, in history, just empty and
    // retitled — clear is not the same as delete.
    let sessions = ai_chat::list_sessions(&conn, &biz, &uid).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["title"], "New chat");
    assert_eq!(sessions[0]["message_count"], 0);
}

#[test]
fn test_delete_session_removes_it_and_its_messages() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let session_id = ai_chat::create_session(&conn, &biz, &uid).unwrap();
    ai_chat::record_turn(&mut conn, &biz, &uid, &session_id, "Q1", "A1").unwrap();

    ai_chat::delete_session(&conn, &biz, &uid, &session_id).unwrap();

    let sessions = ai_chat::list_sessions(&conn, &biz, &uid).unwrap();
    assert!(sessions.is_empty());
    // The row is gone, so even an ownership-checked read now correctly
    // reports "not found" rather than an empty list.
    assert!(ai_chat::get_messages(&conn, &biz, &uid, &session_id).is_err());
}

/// Regression test for the exact gap found during review: an ownership
/// check that only guarded the later write (`record_turn`), not the
/// read (`history_for_provider`) that happens first — which meant a
/// forged session id could pull a DIFFERENT business's private chat
/// history into an AI prompt before the write ever got a chance to
/// reject it. Every one of these calls, with business B's user reading
/// a session that belongs to business A, must fail — not return data.
#[test]
fn test_cannot_read_or_write_another_businesss_session() {
    let mut conn = test_db();
    let biz_a = test_business(&mut conn);
    let (uid_a, _) = test_owner(&mut conn, &biz_a);
    let biz_b = crate::business_panel::create_business(&mut conn, "Other Biz", "USD", "UTC").unwrap();
    crate::onboarding::apply_business_type(&mut conn, &biz_b, "retail").unwrap();
    let hash = crate::auth::hash_secret("password123").unwrap();
    let uid_b = crate::business_panel::add_user(&conn, &biz_b, "owner_b", &hash, "Owner").unwrap();

    let session_id = ai_chat::create_session(&conn, &biz_a, &uid_a).unwrap();
    ai_chat::record_turn(&mut conn, &biz_a, &uid_a, &session_id, "private question", "private answer").unwrap();

    // Business B, reading business A's session id directly:
    assert!(ai_chat::get_messages(&conn, &biz_b, &uid_b, &session_id).is_err());
    assert!(ai_chat::history_for_provider(&conn, &biz_b, &uid_b, &session_id).is_err());
    assert!(ai_chat::clear_session(&mut conn, &biz_b, &uid_b, &session_id).is_err());
    assert!(ai_chat::delete_session(&conn, &biz_b, &uid_b, &session_id).is_err());
    assert!(ai_chat::record_turn(&mut conn, &biz_b, &uid_b, &session_id, "x", "y").is_err());

    // Business B's own session list stays empty — A's session never
    // leaks into it.
    assert!(ai_chat::list_sessions(&conn, &biz_b, &uid_b).unwrap().is_empty());

    // Business A can still read its own session fine — the isolation
    // above isn't accidentally blocking the legitimate owner too.
    let history = ai_chat::history_for_provider(&conn, &biz_a, &uid_a, &session_id).unwrap();
    assert_eq!(history.len(), 2);
}

#[test]
fn test_export_to_xlsx_produces_a_real_workbook_with_messages_and_empty_sessions() {
    let mut conn = test_db();
    let biz = test_business(&mut conn);
    let (uid, _) = test_owner(&mut conn, &biz);

    let with_messages = ai_chat::create_session(&conn, &biz, &uid).unwrap();
    ai_chat::record_turn(&mut conn, &biz, &uid, &with_messages, "Q1", "A1").unwrap();
    // An empty session (created, never asked anything) must still show
    // up in the export — a silent drop would make the row count lie.
    let _empty = ai_chat::create_session(&conn, &biz, &uid).unwrap();

    let bytes = ai_chat::export_to_xlsx(&conn, &biz, &uid).unwrap();
    // A real, non-trivial .xlsx is a zip archive — "PK" magic bytes at
    // the start is the cheapest real assertion that this is an actual
    // workbook and not an empty or corrupt buffer.
    assert!(bytes.len() > 100);
    assert_eq!(&bytes[0..2], b"PK");
}
