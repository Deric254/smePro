use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde_json::json;

use crate::ai_context;

/// Which AI backend to call. NVIDIA NIM is the default because it's
/// genuinely free (no credit card, ~40 requests/min) and OpenAI-
/// compatible, which keeps its request/response shape simple. Gemini
/// and Claude are drop-in alternatives — same `ask()` function, same
/// context-grounding behavior, just a different HTTP call underneath.
#[derive(Debug, Clone, Copy)]
enum Provider {
    NvidiaNim,
    Gemini,
    Claude,
}

impl Provider {
    fn from_env() -> Self {
        match std::env::var("AI_PROVIDER").unwrap_or_default().to_lowercase().as_str() {
            "gemini" => Provider::Gemini,
            "claude" | "anthropic" => Provider::Claude,
            _ => Provider::NvidiaNim, // default: free, no card required
        }
    }
}

/// Answers a free-form business question, grounded in a real snapshot of
/// the business's own data (see ai_context.rs). The provider is chosen
/// by the AI_PROVIDER env var (defaults to NVIDIA NIM, which is free).
/// Every other feature in the app works with zero AI configuration —
/// this function returns a clear, actionable error if the selected
/// provider's API key isn't set, rather than crashing.
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

    match Provider::from_env() {
        Provider::NvidiaNim => ask_nvidia_nim(&system_prompt, question),
        Provider::Gemini => ask_gemini(&system_prompt, question),
        Provider::Claude => ask_claude(&system_prompt, question),
    }
}

fn tls_agent() -> Result<ureq::Agent> {
    Ok(ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(native_tls::TlsConnector::new()?))
        .build())
}

/// NVIDIA NIM — free tier, OpenAI-compatible chat completions API.
/// Get a key (no credit card) at https://build.nvidia.com
fn ask_nvidia_nim(system_prompt: &str, question: &str) -> Result<String> {
    let api_key = std::env::var("NVIDIA_API_KEY").map_err(|_| {
        anyhow!(
            "AI assistant not configured: set NVIDIA_API_KEY (free, no credit card, get one at \
             https://build.nvidia.com) or switch AI_PROVIDER to 'gemini'/'claude'. \
             Everything else in the app works without this."
        )
    })?;
    let model = std::env::var("NVIDIA_MODEL").unwrap_or_else(|_| "deepseek-ai/deepseek-v4-pro".to_string());

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
/// they're asking about is sensitive. Get a key at https://aistudio.google.com
fn ask_gemini(system_prompt: &str, question: &str) -> Result<String> {
    let api_key = std::env::var("GOOGLE_API_KEY").map_err(|_| {
        anyhow!(
            "AI assistant not configured: set GOOGLE_API_KEY (free tier, get one at \
             https://aistudio.google.com) or switch AI_PROVIDER to 'nvidia_nim'/'claude'. \
             Everything else in the app works without this."
        )
    })?;
    let model = std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".to_string());

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

/// Claude — paid, no free tier, but included since it's Anthropic's own
/// model and may be worth it once the business is generating revenue.
fn ask_claude(system_prompt: &str, question: &str) -> Result<String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
        anyhow!(
            "AI assistant not configured: set ANTHROPIC_API_KEY or switch AI_PROVIDER to \
             'nvidia_nim' (free) or 'gemini' (free tier). Everything else in the app works without this."
        )
    })?;
    let model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-6".to_string());

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
