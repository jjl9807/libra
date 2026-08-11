//! Browser Origin / CSRF and per-session write rate-limit guards (W3-05).
//!
//! Browser writes to `/api/code/*` require a trusted loopback `Origin`
//! (or same-origin `Referer` fallback). Automation writes authenticate with
//! bearer/control tokens and do **not** use Origin as a substitute (GC-CODE-11).
//! Per-session write rate limiting applies to both browser and automation
//! producers after identity checks succeed.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::http::{HeaderMap, header};

/// Default: 120 writes / 60s per Code UI session (human + automation).
pub const DEFAULT_SESSION_WRITE_RATE_LIMIT: u32 = 120;
pub const DEFAULT_SESSION_WRITE_RATE_WINDOW_SECS: u64 = 60;

const ENV_RATE_LIMIT: &str = "LIBRA_CODE_SESSION_WRITE_RATE_LIMIT";
const ENV_RATE_WINDOW: &str = "LIBRA_CODE_SESSION_WRITE_RATE_WINDOW_SECS";

fn format_http_origin(ip: IpAddr, port: u16) -> String {
    match (ip, port) {
        // Browsers omit the default HTTP port in Origin serialization.
        (IpAddr::V4(v4), 80) => format!("http://{v4}"),
        (IpAddr::V6(v6), 80) => format!("http://[{v6}]"),
        (IpAddr::V4(v4), port) => format!("http://{v4}:{port}"),
        (IpAddr::V6(v6), port) => format!("http://[{v6}]:{port}"),
    }
}

fn push_unique_origin(origins: &mut Vec<String>, origin: String) {
    if !origins
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(origin.as_str()))
    {
        origins.push(origin);
    }
}

fn push_canonical_loopback_aliases(origins: &mut Vec<String>, port: u16) {
    if port == 80 {
        push_unique_origin(origins, "http://127.0.0.1".to_string());
        push_unique_origin(origins, "http://localhost".to_string());
        push_unique_origin(origins, "http://[::1]".to_string());
        return;
    }
    push_unique_origin(origins, format!("http://127.0.0.1:{port}"));
    push_unique_origin(origins, format!("http://localhost:{port}"));
    push_unique_origin(origins, format!("http://[::1]:{port}"));
}

/// Origins the browser SPA may present when talking to this bind address.
///
/// Always trusts the exact bound IP (so `--host 127.0.0.2` works). When bound
/// to the canonical loopback addresses users type in the URL bar (`127.0.0.1`
/// / `::1`), also accept the common aliases `localhost` / `127.0.0.1` / `[::1]`.
/// Port 80 uses browser-canonical forms without `:80`.
pub fn trusted_loopback_origins(bound: SocketAddr) -> Vec<String> {
    let port = bound.port();
    let mut origins = Vec::with_capacity(4);
    push_unique_origin(&mut origins, format_http_origin(bound.ip(), port));

    let is_canonical_loopback = matches!(
        bound.ip(),
        IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST
    ) || bound.ip() == IpAddr::V6(Ipv6Addr::LOCALHOST);
    if is_canonical_loopback {
        push_canonical_loopback_aliases(&mut origins, port);
    }

    origins
}

/// Extract a browser Origin, falling back to the origin component of Referer.
pub fn browser_request_origin(headers: &HeaderMap) -> Option<String> {
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "null")
    {
        return Some(origin.to_string());
    }

    headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(origin_from_referer)
}

fn origin_from_referer(referer: &str) -> Option<String> {
    let referer = referer.trim();
    if referer.is_empty() {
        return None;
    }
    // Prefer `url::Url` when available; keep a small manual parse so this
    // module stays free of extra deps for the common http(s)://host[:port]/ case.
    let without_scheme = referer
        .strip_prefix("http://")
        .or_else(|| referer.strip_prefix("https://"))?;
    let scheme = if referer.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    let host_port = without_scheme.split('/').next().unwrap_or("");
    if host_port.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host_port}"))
}

/// Returns `Ok(())` when Origin/Referer matches a trusted loopback origin.
pub fn ensure_trusted_browser_origin(
    headers: &HeaderMap,
    trusted: &[String],
) -> Result<(), OriginGuardError> {
    let Some(origin) = browser_request_origin(headers) else {
        return Err(OriginGuardError::Missing);
    };
    if trusted
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(origin.as_str()))
    {
        Ok(())
    } else {
        Err(OriginGuardError::Untrusted { origin })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginGuardError {
    Missing,
    Untrusted { origin: String },
}

impl OriginGuardError {
    pub fn code(&self) -> &'static str {
        "ORIGIN_REQUIRED"
    }

    pub fn message(&self) -> String {
        match self {
            Self::Missing => {
                "Browser Code UI write requests require a trusted loopback Origin (or same-origin Referer)".to_string()
            }
            Self::Untrusted { origin } => {
                format!(
                    "Browser Code UI write Origin '{origin}' is not a trusted loopback origin for this server"
                )
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionWriteRateLimitConfig {
    pub max_writes: u32,
    pub window: Duration,
}

impl SessionWriteRateLimitConfig {
    pub fn from_env_or_default() -> Self {
        let max_writes = std::env::var(ENV_RATE_LIMIT)
            .ok()
            .and_then(|raw| raw.trim().parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_SESSION_WRITE_RATE_LIMIT);
        let window_secs = std::env::var(ENV_RATE_WINDOW)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_SESSION_WRITE_RATE_WINDOW_SECS);
        Self {
            max_writes,
            window: Duration::from_secs(window_secs),
        }
    }

    pub fn for_tests(max_writes: u32, window: Duration) -> Self {
        Self { max_writes, window }
    }
}

#[derive(Debug)]
struct SessionWindow {
    started_at: Instant,
    count: u32,
}

/// Sliding fixed-window limiter keyed by Code UI session id.
#[derive(Debug)]
pub struct SessionWriteRateLimiter {
    config: SessionWriteRateLimitConfig,
    sessions: Mutex<HashMap<String, SessionWindow>>,
}

impl SessionWriteRateLimiter {
    pub fn new(config: SessionWriteRateLimitConfig) -> Self {
        Self {
            config,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn from_env_or_default() -> Arc<Self> {
        Arc::new(Self::new(SessionWriteRateLimitConfig::from_env_or_default()))
    }

    pub fn config(&self) -> &SessionWriteRateLimitConfig {
        &self.config
    }

    /// Record one write. Returns `Err(retry_after)` when the session is over budget.
    pub fn check_and_record(&self, session_id: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = sessions
            .entry(session_id.to_string())
            .or_insert(SessionWindow {
                started_at: now,
                count: 0,
            });
        if now.duration_since(entry.started_at) >= self.config.window {
            entry.started_at = now;
            entry.count = 0;
        }
        if entry.count >= self.config.max_writes {
            let elapsed = now.duration_since(entry.started_at);
            let retry_after = self
                .config
                .window
                .checked_sub(elapsed)
                .unwrap_or(Duration::from_secs(1));
            return Err(retry_after);
        }
        entry.count = entry.count.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn trusted_origins_cover_loopback_aliases() {
        let origins = trusted_loopback_origins(SocketAddr::from(([127, 0, 0, 1], 4317)));
        assert!(origins.iter().any(|o| o == "http://127.0.0.1:4317"));
        assert!(origins.iter().any(|o| o == "http://localhost:4317"));
        assert!(origins.iter().any(|o| o == "http://[::1]:4317"));
    }

    #[test]
    fn trusted_origins_include_non_canonical_loopback_bind() {
        let origins = trusted_loopback_origins(SocketAddr::from(([127, 0, 0, 2], 4317)));
        assert!(origins.iter().any(|o| o == "http://127.0.0.2:4317"));
        // Do not widen trust to other loopback hosts for a non-canonical bind.
        assert!(!origins.iter().any(|o| o == "http://127.0.0.1:4317"));
        assert!(!origins.iter().any(|o| o == "http://localhost:4317"));
    }

    #[test]
    fn trusted_origins_omit_default_http_port() {
        let origins = trusted_loopback_origins(SocketAddr::from(([127, 0, 0, 1], 80)));
        assert!(origins.iter().any(|o| o == "http://127.0.0.1"));
        assert!(origins.iter().any(|o| o == "http://localhost"));
        assert!(origins.iter().any(|o| o == "http://[::1]"));
        assert!(!origins.iter().any(|o| o.contains(":80")));
    }

    #[test]
    fn origin_header_is_preferred_over_referer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:9"),
        );
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("http://evil.example/path"),
        );
        assert_eq!(
            browser_request_origin(&headers).as_deref(),
            Some("http://127.0.0.1:9")
        );
    }

    #[test]
    fn referer_fallback_extracts_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("http://localhost:4317/app?x=1"),
        );
        assert_eq!(
            browser_request_origin(&headers).as_deref(),
            Some("http://localhost:4317")
        );
    }

    #[test]
    fn missing_origin_fails_closed() {
        let headers = HeaderMap::new();
        let trusted = trusted_loopback_origins(SocketAddr::from(([127, 0, 0, 1], 1)));
        assert!(matches!(
            ensure_trusted_browser_origin(&headers, &trusted),
            Err(OriginGuardError::Missing)
        ));
    }

    #[test]
    fn cross_site_origin_fails_closed() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        let trusted = trusted_loopback_origins(SocketAddr::from(([127, 0, 0, 1], 1)));
        assert!(matches!(
            ensure_trusted_browser_origin(&headers, &trusted),
            Err(OriginGuardError::Untrusted { .. })
        ));
    }

    #[test]
    fn rate_limiter_trips_then_recovers_after_window() {
        let limiter = SessionWriteRateLimiter::new(SessionWriteRateLimitConfig::for_tests(
            2,
            Duration::from_millis(30),
        ));
        assert!(limiter.check_and_record("s1").is_ok());
        assert!(limiter.check_and_record("s1").is_ok());
        assert!(limiter.check_and_record("s1").is_err());
        std::thread::sleep(Duration::from_millis(35));
        assert!(limiter.check_and_record("s1").is_ok());
    }
}
