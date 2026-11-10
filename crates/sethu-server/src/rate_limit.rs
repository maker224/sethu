//! Per-IP sliding-window rate limiting for authentication endpoints.

use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

pub struct RateLimiter {
    entries: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `true` when the request is allowed and records it.
    pub fn check(&self, key: &str, max: usize, window: Duration) -> bool {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());

        // Opportunistic cleanup keeps memory bounded on internet-facing boxes.
        if entries.len() > 4_096 {
            entries.retain(|_, hits| hits.back().is_some_and(|t| now.duration_since(*t) < window));
        }

        let hits = entries.entry(key.to_string()).or_default();
        while hits
            .front()
            .is_some_and(|t| now.duration_since(*t) >= window)
        {
            hits.pop_front();
        }
        if hits.len() >= max {
            return false;
        }
        hits.push_back(now);
        true
    }

    pub fn key(ip: Option<IpAddr>) -> String {
        ip.map(|ip| ip.to_string())
            .unwrap_or_else(|| "unknown".into())
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_limit() {
        let limiter = RateLimiter::new();
        let window = Duration::from_secs(60);
        for _ in 0..3 {
            assert!(limiter.check("1.2.3.4", 3, window));
        }
        assert!(!limiter.check("1.2.3.4", 3, window));
        assert!(limiter.check("5.6.7.8", 3, window));
    }
}
