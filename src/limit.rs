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

/// Client address behind nginx/Cloudflare: first X-Forwarded-For entry, then X-Real-IP,
/// then the socket address.
pub fn client_ip(headers: &HeaderMap, addr: Option<SocketAddr>) -> String {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
    header("x-forwarded-for")
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .or_else(|| header("x-real-ip").map(str::to_string))
        .or_else(|| addr.map(|a| a.ip().to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

pub async fn limit(State(limiter): State<&'static Limiter>, req: Request, next: Next) -> Response {
    let addr = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = client_ip(req.headers(), addr);
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
