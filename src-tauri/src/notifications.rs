use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
enum Provider {
    /// Records the message and marks it "sent (logged only)" — no
    /// external account needed. This is the default so the whole
    /// notification system is usable and testable out of the box.
    /// A real deployment without a WhatsApp/SMS account configured still
    /// gets a correct, inspectable log of what *would* have gone out.
    Log,
    /// Real delivery via Twilio (SMS or WhatsApp, depending on the
    /// `from` number's configuration in the Twilio console).
    Twilio,
}

impl Provider {
    fn from_env() -> Self {
        match std::env::var("NOTIFICATION_PROVIDER").unwrap_or_default().to_lowercase().as_str() {
            "twilio" => Provider::Twilio,
            _ => Provider::Log,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct NotificationRecord {
    pub id: String,
    pub channel: String,
    pub recipient: String,
    pub message: String,
    pub status: String,
}

/// Sends (or logs) a WhatsApp/SMS notification and always records it in
/// the `notifications` table — this is what gives an owner a visible
/// history of alerts sent to customers/suppliers regardless of which
/// provider is active.
pub fn send(conn: &Connection, business_id: &str, channel: &str, recipient: &str, message: &str) -> Result<NotificationRecord> {
    if channel != "whatsapp" && channel != "sms" {
        return Err(anyhow!("channel must be 'whatsapp' or 'sms'"));
    }

    let (status, provider_response) = match Provider::from_env() {
        Provider::Log => ("sent (logged only, no provider configured)".to_string(), None),
        Provider::Twilio => match send_via_twilio(channel, recipient, message) {
            Ok(resp) => ("sent".to_string(), Some(resp)),
            Err(e) => ("failed".to_string(), Some(e.to_string())),
        },
    };

    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO notifications (id, business_id, channel, recipient, message, status, provider_response, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
        params![id, business_id, channel, recipient, message, status, provider_response],
    )?;

    Ok(NotificationRecord {
        id,
        channel: channel.to_string(),
        recipient: recipient.to_string(),
        message: message.to_string(),
        status,
    })
}

/// Real Twilio delivery. Requires TWILIO_ACCOUNT_SID, TWILIO_AUTH_TOKEN,
/// and TWILIO_FROM_NUMBER. For WhatsApp specifically, TWILIO_FROM_NUMBER
/// must be a WhatsApp-enabled sender per Twilio's WhatsApp API setup —
/// that's a Twilio account configuration step, not something this code
/// can do for you.
fn send_via_twilio(channel: &str, recipient: &str, message: &str) -> Result<String> {
    let sid = std::env::var("TWILIO_ACCOUNT_SID").map_err(|_| anyhow!("TWILIO_ACCOUNT_SID not set"))?;
    let token = std::env::var("TWILIO_AUTH_TOKEN").map_err(|_| anyhow!("TWILIO_AUTH_TOKEN not set"))?;
    let from = std::env::var("TWILIO_FROM_NUMBER").map_err(|_| anyhow!("TWILIO_FROM_NUMBER not set"))?;

    let (from_addr, to_addr) = if channel == "whatsapp" {
        (format!("whatsapp:{from}"), format!("whatsapp:{recipient}"))
    } else {
        (from, recipient.to_string())
    };

    let url = format!("https://api.twilio.com/2010-04-01/Accounts/{sid}/Messages.json");
    let agent = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(native_tls::TlsConnector::new()?))
        .build();

    use base64::Engine;
    let basic_auth = base64::engine::general_purpose::STANDARD.encode(format!("{sid}:{token}"));

    let response = agent
        .post(&url)
        .set("Authorization", &format!("Basic {basic_auth}"))
        .send_form(&[("From", &from_addr), ("To", &to_addr), ("Body", &message.to_string())]);

    match response {
        Ok(resp) => Ok(resp.into_string()?),
        Err(ureq::Error::Status(code, resp)) => {
            Err(anyhow!("Twilio API returned {code}: {}", resp.into_string().unwrap_or_default()))
        }
        Err(e) => Err(anyhow!("failed to reach Twilio API: {e}")),
    }
}

/// Composes and sends a low-stock alert using the same context builder
/// that powers the AI assistant — one source of truth for "what's low"
/// used by both features.
pub fn send_low_stock_alert(conn: &Connection, business_id: &str, user_id: &str, channel: &str, recipient: &str) -> Result<NotificationRecord> {
    let snapshot = crate::ai_context::build_snapshot(conn, business_id, user_id)?;
    let mut lines = vec!["Low stock alert:".to_string()];
    if let Some(modules) = snapshot["modules"].as_object() {
        for (module_id, data) in modules {
            if let Some(alerts) = data["low_stock_alerts"].as_array() {
                for item in alerts {
                    lines.push(format!(
                        "- [{module_id}] {}: {} left (reorder at {})",
                        item["name"].as_str().unwrap_or("?"),
                        item["quantity"],
                        item["reorder_level"]
                    ));
                }
            }
        }
    }
    if lines.len() == 1 {
        lines.push("Nothing is currently low — all items are above their reorder level.".to_string());
    }
    send(conn, business_id, channel, recipient, &lines.join("\n"))
}

pub fn list_recent(conn: &Connection, business_id: &str, limit: i64) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, channel, recipient, message, status, created_at FROM notifications
         WHERE business_id = ?1 ORDER BY created_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![business_id, limit], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "channel": r.get::<_, String>(1)?,
            "recipient": r.get::<_, String>(2)?,
            "message": r.get::<_, String>(3)?,
            "status": r.get::<_, String>(4)?,
            "created_at": r.get::<_, String>(5)?,
        }))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}
