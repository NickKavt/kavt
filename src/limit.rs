//! Per-IP request limits, the same three buckets the original server used:
//! login, data changes and reads. Each allows a burst, then one request per period.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub struct Limiter {
    burst: u32,
    period: Duration,
    /// Per client: the time at which the bucket will be full again (GCRA "theoretical arrival time").
    state: Mutex<HashMap<String, Instant>>,
}

/// The addresses of the proxies whose `X-Forwarded-For` is believed, from `Config`.
#[derive(Clone)]
pub struct TrustedProxies(pub std::sync::Arc<Vec<String>>);

impl Limiter {
    pub fn new(burst: u32, period: Duration) -> Self {
        Self { burst, period, state: Mutex::new(HashMap::new()) }
    }

    /// Ok if the request may pass, otherwise the number of seconds to wait.
    fn check(&self, key: &str) -> Result<(), u64> {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        if state.len() > 50_000 {
            state.retain(|_, tat| *tat > now);
        }
        let tat = state.get(key).copied().unwrap_or(now).max(now);
        let window = self.period * self.burst;
        let next = tat + self.period;
        if next - now > window {
            let wait = (next - window) - now;
            return Err(wait.as_secs_f64().ceil().max(1.0) as u64);
        }
        state.insert(key.to_string(), next);
        Ok(())
    }
}

/// Client address behind nginx/Cloudflare. `X-Forwarded-For` is believed only when the
/// connection comes from a proxy named in `BUDGET_TRUSTED_PROXIES`: anyone else could send a
/// made-up address and get a fresh limit for every request. From a proxy the last entry is the
/// one it added, which is the client; the entries before it are whatever the client sent.
pub fn client_ip(headers: &HeaderMap, addr: Option<SocketAddr>, trusted: &[String]) -> String {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
    let peer = addr.map(|a| a.ip().to_string());
    let from_proxy = peer.as_deref().is_some_and(|ip| trusted.iter().any(|t| t == ip));
    if from_proxy {
        if let Some(ip) = header("x-forwarded-for").and_then(|v| v.rsplit(',').next()).map(str::trim).filter(|v| !v.is_empty()) {
            return ip.to_string();
        }
        if let Some(ip) = header("x-real-ip") {
            return ip.to_string();
        }
    }
    peer.unwrap_or_else(|| "unknown".to_string())
}

pub async fn limit(State(limiter): State<&'static Limiter>, req: Request, next: Next) -> Response {
    let addr = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let trusted = req.extensions().get::<TrustedProxies>().map(|t| t.0.as_slice()).unwrap_or(&[]);
    let ip = client_ip(req.headers(), addr, trusted);
    match limiter.check(&ip) {
        Ok(()) => next.run(req).await,
        Err(wait) => {
            let mut res = (StatusCode::TOO_MANY_REQUESTS, format!("Too Many Requests! Wait for {wait}s")).into_response();
            res.headers_mut().remove(axum::http::header::CONTENT_TYPE);
            res.headers_mut().insert("x-ratelimit-after", HeaderValue::from(wait));
            res
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn peer(ip: &str) -> Option<SocketAddr> {
        Some(format!("{ip}:1234").parse().unwrap())
    }

    fn headers(xff: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = xff {
            h.insert("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn a_made_up_forwarded_for_is_ignored_from_a_stranger() {
        let trusted: Vec<String> = vec![];
        assert_eq!(client_ip(&headers(Some("1.2.3.4")), peer("10.0.0.9"), &trusted), "10.0.0.9");
    }

    #[test]
    fn the_proxy_adds_the_last_entry_so_that_one_is_the_client() {
        let trusted = vec!["10.0.0.9".to_string()];
        assert_eq!(client_ip(&headers(Some("6.6.6.6, 1.2.3.4")), peer("10.0.0.9"), &trusted), "1.2.3.4");
    }

    #[test]
    fn without_a_header_the_socket_address_is_used() {
        let trusted = vec!["10.0.0.9".to_string()];
        assert_eq!(client_ip(&headers(None), peer("10.0.0.9"), &trusted), "10.0.0.9");
    }
}
