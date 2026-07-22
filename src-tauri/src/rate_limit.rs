use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A simple fixed-window rate limiter, keyed by whatever the caller
/// chooses (business_id + username, or business_id + IP, etc).
///
/// This exists specifically for auth-adjacent endpoints — login,
/// security-question recovery, and admin-code recovery — where an
/// attacker gets unlimited free guesses otherwise. It is deliberately
/// simple (in-memory, per-process) rather than a distributed rate
/// limiter: this is a single-tenant local desktop app, not a public
/// multi-server API, so the threat model is "someone hammering this one
/// running instance," which a process-local limiter fully covers.
pub struct RateLimiter {
    attempts: Mutex<HashMap<String, Vec<Instant>>>,
    max_attempts: usize,
    window: Duration,
}

impl RateLimiter {
    pub fn new(max_attempts: usize, window: Duration) -> Self {
        Self { attempts: Mutex::new(HashMap::new()), max_attempts, window }
    }

    /// Records an attempt for `key` and returns `Ok(())` if still under
    /// the limit, or `Err(seconds_until_retry)` if the caller should be
    /// rejected. Call this BEFORE doing the expensive work (password
    /// hashing, DB lookups) so a lockout also saves real CPU, not just
    /// blocks the response.
    pub fn check(&self, key: &str) -> Result<(), u64> {
        let mut map = self.attempts.lock().unwrap();
        let now = Instant::now();
        let entry = map.entry(key.to_string()).or_default();

        // Drop attempts outside the current window — this is what makes
        // it a *rolling* window rather than a permanent lockout.
        entry.retain(|&t| now.duration_since(t) < self.window);

        if entry.len() >= self.max_attempts {
            let oldest = entry[0];
            let retry_after = self.window.saturating_sub(now.duration_since(oldest));
            return Err(retry_after.as_secs().max(1));
        }

        entry.push(now);
        Ok(())
    }

    /// Clears attempts for `key` — called on a successful login so a
    /// legitimate user who mistyped their password a couple of times
    /// isn't left sitting near the limit afterward.
    pub fn reset(&self, key: &str) {
        self.attempts.lock().unwrap().remove(key);
    }
}
