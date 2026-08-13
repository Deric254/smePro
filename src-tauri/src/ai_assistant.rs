use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::ai_context;

/// Which AI backend to call. NVIDIA NIM is the default because it's
/// genuinely free (no credit card, ~40 requests/min) and OpenAI-
/// compatible, which keeps its request/response shape simple. Gemini,
/// OpenAI, and Claude are drop-in alternatives — same `ask()`
/// function, same context-grounding behavior, just a different HTTP
/// call underneath.
#[derive(Debug, Clone, Copy)]
enum Provider {
    NvidiaNim,
    Gemini,
    OpenAi,
    Claude,
}

impl Provider {
    /// Resolves the configured provider — checks the business's own
    /// stored setting first (set via Admin → AI Settings, the only
    /// path a real customer can actually reach), falling back to the
    /// AI_PROVIDER environment variable for local development
    /// convenience, and finally to the free default if neither is set.
    fn resolve(conn: &Connection, business_id: &str) -> Self {
        let stored = crate::settings::get(conn, business_id, "ai_provider");
        let raw = stored.unwrap_or_else(|| std::env::var("AI_PROVIDER").unwrap_or_default());
        match raw.to_lowercase().as_str() {
            "gemini" => Provider::Gemini,
            "openai" => Provider::OpenAi,
            "claude" | "anthropic" => Provider::Claude,
            _ => Provider::NvidiaNim, // default: free, no card required
        }
    }
}

/// Reads an API key for `provider_key` (e.g. "nvidia") — the stored
/// setting first (Admin → AI Settings), then the matching environment
/// variable as a fallback for local dev. Returns a clear, actionable
/// error naming exactly where to go fix it, rather than a bare "not
/// configured."
fn resolve_key(conn: &Connection, business_id: &str, provider_key: &str, env_var: &str, free_url: Option<&str>) -> Result<String> {
    let setting_key = format!("ai_{provider_key}_api_key");
    if let Some(k) = crate::settings::get(conn, business_id, &setting_key) {
        if !k.trim().is_empty() {
            return Ok(k);
        }
    }
    if let Ok(k) = std::env::var(env_var) {
        if !k.trim().is_empty() {
            return Ok(k);
        }
    }
    let hint = free_url
        .map(|u| format!(" — free, no credit card, get one at {u}"))
        .unwrap_or_default();
    Err(anyhow!(
        "AI assistant not configured for this provider{hint}. Add a key under Admin → AI Settings. \
         Everything else in the app works without this."
    ))
}

/// One prior turn of a conversation — `role` is "user" or "ai", matching
/// the values stored in `ai_chat_messages.role` (see ai_chat.rs), so
/// callers can pass a session's history straight through with no
/// translation step.
pub struct Turn {
    pub role: String,
    pub content: String,
}

/// Answers a free-form business question, grounded in a real snapshot
/// of the business's own data (see ai_context.rs) — the assistant
/// genuinely sees real current numbers, not a static description of
/// what the app can do. The provider and its key are resolved from
/// this business's own AI Settings, not a shared/global config, so
/// each business brings its own key to its own account with whichever
/// provider they've chosen.
///
/// Same as `ask`, but with the conversation's prior turns included so a
/// follow-up question ("what about last week?") is actually answered in
/// context instead of as an isolated one-off question. `history` should
/// be in chronological order and NOT include `question` itself.
pub fn ask_with_history(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    question: &str,
    history: &[Turn],
) -> Result<String> {
    let snapshot = ai_context::build_snapshot(conn, business_id, user_id)?;
    let system_prompt = format!(
        "You are a business assistant embedded in an SME's ERP system. \
         You are given a structured snapshot of the business's CURRENT real data below — \
         use it as ground truth and do not invent numbers that aren't in it. \
         If the snapshot doesn't contain what's needed to answer, say so plainly rather than guessing. \
         Keep answers short, concrete, and in plain language a busy shop owner would understand. \
         This answer is shown in a plain chat bubble that does NOT render markdown — never use \
         asterisks, #, backticks, or any markdown syntax; write plain sentences, and use a plain \
         hyphen '-' at the start of a line for a list item if a list genuinely helps.\n\n\
         BUSINESS SNAPSHOT:\n{}",
        serde_json::to_string_pretty(&snapshot)?
    );

    match Provider::resolve(conn, business_id) {
        Provider::NvidiaNim => ask_nvidia_nim(conn, business_id, &system_prompt, question, history),
        Provider::Gemini => ask_gemini(conn, business_id, &system_prompt, question, history),
        Provider::OpenAi => ask_openai(conn, business_id, &system_prompt, question, history),
        Provider::Claude => ask_claude(conn, business_id, &system_prompt, question, history),
    }
}

/// Single-turn convenience wrapper, kept for any existing caller that
/// doesn't have a session (e.g. tests) — equivalent to `ask_with_history`
/// with an empty history.
pub fn ask(conn: &Connection, business_id: &str, user_id: &str, question: &str) -> Result<String> {
    ask_with_history(conn, business_id, user_id, question, &[])
}

fn tls_agent() -> Result<ureq::Agent> {
    // See the matching comment in notifications.rs — plain default
    // agent, rustls via ureq's "tls" feature, no system OpenSSL needed.
    //
    // The timeout here is NOT optional polish: http_api.rs::serve()
    // runs a single-threaded, blocking `incoming_requests()` loop —
    // every request for every user of this business is handled
    // serially on one thread. An AI provider call with no timeout
    // that hangs (network stall, provider outage, an LLM that never
    // finishes generating) would freeze the ENTIRE HTTP API — every
    // endpoint, every user — until the process is killed and
    // restarted, since there's no other thread to serve anything
    // else in the meantime. 30 seconds is generous enough for
    // legitimate LLM generation latency while still bounding how
    // long one stalled provider can take the whole app down for.
    Ok(ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(30)).build())
}

fn model_for(conn: &Connection, business_id: &str, provider_key: &str, default: &str) -> String {
    crate::settings::get(conn, business_id, &format!("ai_{provider_key}_model")).unwrap_or_else(|| default.to_string())
}

/// Builds the OpenAI-compatible `messages` array (system + prior turns
/// + the new question) shared by NIM and OpenAI — the only two
/// providers here using that exact wire shape. `Turn.role` of "ai" is
/// translated to the API's own "assistant" here, since "ai" is this
/// app's internal storage vocabulary (see ai_chat.rs), not any
/// provider's wire format.
fn openai_style_messages(system_prompt: &str, question: &str, history: &[Turn]) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": system_prompt })];
    for turn in history {
        let role = if turn.role == "ai" { "assistant" } else { "user" };
        messages.push(json!({ "role": role, "content": turn.content }));
    }
    messages.push(json!({ "role": "user", "content": question }));
    json!(messages)
}

/// NVIDIA NIM — free tier, OpenAI-compatible chat completions API.
fn ask_nvidia_nim(conn: &Connection, business_id: &str, system_prompt: &str, question: &str, history: &[Turn]) -> Result<String> {
    let api_key = resolve_key(conn, business_id, "nvidia", "NVIDIA_API_KEY", Some("https://build.nvidia.com"))?;
    let model = model_for(conn, business_id, "nvidia", "deepseek-ai/deepseek-v4-pro");

    let body = json!({
        "model": model,
        "max_tokens": 500,
        "messages": openai_style_messages(system_prompt, question, history),
        // Required for NVIDIA NIM's DeepSeek-V4 family specifically —
        // without this, the request can hang with no response at all
        // rather than erroring cleanly (confirmed via NVIDIA's own
        // official API example at build.nvidia.com/deepseek-ai/deepseek-v4-pro,
        // which explicitly sets this, and a documented case of the
        // exact hang when it's omitted). "thinking: false" is also the
        // right choice functionally, not just to avoid the hang: this
        // is a short, plain-language business Q&A assistant (see the
        // system prompt above), not a task that benefits from
        // DeepSeek's extended chain-of-thought reasoning mode — and
        // thinking mode would also be slower and more expensive for
        // no benefit here. If a future default model on this provider
        // isn't a DeepSeek-V4-family model, an unrecognized field in
        // extra_body/chat_template_kwargs is typically just ignored by
        // OpenAI-compatible APIs rather than erroring, so this stays
        // safe even if the default model changes later.
        "chat_template_kwargs": { "thinking": false }
    });

    let agent = tls_agent()?;
    let response = agent
        .post("https://integrate.api.nvidia.com/v1/chat/completions")
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("content-type", "application/json")
        .send_json(body);

    match response {
        Ok(resp) => {
            let parsed: serde_json::Value = resp.into_json()?;
            parsed["choices"][0]["message"]["content"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("unexpected response shape from NVIDIA NIM"))
        }
        Err(ureq::Error::Status(code, resp)) => {
            Err(anyhow!("NVIDIA NIM API returned {code}: {}", resp.into_string().unwrap_or_default()))
        }
        Err(e) => Err(anyhow!("failed to reach NVIDIA NIM API: {e}")),
    }
}

/// Google Gemini — free tier (Flash / Flash-Lite), no credit card.
/// Note: on the free tier, Google's terms allow using your prompts to
/// improve their models — flag this to the business owner if the data
/// they're asking about is sensitive.
fn ask_gemini(conn: &Connection, business_id: &str, system_prompt: &str, question: &str, history: &[Turn]) -> Result<String> {
    let api_key = resolve_key(conn, business_id, "gemini", "GOOGLE_API_KEY", Some("https://aistudio.google.com"))?;
    let model = model_for(conn, business_id, "gemini", "gemini-3.6-flash");

    // Gemini's wire format uses "model" (not "assistant") for the
    // other side of the conversation, and "contents" instead of
    // "messages" — different enough from the OpenAI-style shape above
    // that it isn't worth sharing a helper between the two.
    let mut contents: Vec<Value> = history
        .iter()
        .map(|t| json!({ "role": if t.role == "ai" { "model" } else { "user" }, "parts": [{ "text": t.content }] }))
        .collect();
    contents.push(json!({ "role": "user", "parts": [{ "text": question }] }));

    let body = json!({
        "systemInstruction": { "parts": [{ "text": system_prompt }] },
        "contents": contents
    });

    let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}");
    let agent = tls_agent()?;
    let response = agent.post(&url).set("content-type", "application/json").send_json(body);

    match response {
        Ok(resp) => {
            let parsed: serde_json::Value = resp.into_json()?;
            parsed["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("unexpected response shape from Gemini"))
        }
        Err(ureq::Error::Status(code, resp)) => {
            Err(anyhow!("Gemini API returned {code}: {}", resp.into_string().unwrap_or_default()))
        }
        Err(e) => Err(anyhow!("failed to reach Gemini API: {e}")),
    }
}

/// OpenAI — paid (has a small free trial credit for new accounts, not
/// an ongoing free tier). Chat completions API, same shape as NVIDIA
/// NIM since NIM deliberately mirrors it.
fn ask_openai(conn: &Connection, business_id: &str, system_prompt: &str, question: &str, history: &[Turn]) -> Result<String> {
    let api_key = resolve_key(conn, business_id, "openai", "OPENAI_API_KEY", None)?;
    let model = model_for(conn, business_id, "openai", "gpt-4o-mini");

    let body = json!({
        "model": model,
        "max_tokens": 500,
        "messages": openai_style_messages(system_prompt, question, history)
    });

    let agent = tls_agent()?;
    let response = agent
        .post("https://api.openai.com/v1/chat/completions")
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("content-type", "application/json")
        .send_json(body);

    match response {
        Ok(resp) => {
            let parsed: serde_json::Value = resp.into_json()?;
            parsed["choices"][0]["message"]["content"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("unexpected response shape from OpenAI"))
        }
        Err(ureq::Error::Status(code, resp)) => {
            Err(anyhow!("OpenAI API returned {code}: {}", resp.into_string().unwrap_or_default()))
        }
        Err(e) => Err(anyhow!("failed to reach OpenAI API: {e}")),
    }
}

/// Claude — paid, no ongoing free tier, but included since it's
/// Anthropic's own model and may be worth it once the business is
/// generating revenue.
fn ask_claude(conn: &Connection, business_id: &str, system_prompt: &str, question: &str, history: &[Turn]) -> Result<String> {
    let api_key = resolve_key(conn, business_id, "claude", "ANTHROPIC_API_KEY", None)?;
    let model = model_for(conn, business_id, "claude", "claude-sonnet-4-6");

    // Claude takes "system" as its own top-level field, not a message
    // in the array — same shape as OpenAI's messages otherwise
    // ("assistant" for the model's own prior turns), so this reuses
    // the same translation as openai_style_messages minus the system
    // entry, which Claude would reject as an invalid message role.
    let mut messages: Vec<Value> = history
        .iter()
        .map(|t| json!({ "role": if t.role == "ai" { "assistant" } else { "user" }, "content": t.content }))
        .collect();
    messages.push(json!({ "role": "user", "content": question }));

    let body = json!({
        "model": model,
        "max_tokens": 500,
        "system": system_prompt,
        "messages": messages
    });

    let agent = tls_agent()?;
    let response = agent
        .post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", &api_key)
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .send_json(body);

    match response {
        Ok(resp) => {
            let parsed: serde_json::Value = resp.into_json()?;
            parsed["content"]
                .as_array()
                .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
                .and_then(|b| b["text"].as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("unexpected response shape from Claude API"))
        }
        Err(ureq::Error::Status(code, resp)) => {
            Err(anyhow!("Claude API returned {code}: {}", resp.into_string().unwrap_or_default()))
        }
        Err(e) => Err(anyhow!("failed to reach Claude API: {e}")),
    }
}
