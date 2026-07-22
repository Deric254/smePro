use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Purpose {
    Activation,
    Subscription,
}

impl Purpose {
    pub fn as_str(&self) -> &'static str {
        match self {
            Purpose::Activation => "activation",
            Purpose::Subscription => "subscription",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "activation" => Ok(Purpose::Activation),
            "subscription" => Ok(Purpose::Subscription),
            other => Err(anyhow!("unknown payment purpose '{other}', expected activation or subscription")),
        }
    }
}

fn tls_agent() -> Result<ureq::Agent> {
    Ok(ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(native_tls::TlsConnector::new()?))
        .build())
}

fn record_intent(
    conn: &Connection,
    business_id: &str,
    provider: &str,
    provider_reference: &str,
    purpose: Purpose,
    amount: f64,
    currency: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO payment_intents (id, business_id, provider, provider_reference, purpose, amount, currency, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', datetime('now'))",
        params![Uuid::new_v4().to_string(), business_id, provider, provider_reference, purpose.as_str(), amount, currency],
    )?;
    Ok(())
}

/// Looks up which business + purpose a provider's reference (Stripe
/// session id, M-Pesa CheckoutRequestID) belongs to, and returns it only
/// if the intent is still pending — prevents a replayed/duplicated
/// webhook from double-crediting a payment.
fn find_pending_intent(conn: &Connection, provider_reference: &str) -> Result<(String, Purpose)> {
    let (business_id, purpose_str): (String, String) = conn.query_row(
        "SELECT business_id, purpose FROM payment_intents WHERE provider_reference = ?1 AND status = 'pending'",
        params![provider_reference],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).map_err(|_| anyhow!("no matching pending payment found for reference '{provider_reference}'"))?;
    Ok((business_id, Purpose::parse(&purpose_str)?))
}

fn mark_completed(conn: &Connection, provider_reference: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE payment_intents SET status = ?1, completed_at = datetime('now') WHERE provider_reference = ?2",
        params![status, provider_reference],
    )?;
    Ok(())
}

/// Applies a confirmed payment to the business's license — the one
/// place both Stripe and M-Pesa webhook handlers converge, so activation
/// vs. subscription-renewal logic can never drift between providers.
/// Also the one place a webhook-triggered license change gets an audit
/// entry — `user_id` is deliberately `None` here since no logged-in
/// user triggered this, the payment provider's webhook did.
fn apply_payment(conn: &Connection, business_id: &str, provider: &str, reference: &str, purpose: Purpose) -> Result<()> {
    let result = match purpose {
        // If this business is somehow already activated (e.g. a customer's
        // "activation" checkout was accidentally started twice), treat a
        // second activation payment as a renewal instead of erroring the
        // webhook — the payment genuinely happened either way, and failing
        // it here would make Stripe/Safaricom retry this webhook forever.
        Purpose::Activation => match crate::license::activate(conn, business_id) {
            Ok(()) => Ok(()),
            Err(_) => crate::license::record_payment(conn, business_id),
        },
        Purpose::Subscription => crate::license::record_payment(conn, business_id),
    };
    if result.is_ok() {
        let _ = crate::audit::log(
            conn, business_id, None, "_license",
            if purpose == Purpose::Activation { "webhook_activate" } else { "webhook_payment" },
            None,
            Some(&json!({"provider": provider, "reference": reference})),
        );
    }
    result
}

// =========================================================================
// Stripe
// =========================================================================

pub mod stripe {
    use super::*;

    /// Creates a real Stripe Checkout session and returns the URL the
    /// frontend should redirect the owner to. `client_reference_id` is
    /// how the webhook later ties the completed session back to this
    /// business — Stripe echoes it back verbatim in the event payload.
    pub fn create_checkout_session(
        conn: &Connection,
        business_id: &str,
        purpose: Purpose,
        amount: f64,
        currency: &str,
        success_url: &str,
        cancel_url: &str,
    ) -> Result<String> {
        let secret_key = std::env::var("STRIPE_SECRET_KEY")
            .map_err(|_| anyhow!("Stripe is not configured: set STRIPE_SECRET_KEY (get one at https://dashboard.stripe.com/apikeys)"))?;

        let unit_amount = (amount * 100.0).round() as i64; // Stripe wants the smallest currency unit (cents)
        let product_name = match purpose {
            Purpose::Activation => "Ledger & Counter — Activation",
            Purpose::Subscription => "Ledger & Counter — Monthly Subscription",
        };

        let agent = tls_agent()?;
        let response = agent
            .post("https://api.stripe.com/v1/checkout/sessions")
            .set("Authorization", &format!("Bearer {secret_key}"))
            .send_form(&[
                ("payment_method_types[0]", "card"),
                ("mode", "payment"),
                ("client_reference_id", business_id),
                ("metadata[business_id]", business_id),
                ("metadata[purpose]", purpose.as_str()),
                ("line_items[0][price_data][currency]", currency),
                ("line_items[0][price_data][product_data][name]", product_name),
                ("line_items[0][price_data][unit_amount]", &unit_amount.to_string()),
                ("line_items[0][quantity]", "1"),
                ("success_url", success_url),
                ("cancel_url", cancel_url),
            ]);

        let session: Value = match response {
            Ok(resp) => resp.into_json()?,
            Err(ureq::Error::Status(code, resp)) => {
                return Err(anyhow!("Stripe API returned {code}: {}", resp.into_string().unwrap_or_default()));
            }
            Err(e) => return Err(anyhow!("failed to reach Stripe API: {e}")),
        };

        let session_id = session["id"].as_str().ok_or_else(|| anyhow!("Stripe response missing session id"))?;
        let url = session["url"].as_str().ok_or_else(|| anyhow!("Stripe response missing checkout url"))?;

        record_intent(conn, business_id, "stripe", session_id, purpose, amount, currency)?;
        Ok(url.to_string())
    }

    /// Verifies a Stripe webhook's signature. This is genuinely testable
    /// without reaching Stripe's servers — it's pure HMAC verification
    /// against a shared secret, the same construction Stripe documents:
    /// signed_payload = "{timestamp}.{raw_body}", compared against the
    /// v1 signature in the Stripe-Signature header.
    pub fn verify_webhook_signature(payload: &str, sig_header: &str, secret: &str) -> Result<()> {
        let mut timestamp = None;
        let mut v1_sig = None;
        for part in sig_header.split(',') {
            let mut kv = part.splitn(2, '=');
            match (kv.next(), kv.next()) {
                (Some("t"), Some(v)) => timestamp = Some(v),
                (Some("v1"), Some(v)) => v1_sig = Some(v),
                _ => {}
            }
        }
        let (timestamp, v1_sig) = match (timestamp, v1_sig) {
            (Some(t), Some(s)) => (t, s),
            _ => return Err(anyhow!("malformed Stripe-Signature header")),
        };

        let signed_payload = format!("{timestamp}.{payload}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| anyhow!("bad webhook secret: {e}"))?;
        mac.update(signed_payload.as_bytes());
        let expected = hex::encode(mac.finalize().into_bytes());

        if constant_time_eq(expected.as_bytes(), v1_sig.as_bytes()) {
            Ok(())
        } else {
            Err(anyhow!("webhook signature does not match — request may not be genuinely from Stripe"))
        }
    }

    /// Processes a verified `checkout.session.completed` event: looks up
    /// which business/purpose it belongs to and applies the payment.
    pub fn handle_webhook_event(conn: &Connection, event: &Value) -> Result<()> {
        let event_type = event["type"].as_str().unwrap_or("");
        if event_type != "checkout.session.completed" {
            return Ok(()); // ignore event types we don't act on
        }
        let session = &event["data"]["object"];
        let session_id = session["id"].as_str().ok_or_else(|| anyhow!("event missing session id"))?;

        let (business_id, purpose) = find_pending_intent(conn, session_id)?;
        apply_payment(conn, &business_id, "stripe", session_id, purpose)?;
        mark_completed(conn, session_id, "completed")?;
        Ok(())
    }

    fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

// =========================================================================
// M-Pesa (Safaricom Daraja API) — STK Push
// =========================================================================

pub mod mpesa {
    use super::*;
    use chrono::Utc;

    fn base_url() -> String {
        // Sandbox by default; set MPESA_ENV=production once you have
        // real Daraja production credentials and have gone through
        // Safaricom's go-live process.
        if std::env::var("MPESA_ENV").as_deref() == Ok("production") {
            "https://api.safaricom.co.ke".to_string()
        } else {
            "https://sandbox.safaricom.co.ke".to_string()
        }
    }

    fn get_oauth_token() -> Result<String> {
        let consumer_key = std::env::var("MPESA_CONSUMER_KEY")
            .map_err(|_| anyhow!("M-Pesa is not configured: set MPESA_CONSUMER_KEY / MPESA_CONSUMER_SECRET (get them at https://developer.safaricom.co.ke)"))?;
        let consumer_secret = std::env::var("MPESA_CONSUMER_SECRET")
            .map_err(|_| anyhow!("MPESA_CONSUMER_SECRET not set"))?;

        use base64::Engine;
        let basic_auth = base64::engine::general_purpose::STANDARD.encode(format!("{consumer_key}:{consumer_secret}"));

        let agent = tls_agent()?;
        let url = format!("{}/oauth/v1/generate?grant_type=client_credentials", base_url());
        let response = agent.get(&url).set("Authorization", &format!("Basic {basic_auth}")).call();

        let body: Value = match response {
            Ok(resp) => resp.into_json()?,
            Err(ureq::Error::Status(code, resp)) => {
                return Err(anyhow!("M-Pesa OAuth returned {code}: {}", resp.into_string().unwrap_or_default()));
            }
            Err(e) => return Err(anyhow!("failed to reach M-Pesa OAuth endpoint: {e}")),
        };
        body["access_token"].as_str().map(|s| s.to_string()).ok_or_else(|| anyhow!("M-Pesa OAuth response missing access_token"))
    }

    /// Initiates an STK push — a real-time payment prompt that pops up
    /// on the customer's phone asking them to enter their M-Pesa PIN.
    /// `phone` must be in `2547XXXXXXXX` format (Safaricom's requirement,
    /// not this code's choice).
    pub fn initiate_stk_push(
        conn: &Connection,
        business_id: &str,
        purpose: Purpose,
        amount: f64,
        phone: &str,
        callback_url: &str,
    ) -> Result<String> {
        let shortcode = std::env::var("MPESA_SHORTCODE").map_err(|_| anyhow!("MPESA_SHORTCODE not set"))?;
        let passkey = std::env::var("MPESA_PASSKEY").map_err(|_| anyhow!("MPESA_PASSKEY not set"))?;
        let token = get_oauth_token()?;

        let timestamp = Utc::now().format("%Y%m%d%H%M%S").to_string();
        use base64::Engine;
        let password = base64::engine::general_purpose::STANDARD.encode(format!("{shortcode}{passkey}{timestamp}"));

        let description = match purpose {
            Purpose::Activation => "Ledger and Counter Activation",
            Purpose::Subscription => "Ledger and Counter Subscription",
        };

        let body = json!({
            "BusinessShortCode": shortcode,
            "Password": password,
            "Timestamp": timestamp,
            "TransactionType": "CustomerPayBillOnline",
            "Amount": amount.round() as i64, // M-Pesa STK push amounts are whole KES, no cents
            "PartyA": phone,
            "PartyB": shortcode,
            "PhoneNumber": phone,
            "CallBackURL": callback_url,
            "AccountReference": business_id,
            "TransactionDesc": description
        });

        let agent = tls_agent()?;
        let url = format!("{}/mpesa/stkpush/v1/processrequest", base_url());
        let response = agent.post(&url).set("Authorization", &format!("Bearer {token}")).send_json(body);

        let resp_body: Value = match response {
            Ok(resp) => resp.into_json()?,
            Err(ureq::Error::Status(code, resp)) => {
                return Err(anyhow!("M-Pesa STK push returned {code}: {}", resp.into_string().unwrap_or_default()));
            }
            Err(e) => return Err(anyhow!("failed to reach M-Pesa STK push endpoint: {e}")),
        };

        let checkout_request_id = resp_body["CheckoutRequestID"].as_str()
            .ok_or_else(|| anyhow!("M-Pesa response missing CheckoutRequestID: {resp_body}"))?;

        record_intent(conn, business_id, "mpesa", checkout_request_id, purpose, amount, "KES")?;
        Ok(checkout_request_id.to_string())
    }

    /// Parses and applies Safaricom's STK push callback. Safaricom's
    /// callback shape deliberately does NOT echo back our
    /// AccountReference in the result metadata — only the
    /// CheckoutRequestID we generated at push time — which is exactly
    /// why `payment_intents` exists: to map that ID back to a business.
    pub fn handle_callback(conn: &Connection, callback_body: &Value) -> Result<()> {
        let stk = &callback_body["Body"]["stkCallback"];
        let checkout_request_id = stk["CheckoutRequestID"].as_str()
            .ok_or_else(|| anyhow!("callback missing CheckoutRequestID"))?;
        let result_code = stk["ResultCode"].as_i64().ok_or_else(|| anyhow!("callback missing ResultCode"))?;

        let (business_id, purpose) = find_pending_intent(conn, checkout_request_id)?;

        if result_code == 0 {
            apply_payment(conn, &business_id, "mpesa", checkout_request_id, purpose)?;
            mark_completed(conn, checkout_request_id, "completed")?;
        } else {
            // ResultCode != 0 means the customer cancelled, entered the
            // wrong PIN, or the request timed out — a normal outcome,
            // not a system error.
            mark_completed(conn, checkout_request_id, "failed")?;
        }
        Ok(())
    }
}
