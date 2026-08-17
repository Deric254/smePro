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
        // Same reasoning as http_api.rs's own connection lock: this
        // Mutex is shared across every login and recovery attempt for
        // the server's entire lifetime. A plain .unwrap() here means
        // one panic anywhere while this lock is held would poison it
        // permanently — and since http_api.rs now catches panics per-
        // request (see serve()'s catch_unwind), that single poisoning
        // would otherwise turn EVERY future login and recovery attempt
        // into a 500 error forever, not just the one that triggered
        // it. Recovering via into_inner() means one bad moment doesn't
        // permanently break the ability to log in at all.
        let mut map = self.attempts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        self.attempts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_recovers_from_a_poisoned_mutex_instead_of_panicking_forever() {
        // Proves the exact scenario unwrap_or_else(|poisoned|
        // poisoned.into_inner()) exists to survive: something panics
        // while holding this Mutex (simulated here by panicking in a
        // spawned thread mid-lock), and the limiter must still work
        // for every future caller afterward — not cascade into
        // permanently broken login/recovery for the rest of the
        // server's life, which is exactly what a plain .unwrap() here
        // would have caused.
        let limiter = Arc::new(RateLimiter::new(5, Duration::from_secs(60)));

        let poison_limiter = Arc::clone(&limiter);
        let handle = std::thread::spawn(move || {
            let _guard = poison_limiter.attempts.lock().unwrap();
            panic!("simulated panic while holding the rate limiter's lock");
        });
        let result = handle.join();
        assert!(result.is_err(), "the spawned thread should have genuinely panicked, proving the mutex really is poisoned next");

        // Without the fix, both of these would panic instead of
        // returning normally.
        assert!(limiter.check("some-key").is_ok(), "check() must recover from a poisoned mutex, not panic");
        limiter.reset("some-key");
    }
}
