use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------- helpers

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE business_settings (business_id TEXT NOT NULL, key TEXT NOT NULL, \
         value TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY (business_id, key));",
    )
    .unwrap();
    conn
}

fn set(conn: &Connection, key: &str, value: &str) {
    crate::settings::set(conn, "biz", key, value).unwrap();
}

/// A localhost HTTP server with canned replies. Records every request body.
struct Mock {
    base_url: &'static str,
    seen: Arc<Mutex<Vec<Value>>>,
}

impl Mock {
    fn models_seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|b| b["model"].as_str().unwrap_or("").to_string()).collect()
    }
}

fn mock(reply: impl Fn(&str) -> (u16, String) + Send + 'static) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_in_thread = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let body = read_body(&mut s);
            let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let model = parsed["model"].as_str().unwrap_or("").to_string();
            seen_in_thread.lock().unwrap().push(parsed);
            let (status, payload) = reply(&model);
            let resp = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    Mock { base_url: Box::leak(format!("http://127.0.0.1:{port}/v1").into_boxed_str()), seen }
}

fn read_body(s: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = s.read(&mut chunk).unwrap();
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        assert!(n > 0, "connection closed before headers");
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
    let len: usize = headers
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .map(|v| v.trim().parse().unwrap())
        .unwrap_or(0);
    while buf.len() < header_end + len {
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "connection closed mid-body");
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&buf[header_end..header_end + len]).to_string()
}

fn ok(text: &str) -> (u16, String) {
    (200, json!({ "choices": [{ "message": { "content": text } }] }).to_string())
}

const SECRET: &str = "sk-secret-123";

fn endpoint(label: &'static str, base_url: &'static str, models: &[&str]) -> Endpoint {
    Endpoint {
        label,
        wire: Wire::OpenAi { base_url, token_field: "max_tokens" },
        api_key: SECRET.to_string(),
        models: models.iter().map(|m| m.to_string()).collect(),
    }
}

fn run(chain: Vec<Endpoint>) -> Result<Answer> {
    send(&PreparedAiCall { system_prompt: "sys".into(), chain }, "How are sales?", &[])
}

// ------------------------------------------------------- failover chain

#[test]
fn retired_model_falls_through_to_the_next_model() {
    let m = mock(|model| if model == "dead" { (410, r#"{"detail":"end of life"}"#.into()) } else { ok("hello") });
    let answer = run(vec![endpoint("A", m.base_url, &["dead", "live"])]).unwrap();
    assert_eq!(answer.text, "hello");
    assert_eq!(answer.source, "A · live");
    assert_eq!(m.models_seen(), ["dead", "live"]);
}

#[test]
fn rejected_key_skips_the_providers_remaining_models_and_uses_the_backup() {
    let a = mock(|_| (401, r#"{"error":{"message":"bad key"}}"#.into()));
    let b = mock(|_| ok("from backup"));
    let answer = run(vec![endpoint("A", a.base_url, &["a1", "a2"]), endpoint("B", b.base_url, &["b1"])]).unwrap();
    assert_eq!(answer.source, "B · b1");
    assert_eq!(a.models_seen(), ["a1"], "a bad key fails every model alike, so only one attempt is spent");
}

#[test]
fn rate_limit_and_server_error_try_the_next_model() {
    let m = mock(|model| match model {
        "m1" => (429, "{}".into()),
        "m2" => (503, "upstream down".into()),
        _ => ok("third time"),
    });
    let answer = run(vec![endpoint("A", m.base_url, &["m1", "m2", "m3"])]).unwrap();
    assert_eq!(answer.source, "A · m3");
}

#[test]
fn empty_answer_from_a_reasoning_model_moves_on() {
    let m = mock(|model| if model == "thinker" { ok("") } else { ok("real answer") });
    let answer = run(vec![endpoint("A", m.base_url, &["thinker", "plain"])]).unwrap();
    assert_eq!(answer.text, "real answer");
}

#[test]
fn unreachable_provider_is_skipped_without_trying_its_other_models() {
    let dead_port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port(); // listener dropped: port closed
    let dead_url: &'static str = Box::leak(format!("http://127.0.0.1:{dead_port}/v1").into_boxed_str());
    let b = mock(|_| ok("ok"));
    let answer = run(vec![endpoint("A", dead_url, &["a1", "a2"]), endpoint("B", b.base_url, &["b1"])]).unwrap();
    assert_eq!(answer.source, "B · b1");
}

#[test]
fn total_failure_lists_every_attempt_and_never_leaks_the_key_or_url() {
    let m = mock(|model| if model == "m1" { (410, r#"{"detail":"gone for good"}"#.into()) } else { (500, "boom".into()) });
    let err = run(vec![endpoint("A", m.base_url, &["m1", "m2"])]).err().unwrap().to_string();
    assert!(err.contains("A m1 → 410: gone for good"), "{err}");
    assert!(err.contains("A m2 → 500: boom"), "{err}");
    assert!(err.contains("Admin → AI Settings"), "{err}");
    assert!(!err.contains(SECRET) && !err.contains("127.0.0.1"), "{err}");
}

#[test]
fn only_the_most_recent_turns_are_sent() {
    let m = mock(|_| ok("ok"));
    let history: Vec<Turn> = (0..40)
        .map(|i| Turn { role: if i % 2 == 0 { "user" } else { "ai" }.into(), content: format!("turn {i}") })
        .collect();
    send(&PreparedAiCall { system_prompt: "sys".into(), chain: vec![endpoint("A", m.base_url, &["m"])] }, "now?", &history).unwrap();
    let body = m.seen.lock().unwrap()[0].clone();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1 + MAX_HISTORY_TURNS + 1, "system + capped history + question");
    assert_eq!(messages[1]["content"], "turn 28", "oldest kept turn");
    assert_eq!(messages.last().unwrap()["content"], "now?");
}

// --------------------------------------------------------- request shapes

fn sample_prompt<'a>(history: &'a [Turn]) -> Prompt<'a> {
    Prompt { system: "SYS", question: "Q?", history }
}

#[test]
fn openai_style_request_has_system_first_and_the_providers_token_field() {
    let history = [Turn { role: "user".into(), content: "hi".into() }, Turn { role: "ai".into(), content: "hello".into() }];
    let ep = Endpoint {
        label: "O",
        wire: Wire::OpenAi { base_url: "https://x/v1", token_field: "max_completion_tokens" },
        api_key: "k".into(),
        models: vec![],
    };
    let (url, headers, body) = build_request(&ep, "m", &sample_prompt(&history));
    assert_eq!(url, "https://x/v1/chat/completions");
    assert_eq!(headers, [("Authorization", "Bearer k".to_string())]);
    assert_eq!(body["max_completion_tokens"], MAX_OUTPUT_TOKENS);
    assert!(body.get("max_tokens").is_none());
    let roles: Vec<&str> = body["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"], "the app's own 'ai' role becomes 'assistant'");
}

#[test]
fn claude_request_puts_system_at_top_level_not_in_messages() {
    let ep = Endpoint { label: "C", wire: Wire::Claude, api_key: "k".into(), models: vec![] };
    let (_, headers, body) = build_request(&ep, "m", &sample_prompt(&[]));
    assert_eq!(body["system"], "SYS");
    assert!(body["messages"].as_array().unwrap().iter().all(|m| m["role"] != "system"));
    assert!(headers.contains(&("x-api-key", "k".to_string())));
}

#[test]
fn gemini_key_travels_in_a_header_never_in_the_url() {
    let history = [Turn { role: "ai".into(), content: "earlier".into() }];
    let ep = Endpoint { label: "G", wire: Wire::Gemini, api_key: SECRET.into(), models: vec![] };
    let (url, headers, body) = build_request(&ep, "gemini-x", &sample_prompt(&history));
    assert!(url.ends_with("/models/gemini-x:generateContent"));
    assert!(!url.contains(SECRET) && !url.contains("key="));
    assert_eq!(headers, [("x-goog-api-key", SECRET.to_string())]);
    assert_eq!(body["contents"][0]["role"], "model");
}

// ------------------------------------------------------- response parsing

#[test]
fn extracts_text_for_each_wire_shape() {
    let openai = Wire::OpenAi { base_url: "", token_field: "" };
    assert_eq!(extract_text(openai, &json!({"choices":[{"message":{"content":"a"}}]})).as_deref(), Some("a"));
    assert_eq!(extract_text(openai, &json!({"choices":[{"message":{"content":null}}]})), None);
    let gemini = json!({"candidates":[{"content":{"parts":[{"text":"hidden","thought":true},{"text":"one "},{"text":"two"}]}}]});
    assert_eq!(extract_text(Wire::Gemini, &gemini).as_deref(), Some("one two"), "thought parts are not part of the answer");
    let claude = json!({"content":[{"type":"thinking","thinking":"..."},{"type":"text","text":"c"}]});
    assert_eq!(extract_text(Wire::Claude, &claude).as_deref(), Some("c"));
}

#[test]
fn clean_answer_strips_think_blocks_and_rejects_truncated_ones() {
    assert_eq!(clean_answer("  plain  "), "plain");
    assert_eq!(clean_answer("<think>hmm</think>\n\nThe answer"), "The answer");
    assert_eq!(clean_answer("<think>never finished"), "");
}

#[test]
fn error_summary_reads_the_common_error_shapes_and_caps_length() {
    let nvidia = r#"{"type":"about:blank","title":"Gone","status":410,"detail":"The model has reached its end of life"}"#;
    assert_eq!(error_summary(nvidia), "The model has reached its end of life");
    assert_eq!(error_summary(r#"{"error":{"message":"quota exceeded"}}"#), "quota exceeded");
    assert_eq!(error_summary("<html>\n  Bad   Gateway </html>"), "<html> Bad Gateway </html>");
    assert_eq!(error_summary(&"x".repeat(500)).chars().count(), 161);
}

// ------------------------------------------------- provider configuration

fn labels(chain: &[Endpoint]) -> Vec<&'static str> {
    chain.iter().map(|e| e.label).collect()
}

#[test]
fn chain_is_active_provider_then_other_free_providers_that_have_keys() {
    let conn = db();
    set(&conn, "ai_provider", "groq");
    set(&conn, "ai_groq_api_key", "g");
    set(&conn, "ai_nvidia_api_key", "n");
    set(&conn, "ai_gemini_api_key", "m");
    assert_eq!(labels(&chain_for(&conn, "biz").unwrap()), ["Groq", "NVIDIA NIM", "Google Gemini"]);
}

#[test]
fn paid_providers_are_only_used_when_chosen_never_as_a_silent_backup() {
    let conn = db();
    set(&conn, "ai_nvidia_api_key", "n");
    set(&conn, "ai_claude_api_key", "c");
    set(&conn, "ai_openai_api_key", "o");
    assert_eq!(labels(&chain_for(&conn, "biz").unwrap()), ["NVIDIA NIM"]);

    set(&conn, "ai_provider", "claude");
    assert_eq!(labels(&chain_for(&conn, "biz").unwrap()), ["Claude (Anthropic)", "NVIDIA NIM"]);
}

#[test]
fn admin_model_override_goes_first_without_duplicates_and_chain_is_bounded() {
    let conn = db();
    set(&conn, "ai_nvidia_api_key", "n");
    set(&conn, "ai_nvidia_model", "  my/custom-model ");
    let models = chain_for(&conn, "biz").unwrap().remove(0).models;
    assert_eq!(models[0], "my/custom-model");
    assert_eq!(models.len(), MAX_MODELS_PER_PROVIDER);

    set(&conn, "ai_nvidia_model", PROVIDERS[0].models[1]);
    let models = chain_for(&conn, "biz").unwrap().remove(0).models;
    assert_eq!(models[0], PROVIDERS[0].models[1]);
    assert_eq!(models.iter().filter(|m| *m == PROVIDERS[0].models[1]).count(), 1);
}

#[test]
fn blank_override_and_unknown_provider_fall_back_to_defaults() {
    let conn = db();
    set(&conn, "ai_provider", "nvidia_nim"); // legacy .env value
    set(&conn, "ai_nvidia_api_key", "n");
    set(&conn, "ai_nvidia_model", "   ");
    let models = chain_for(&conn, "biz").unwrap().remove(0).models;
    assert_eq!(models[0], PROVIDERS[0].models[0]);
}

#[test]
fn active_provider_without_a_key_is_a_clear_error_naming_where_to_fix_it() {
    // Env vars are process-global; only assert when the dev machine has none set.
    if std::env::var("GROQ_API_KEY").is_ok() {
        return;
    }
    let conn = db();
    set(&conn, "ai_provider", "groq");
    set(&conn, "ai_nvidia_api_key", "n"); // a backup alone must not silently replace the choice
    let err = chain_for(&conn, "biz").err().unwrap().to_string();
    assert!(err.contains("Groq") && err.contains("console.groq.com") && err.contains("Admin → AI Settings"), "{err}");
}

#[test]
fn settings_view_reports_key_presence_but_never_the_key() {
    let conn = db();
    set(&conn, "ai_provider", "groq");
    set(&conn, "ai_groq_api_key", SECRET);
    set(&conn, "ai_groq_model", "openai/gpt-oss-20b");
    let view = settings_view(&conn, "biz");
    assert_eq!(view["provider"], "groq");
    let groq = view["providers"].as_array().unwrap().iter().find(|p| p["id"] == "groq").unwrap();
    assert_eq!(groq["key_set"], true);
    assert_eq!(groq["model"], "openai/gpt-oss-20b");
    assert_eq!(groq["default_model"], PROVIDERS[1].models[0]);
    assert!(!view.to_string().contains(SECRET));
}

#[test]
fn provider_table_is_internally_consistent() {
    let mut ids: Vec<_> = PROVIDERS.iter().map(|p| p.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), PROVIDERS.len(), "duplicate provider id");
    assert!(PROVIDERS.iter().all(|p| !p.models.is_empty()));
    assert!(PROVIDERS[0].free, "the default provider must be a free one");
}
