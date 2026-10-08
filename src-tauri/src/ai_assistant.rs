//! The AI assistant's provider layer.
//!
//! Model names are the least stable thing this app depends on: providers
//! retire models every few months, and a hardcoded name is a bug with a
//! timer on it. So nothing here assumes any single model stays alive:
//!
//! * every provider has an ordered list of candidate models, tried in turn;
//! * the admin can override the first candidate (Admin → AI Settings);
//! * if a provider is exhausted or rejects the key, the next *free*
//!   provider that has a key stored is tried automatically;
//! * the whole attempt chain runs inside a fixed time budget, so a bad day
//!   at one vendor costs one slow answer, never a hang.
//!
//! The model lists below are the only place model names live. When one
//! goes stale, the chain skips it; update the list at your leisure.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::ai_context;

const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
const TOTAL_BUDGET: Duration = Duration::from_secs(45);
const MIN_ATTEMPT: Duration = Duration::from_secs(3);
/// Per provider. Bounds worst-case latency when several models are dead.
const MAX_MODELS_PER_PROVIDER: usize = 3;
/// Free tiers cap tokens per minute (Groq: 8K). Resending an entire long
/// conversation every turn would trip that, so only the recent turns go.
const MAX_HISTORY_TURNS: usize = 12;
/// Headroom matters: reasoning models spend output tokens thinking, and a
/// cap that is too small leaves an empty answer.
const MAX_OUTPUT_TOKENS: u32 = 1200;

/// How a provider's HTTP API is shaped.
#[derive(Debug, Clone, Copy)]
enum Wire {
    /// OpenAI chat-completions. NVIDIA NIM, Groq, OpenRouter and OpenAI all
    /// speak it; `token_field` is the name each accepts for the output cap.
    OpenAi { base_url: &'static str, token_field: &'static str },
    Gemini,
    Claude,
}

struct ProviderSpec {
    /// Settings-key segment: `ai_<id>_api_key`, `ai_<id>_model`.
    id: &'static str,
    label: &'static str,
    free: bool,
    key_url: &'static str,
    env_key: &'static str,
    wire: Wire,
    /// Candidate models, best first. `models[0]` is the shown default.
    models: &'static [&'static str],
}

/// Order matters twice: the first entry is the default provider, and free
/// providers are failed over to in this order (largest free daily
/// allowance first). IDs last verified against provider docs, Oct 2026.
const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "nvidia",
        label: "NVIDIA NIM",
        free: true,
        key_url: "https://build.nvidia.com",
        env_key: "NVIDIA_API_KEY",
        wire: Wire::OpenAi { base_url: "https://integrate.api.nvidia.com/v1", token_field: "max_tokens" },
        models: &["nvidia/nemotron-3-super-120b-a12b", "openai/gpt-oss-20b", "meta/llama-3.3-70b-instruct"],
    },
    ProviderSpec {
        id: "groq",
        label: "Groq",
        free: true,
        key_url: "https://console.groq.com/keys",
        env_key: "GROQ_API_KEY",
        wire: Wire::OpenAi { base_url: "https://api.groq.com/openai/v1", token_field: "max_tokens" },
        models: &["openai/gpt-oss-120b", "openai/gpt-oss-20b", "qwen/qwen3.8-27b"],
    },
    ProviderSpec {
        id: "openrouter",
        label: "OpenRouter",
        free: true,
        key_url: "https://openrouter.ai/keys",
        env_key: "OPENROUTER_API_KEY",
        wire: Wire::OpenAi { base_url: "https://openrouter.ai/api/v1", token_field: "max_tokens" },
        // `openrouter/free` routes to whichever free model is live.
        models: &["nvidia/nemotron-3-super-120b-a12b:free", "google/gemma-4-31b-it:free", "openrouter/free"],
    },
    ProviderSpec {
        id: "gemini",
        label: "Google Gemini",
        free: true,
        key_url: "https://aistudio.google.com",
        env_key: "GOOGLE_API_KEY",
        wire: Wire::Gemini,
        models: &["gemini-3.6-flash", "gemini-2.5-flash", "gemini-2.5-flash-lite"],
    },
    ProviderSpec {
        id: "openai",
        label: "OpenAI",
        free: false,
        key_url: "https://platform.openai.com/api-keys",
        env_key: "OPENAI_API_KEY",
        wire: Wire::OpenAi { base_url: "https://api.openai.com/v1", token_field: "max_completion_tokens" },
        models: &["gpt-5.4-mini", "gpt-4o-mini"],
    },
    ProviderSpec {
        id: "claude",
        label: "Claude (Anthropic)",
        free: false,
        key_url: "https://console.anthropic.com",
        env_key: "ANTHROPIC_API_KEY",
        wire: Wire::Claude,
        models: &["claude-sonnet-5-5", "claude-haiku-5-5"],
    },
];

/// The business's chosen provider: stored setting first (the only path a
/// real customer can reach), then `AI_PROVIDER` for local dev, then the
/// first (free) provider. Unrecognised values fall back to that default.
fn active_spec(conn: &Connection, business_id: &str) -> &'static ProviderSpec {
    let raw = crate::settings::get(conn, business_id, "ai_provider")
        .unwrap_or_else(|| std::env::var("AI_PROVIDER").unwrap_or_default())
        .to_lowercase();
    let id = if raw == "anthropic" { "claude" } else { raw.as_str() };
    PROVIDERS.iter().find(|p| p.id == id).unwrap_or(&PROVIDERS[0])
}

/// Stored key first (Admin → AI Settings), then the environment variable.
fn resolve_key(conn: &Connection, business_id: &str, spec: &ProviderSpec) -> Option<String> {
    crate::settings::get(conn, business_id, &format!("ai_{}_api_key", spec.id))
        .into_iter()
        .chain(std::env::var(spec.env_key))
        .map(|k| k.trim().to_string())
        .find(|k| !k.is_empty())
}

fn model_override(conn: &Connection, business_id: &str, spec: &ProviderSpec) -> Option<String> {
    crate::settings::get(conn, business_id, &format!("ai_{}_model", spec.id))
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

/// What `GET /ai/settings` returns: the one provider table, so the admin
/// screen never keeps its own copy of provider names or defaults. Reports
/// whether a key exists, never the key.
pub fn settings_view(conn: &Connection, business_id: &str) -> Value {
    let providers: Vec<Value> = PROVIDERS
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "label": s.label,
                "free": s.free,
                "key_url": s.key_url,
                "key_set": resolve_key(conn, business_id, s).is_some(),
                "model": model_override(conn, business_id, s).unwrap_or_default(),
                "default_model": s.models[0],
            })
        })
        .collect();
    json!({ "provider": active_spec(conn, business_id).id, "providers": providers })
}

/// One prior turn. `role` is "user" or "ai", the values stored in
/// `ai_chat_messages.role`, so history passes through with no translation.
pub struct Turn {
    pub role: String,
    pub content: String,
}

/// A provider that is configured and ready to be called. Owns everything
/// it needs, so the network phase never touches the database.
struct Endpoint {
    label: &'static str,
    wire: Wire,
    api_key: String,
    models: Vec<String>,
}

/// Everything a call needs from the database, resolved up front. Holds no
/// `Connection`: a caller builds this under a brief lock, drops the lock,
/// then calls `send()`. The API server shares one `Mutex<Connection>`, and
/// a multi-second network call made while holding it would freeze every
/// other request for every user (see http_api.rs's `handle_ai_ask`).
pub struct PreparedAiCall {
    system_prompt: String,
    chain: Vec<Endpoint>,
}

/// A successful answer and which provider/model produced it, so the UI can
/// say when a backup answered.
pub struct Answer {
    pub text: String,
    pub source: String,
}

/// DB half: the grounding snapshot plus the provider chain. No network I/O.
pub fn prepare(conn: &Connection, business_id: &str, user_id: &str) -> Result<PreparedAiCall> {
    let snapshot = ai_context::build_snapshot(conn, business_id, user_id)?;
    let system_prompt = format!(
        "You are a business assistant embedded in an SME's ERP system. \
         You are given a structured snapshot of the business's CURRENT real data below — \
         use it as ground truth and do not invent numbers that aren't in it. \
         If the snapshot doesn't contain what's needed to answer, say so plainly rather than guessing. \
         `monthly_revenue` is revenue per calendar month; a month flagged `month_in_progress` is only \
         partly elapsed, so never present it as a full month or compare it to one as if equivalent. \
         Keep answers short, concrete, and in plain language a busy shop owner would understand. \
         Structure every answer with a short heading, then the direct answer, followed by a \
         'What to do next' section when an action is useful. Use Markdown headings and bullet lists \
         sparingly so the answer is easy to scan. Never invent numbers or recommendations that the \
         snapshot does not support.\n\n\
         BUSINESS SNAPSHOT:\n{}",
        // Compact, not pretty: whitespace is wasted tokens on a metered free tier.
        serde_json::to_string(&snapshot)?
    );
    Ok(PreparedAiCall { system_prompt, chain: chain_for(conn, business_id)? })
}

/// The active provider first (it must have a key), then every other FREE
/// provider that has a key stored. Paid providers are never failed over
/// to: that would bill someone who only chose a free one.
fn chain_for(conn: &Connection, business_id: &str) -> Result<Vec<Endpoint>> {
    let active = active_spec(conn, business_id);
    let endpoint = |spec: &ProviderSpec| -> Option<Endpoint> {
        let api_key = resolve_key(conn, business_id, spec)?;
        let mut models: Vec<String> = model_override(conn, business_id, spec).into_iter().collect();
        for m in spec.models {
            if !models.iter().any(|x| x == m) {
                models.push((*m).to_string());
            }
        }
        models.truncate(MAX_MODELS_PER_PROVIDER);
        Some(Endpoint { label: spec.label, wire: spec.wire, api_key, models })
    };

    let first = endpoint(active).ok_or_else(|| {
        let hint = if active.free { format!(" — free, no credit card, get one at {}", active.key_url) } else { String::new() };
        anyhow!(
            "AI assistant not configured: {} has no API key{hint}. Add one under Admin → AI Settings. \
             Everything else in the app works without this.",
            active.label
        )
    })?;
    let backups = PROVIDERS.iter().filter(|s| s.free && s.id != active.id).filter_map(endpoint);
    Ok(std::iter::once(first).chain(backups).collect())
}

/// Network half: no `Connection` anywhere in this call chain.
pub fn send(prepared: &PreparedAiCall, question: &str, history: &[Turn]) -> Result<Answer> {
    let prompt = Prompt {
        system: &prepared.system_prompt,
        question,
        history: &history[history.len().saturating_sub(MAX_HISTORY_TURNS)..],
    };
    let deadline = Instant::now() + TOTAL_BUDGET;
    let mut failures: Vec<String> = Vec::new();

    'providers: for ep in &prepared.chain {
        for model in &ep.models {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining < MIN_ATTEMPT {
                failures.push("ran out of time".to_string());
                break 'providers;
            }
            match attempt(ep, model, &prompt, remaining.min(ATTEMPT_TIMEOUT)) {
                Ok(text) => return Ok(Answer { text, source: format!("{} · {model}", ep.label) }),
                Err(f) => {
                    failures.push(format!("{} {model} → {}", ep.label, f.detail));
                    if f.skip_provider {
                        break;
                    }
                }
            }
        }
    }

    Err(anyhow!(
        "The AI assistant could not answer. Tried: {}. Fix: in Admin → AI Settings, check the API key, \
         set a model your provider currently offers, or add a second free provider as a backup.",
        failures.join("; ")
    ))
}

struct Prompt<'a> {
    system: &'a str,
    question: &'a str,
    history: &'a [Turn],
}

impl Prompt<'_> {
    /// Prior turns plus the new question, in `{role, content}` shape.
    /// `assistant_role` is what the API calls the model's own side.
    fn chat_messages(&self, assistant_role: &str) -> Vec<Value> {
        self.history
            .iter()
            .map(|t| json!({ "role": if t.role == "ai" { assistant_role } else { "user" }, "content": t.content }))
            .chain(std::iter::once(json!({ "role": "user", "content": self.question })))
            .collect()
    }
}

/// Why one call failed, and whether the rest of that provider's models are
/// worth trying. A rejected key (401/403), a billing block (402) or an
/// unreachable host will fail identically for every model; a retired model
/// (404/410), a rate limit (429) or a server error will not.
struct Failure {
    detail: String,
    skip_provider: bool,
}

impl Failure {
    fn model(detail: impl Into<String>) -> Self {
        Failure { detail: detail.into(), skip_provider: false }
    }
}

fn attempt(ep: &Endpoint, model: &str, p: &Prompt, timeout: Duration) -> Result<String, Failure> {
    let (url, headers, body) = build_request(ep, model, p);
    let resp = post_json(&url, &headers, &body, timeout)?;
    let text = clean_answer(&extract_text(ep.wire, &resp).unwrap_or_default());
    if text.is_empty() {
        return Err(Failure::model("empty answer"));
    }
    Ok(text)
}

fn build_request(ep: &Endpoint, model: &str, p: &Prompt) -> (String, Vec<(&'static str, String)>, Value) {
    match ep.wire {
        Wire::OpenAi { base_url, token_field } => {
            let mut messages = vec![json!({ "role": "system", "content": p.system })];
            messages.extend(p.chat_messages("assistant"));
            (
                format!("{base_url}/chat/completions"),
                vec![("Authorization", format!("Bearer {}", ep.api_key))],
                json!({ "model": model, token_field: MAX_OUTPUT_TOKENS, "messages": messages }),
            )
        }
        Wire::Gemini => {
            // Gemini calls the model's side "model" and wraps text in "parts".
            let contents: Vec<Value> = p
                .history
                .iter()
                .map(|t| json!({ "role": if t.role == "ai" { "model" } else { "user" }, "parts": [{ "text": t.content }] }))
                .chain(std::iter::once(json!({ "role": "user", "parts": [{ "text": p.question }] })))
                .collect();
            (
                format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"),
                // Header, not `?key=`: transport errors print the URL, and
                // those reach the chat window.
                vec![("x-goog-api-key", ep.api_key.clone())],
                json!({
                    "systemInstruction": { "parts": [{ "text": p.system }] },
                    "contents": contents,
                    "generationConfig": { "maxOutputTokens": MAX_OUTPUT_TOKENS },
                }),
            )
        }
        Wire::Claude => (
            "https://api.anthropic.com/v1/messages".to_string(),
            vec![("x-api-key", ep.api_key.clone()), ("anthropic-version", "2023-06-01".to_string())],
            // Claude takes `system` as a top-level field, not a message.
            json!({ "model": model, "max_tokens": MAX_OUTPUT_TOKENS, "system": p.system, "messages": p.chat_messages("assistant") }),
        ),
    }
}

fn post_json(url: &str, headers: &[(&str, String)], body: &Value, timeout: Duration) -> Result<Value, Failure> {
    // rustls via ureq's "tls" feature: no system OpenSSL, which is what
    // breaks cross-compiled (Android, cross-arch macOS) builds.
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let mut req = agent.post(url).set("content-type", "application/json");
    for (name, value) in headers {
        req = req.set(name, value);
    }
    match req.send_json(body) {
        Ok(resp) => resp.into_json::<Value>().map_err(|_| Failure::model("unreadable response")),
        Err(ureq::Error::Status(code, resp)) => Err(Failure {
            detail: format!("{code}: {}", error_summary(&resp.into_string().unwrap_or_default())),
            skip_provider: matches!(code, 401 | 402 | 403),
        }),
        // `kind()` only: the full Display includes the URL.
        Err(ureq::Error::Transport(t)) => Err(Failure { detail: format!("unreachable ({})", t.kind()), skip_provider: true }),
    }
}

fn extract_text(wire: Wire, resp: &Value) -> Option<String> {
    match wire {
        Wire::OpenAi { .. } => resp["choices"][0]["message"]["content"].as_str().map(str::to_string),
        Wire::Gemini => {
            let parts = resp["candidates"][0]["content"]["parts"].as_array()?;
            Some(parts.iter().filter(|p| p["thought"] != true).filter_map(|p| p["text"].as_str()).collect())
        }
        Wire::Claude => resp["content"]
            .as_array()?
            .iter()
            .find(|b| b["type"] == "text")
            .and_then(|b| b["text"].as_str())
            .map(str::to_string),
    }
}

/// Some reasoning models inline their thinking as a `<think>…</think>`
/// prefix. Drop it; an unclosed one means the answer was cut off while
/// still thinking, so return nothing and let the caller try another model.
fn clean_answer(raw: &str) -> String {
    let t = raw.trim();
    if !t.starts_with("<think>") {
        return t.to_string();
    }
    match t.find("</think>") {
        Some(end) => t[end + "</think>".len()..].trim().to_string(),
        None => String::new(),
    }
}

/// Pulls the human-readable message out of a provider error body (the
/// common shapes are `{"error":{"message"}}` and RFC 7807 `{"detail"}`),
/// falling back to the raw text, one line, capped.
fn error_summary(body: &str) -> String {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = [&parsed["error"]["message"], &parsed["detail"], &parsed["message"], &parsed["error"]]
        .into_iter()
        .find_map(Value::as_str)
        .unwrap_or(body);
    let one_line = msg.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 160 {
        format!("{}…", one_line.chars().take(160).collect::<String>())
    } else {
        one_line
    }
}

// Compiled as a child module (not listed in tests/mod.rs) so the tests can
// reach the private chain/request internals they exercise.
#[cfg(test)]
#[path = "tests/ai_assistant_tests.rs"]
mod tests;
