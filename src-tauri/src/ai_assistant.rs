use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde_json::json;

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

/// Answers a free-form business question, grounded in a real snapshot
/// of the business's own data (see ai_context.rs) — the assistant
/// genuinely sees real current numbers, not a static description of
/// what the app can do. The provider and its key are resolved from
/// this business's own AI Settings, not a shared/global config, so
/// each business brings its own key to its own account with whichever
/// provider they've chosen.
pub fn ask(conn: &Connection, business_id: &str, user_id: &str, question: &str) -> Result<String> {
    let snapshot = ai_context::build_snapshot(conn, business_id, user_id)?;
    let system_prompt = format!(
        "You are a business assistant embedded in an SME's ERP system. \
         You are given a structured snapshot of the business's CURRENT real data below — \
         use it as ground truth and do not invent numbers that aren't in it. \
         If the snapshot doesn't contain what's needed to answer, say so plainly rather than guessing. \
         Keep answers short, concrete, and in plain language a busy shop owner would understand.\n\n\
         BUSINESS SNAPSHOT:\n{}",
        serde_json::to_string_pretty(&snapshot)?
    );

    match Provider::resolve(conn, business_id) {
        Provider::NvidiaNim => ask_nvidia_nim(conn, business_id, &system_prompt, question),
        Provider::Gemini => ask_gemini(conn, business_id, &system_prompt, question),
        Provider::OpenAi => ask_openai(conn, business_id, &system_prompt, question),
        Provider::Claude => ask_claude(conn, business_id, &system_prompt, question),
    }
}

fn tls_agent() -> Result<ureq::Agent> {
    // See the matching comment in notifications.rs — plain default
    // agent, rustls via ureq's "tls" feature, no system OpenSSL needed.
    Ok(ureq::AgentBuilder::new().build())
}

fn model_for(conn: &Connection, business_id: &str, provider_key: &str, default: &str) -> String {
    crate::settings::get(conn, business_id, &format!("ai_{provider_key}_model")).unwrap_or_else(|| default.to_string())
}

/// NVIDIA NIM — free tier, OpenAI-compatible chat completions API.
fn ask_nvidia_nim(conn: &Connection, business_id: &str, system_prompt: &str, question: &str) -> Result<String> {
    let api_key = resolve_key(conn, business_id, "nvidia", "NVIDIA_API_KEY", Some("https://build.nvidia.com"))?;
    let model = model_for(conn, business_id, "nvidia", "deepseek-ai/deepseek-v4-pro");

    let body = json!({
        "model": model,
        "max_tokens": 500,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": question }
        ]
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
fn ask_gemini(conn: &Connection, business_id: &str, system_prompt: &str, question: &str) -> Result<String> {
    let api_key = resolve_key(conn, business_id, "gemini", "GOOGLE_API_KEY", Some("https://aistudio.google.com"))?;
    let model = model_for(conn, business_id, "gemini", "gemini-2.5-flash");

    let body = json!({
        "systemInstruction": { "parts": [{ "text": system_prompt }] },
        "contents": [{ "parts": [{ "text": question }] }]
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
fn ask_openai(conn: &Connection, business_id: &str, system_prompt: &str, question: &str) -> Result<String> {
    let api_key = resolve_key(conn, business_id, "openai", "OPENAI_API_KEY", None)?;
    let model = model_for(conn, business_id, "openai", "gpt-4o-mini");

    let body = json!({
        "model": model,
        "max_tokens": 500,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": question }
        ]
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
fn ask_claude(conn: &Connection, business_id: &str, system_prompt: &str, question: &str) -> Result<String> {
    let api_key = resolve_key(conn, business_id, "claude", "ANTHROPIC_API_KEY", None)?;
    let model = model_for(conn, business_id, "claude", "claude-sonnet-4-6");

    let body = json!({
        "model": model,
        "max_tokens": 500,
        "system": system_prompt,
        "messages": [{ "role": "user", "content": question }]
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
