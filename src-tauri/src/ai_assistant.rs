use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::ai_context;

/// THE ACTUAL FIX Deric asked for: the previous default and fallback
/// were the exact same model id, so once NVIDIA retired it (the
/// screenshot this fix was written against shows the API's own 410
/// "end of life" response), EVERY business on default settings had no
/// working fallback at all — `model != fallback_model` was always
/// false, so the fallback branch in `ask_nvidia_nim` below could never
/// actually run.
///
/// IMPORTANT — NEEDS VERIFICATION: these two model ids are this app's
/// best information as of its last update, not a live-checked current
/// catalog — NVIDIA periodically retires models the same way it just
/// retired the previous default, and I have no way to confirm from
/// here whether either of these two is still live today. If the AI
/// assistant still doesn't work after this fix, the real, current
/// answer is whatever's listed at https://build.nvidia.com/models
/// right now — set it under Admin → AI Settings, which always
/// overrides both of these without needing a code change.
const DEFAULT_NVIDIA_MODEL: &str = "meta/llama-3.1-70b-instruct";
/// Deliberately a different model FROM A DIFFERENT VENDOR than the
/// default above, not just a different Llama version — reduces the
/// odds that whatever event retires one also takes down the other at
/// the same time.
const FALLBACK_NVIDIA_MODEL: &str = "mistralai/mixtral-8x7b-instruct-v0.1";

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
/// Everything a call to an AI provider needs that has to come from the
/// database — the provider choice, its API key, its model, and the
/// grounding snapshot baked into the system prompt. Deliberately holds
/// no `Connection` and nothing borrowed from one: this is the exact
/// boundary between "needs the DB" and "just needs the network," so a
/// caller can build one of these under a brief lock, drop the lock,
/// and then call `send()` against owned data while the (possibly
/// multi-second) HTTP call to the provider is in flight. See
/// http_api.rs's AI-ask routes for why that boundary matters: this
/// app's API server holds one shared `Mutex<Connection>` per request,
/// and a network call made while still holding that lock blocks every
/// other endpoint, for every user, for as long as the provider takes
/// to answer.
pub struct PreparedAiCall {
    provider: Provider,
    api_key: String,
    model: String,
    system_prompt: String,
}

/// The DB-dependent half of answering a question: builds the grounding
/// snapshot and resolves which provider/key/model this business is
/// configured to use. Does no network I/O — safe to run under a brief
/// lock. Pair with `send()` below, called after the lock is released.
pub fn prepare(conn: &Connection, business_id: &str, user_id: &str) -> Result<PreparedAiCall> {
    let snapshot = ai_context::build_snapshot(conn, business_id, user_id)?;
    let system_prompt = format!(
        "You are a business assistant embedded in an SME's ERP system. \
         You are given a structured snapshot of the business's CURRENT real data below — \
         use it as ground truth and do not invent numbers that aren't in it. \
         If the snapshot doesn't contain what's needed to answer, say so plainly rather than guessing. \
         Keep answers short, concrete, and in plain language a busy shop owner would understand. \
         Structure every answer with a short heading, then the direct answer, followed by a \
         'What to do next' section when an action is useful. Use Markdown headings and bullet lists \
         sparingly so the answer is easy to scan. Never invent numbers or recommendations that the \
         snapshot does not support.\n\n\
         BUSINESS SNAPSHOT:\n{}",
        serde_json::to_string_pretty(&snapshot)?
    );

    let provider = Provider::resolve(conn, business_id);
    let (api_key, model) = match provider {
        Provider::NvidiaNim => (
            resolve_key(conn, business_id, "nvidia", "NVIDIA_API_KEY", Some("https://build.nvidia.com"))?,
            model_for(conn, business_id, "nvidia", DEFAULT_NVIDIA_MODEL),
        ),
        Provider::Gemini => (
            resolve_key(conn, business_id, "gemini", "GOOGLE_API_KEY", Some("https://aistudio.google.com"))?,
            model_for(conn, business_id, "gemini", "gemini-3.6-flash"),
        ),
        Provider::OpenAi => (
            resolve_key(conn, business_id, "openai", "OPENAI_API_KEY", None)?,
            model_for(conn, business_id, "openai", "gpt-4o-mini"),
        ),
        Provider::Claude => (
            resolve_key(conn, business_id, "claude", "ANTHROPIC_API_KEY", None)?,
            model_for(conn, business_id, "claude", "claude-sonnet-4-6"),
        ),
    };

    Ok(PreparedAiCall { provider, api_key, model, system_prompt })
}

/// The network half — no `Connection` anywhere in this call chain.
/// Safe (and intended) to call with no lock held at all.
pub fn send(prepared: &PreparedAiCall, question: &str, history: &[Turn]) -> Result<String> {
    match prepared.provider {
        Provider::NvidiaNim => ask_nvidia_nim(&prepared.api_key, &prepared.model, &prepared.system_prompt, question, history),
        Provider::Gemini => ask_gemini(&prepared.api_key, &prepared.model, &prepared.system_prompt, question, history),
        Provider::OpenAi => ask_openai(&prepared.api_key, &prepared.model, &prepared.system_prompt, question, history),
        Provider::Claude => ask_claude(&prepared.api_key, &prepared.model, &prepared.system_prompt, question, history),
    }
}

/// Same DB-read-then-network shape as `prepare()` + `send()` above,
/// collapsed into one call — convenient for callers that don't need
/// the lock released in between (e.g. tests, which use an in-memory
/// connection with no concurrent requests to block). Real HTTP
/// callers (see http_api.rs) use `prepare()`/`send()` directly instead,
/// specifically so they CAN release the lock in between.
pub fn ask_with_history(
    conn: &Connection,
    business_id: &str,
    user_id: &str,
    question: &str,
    history: &[Turn],
) -> Result<String> {
    let prepared = prepare(conn, business_id, user_id)?;
    send(&prepared, question, history)
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
    // The timeout here is still not optional polish, even now that
    // http_api.rs's AI-ask routes release the DB mutex before making
    // this call (see prepare()/send() above and serve()'s dedicated
    // AI handling) and the server spawns a thread per request: a hung
    // provider would otherwise tie up that one thread indefinitely.
    // Bounded at 30s so one stalled provider costs one slow answer,
    // never an unbounded hang.
    Ok(ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(30)).build())
}

fn model_for(conn: &Connection, business_id: &str, provider_key: &str, default: &str) -> String {
    crate::settings::get(conn, business_id, &format!("ai_{provider_key}_model")).unwrap_or_else(|| default.to_string())
}

/// Builds the OpenAI-compatible `messages` array (system, prior turns,
/// and the new question) shared by NIM and OpenAI — the only two
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
/// Takes an already-resolved `api_key`/`model` (see `prepare()` above)
/// rather than a `Connection` — this function is purely network I/O,
/// deliberately callable with no DB lock held.
fn ask_nvidia_nim(api_key: &str, model: &str, system_prompt: &str, question: &str, history: &[Turn]) -> Result<String> {
    let model = model.to_string();
    // THE BUG THIS FIXES: this used to be the exact same string as
    // `DEFAULT_NVIDIA_MODEL` above, which meant the fallback attempt
    // below could never actually run for anyone using the default (i.e.
    // everyone who hasn't set `ai_nvidia_model` explicitly) — `model !=
    // fallback_model` was always false, so a dead default model's 410
    // just propagated straight through with no real fallback ever
    // attempted, exactly what the screenshot this fix was written
    // against shows. Deliberately a DIFFERENT, separately-maintained
    // model id, so the fallback path is real: if whichever one is
    // configured as the primary reaches end of life, the other one
    // still has a chance of being live.
    let fallback_model = FALLBACK_NVIDIA_MODEL;
    let agent = tls_agent()?;
    let request = |model_name: &str| -> Result<String> {
        let body = json!({
            "model": model_name,
            "max_tokens": 500,
            "messages": openai_style_messages(system_prompt, question, history),
        });
        match agent
            .post("https://integrate.api.nvidia.com/v1/chat/completions")
            .set("Authorization", &format!("Bearer {api_key}"))
            .set("content-type", "application/json")
            .send_json(body)
        {
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
    };

    match request(&model) {
        Err(error) if model != fallback_model && error.to_string().contains("returned 410") => {
            request(fallback_model).map_err(|fallback_error| {
                anyhow!(
                    "NVIDIA NIM model '{model}' is no longer available, and the fallback model '{fallback_model}' \
                     also failed: {fallback_error}. NVIDIA periodically retires older models — pick a current one \
                     from https://build.nvidia.com/models and set it under Admin → AI Settings."
                )
            })
        }
        result => result,
    }
}

/// Google Gemini — free tier (Flash / Flash-Lite), no credit card.
/// Note: on the free tier, Google's terms allow using your prompts to
/// improve their models — flag this to the business owner if the data
/// they're asking about is sensitive.
fn ask_gemini(api_key: &str, model: &str, system_prompt: &str, question: &str, history: &[Turn]) -> Result<String> {
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
fn ask_openai(api_key: &str, model: &str, system_prompt: &str, question: &str, history: &[Turn]) -> Result<String> {
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
fn ask_claude(api_key: &str, model: &str, system_prompt: &str, question: &str, history: &[Turn]) -> Result<String> {
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
        .set("x-api-key", api_key)
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
