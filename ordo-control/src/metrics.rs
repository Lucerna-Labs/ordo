//! Prometheus-format /metrics endpoint + per-IP rate limiter (Phase 4.6).
//!
//! Kept dependency-free intentionally: `prometheus` and `governor`
//! both pull significant dep trees and our telemetry needs are modest
//! at this stage. The output is **text-format Prometheus** (the
//! historic v0 exposition format), which every scraper accepts.
//!
//! Metrics tracked:
//!   - `ordo_http_requests_total{status}` â€” per-status
//!     request counter (2xx, 4xx, 5xx aggregated bands)
//!   - `ordo_http_in_flight` â€” current in-flight
//!   - `ordo_rate_limited_total` â€” 429s handed out
//!   - `ordo_build_info{version}` â€” 1 gauge with version
//!     label for build tracking
//!
//! Rate limiting: simple token-bucket-ish sliding window per source
//! IP. 60 requests per 10s by default; configurable via env. Reached
//! for requests under `/api/*` and `/ws/*` â€” the dashboard and
//! `/metrics` itself stay unlimited.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;

/// Per-status aggregated counter. Keeps cardinality low (we don't
/// want one label value per HTTP status code in the output).
#[derive(Default)]
struct CounterGroup {
    ok_2xx: u64,
    client_err_4xx: u64,
    server_err_5xx: u64,
    other: u64,
}

struct MetricsInner {
    started_at: Instant,
    requests: Mutex<CounterGroup>,
    in_flight: Mutex<u64>,
    rate_limited: Mutex<u64>,
}

#[derive(Clone)]
pub struct MetricsHandle {
    inner: Arc<MetricsInner>,
}

impl MetricsHandle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MetricsInner {
                started_at: Instant::now(),
                requests: Mutex::new(CounterGroup::default()),
                in_flight: Mutex::new(0),
                rate_limited: Mutex::new(0),
            }),
        }
    }

    pub fn record(&self, status: StatusCode) {
        let mut c = self.inner.requests.lock();
        let code = status.as_u16();
        match code {
            200..=299 => c.ok_2xx += 1,
            400..=499 => c.client_err_4xx += 1,
            500..=599 => c.server_err_5xx += 1,
            _ => c.other += 1,
        }
    }

    pub fn inc_in_flight(&self) {
        let mut g = self.inner.in_flight.lock();
        *g = g.saturating_add(1);
    }

    pub fn dec_in_flight(&self) {
        let mut g = self.inner.in_flight.lock();
        *g = g.saturating_sub(1);
    }

    pub fn record_rate_limited(&self) {
        let mut g = self.inner.rate_limited.lock();
        *g += 1;
    }

    /// Render the Prometheus text-format snapshot.
    pub fn render(&self) -> String {
        let requests = self.inner.requests.lock();
        let in_flight = *self.inner.in_flight.lock();
        let rate_limited = *self.inner.rate_limited.lock();
        let uptime = self.inner.started_at.elapsed().as_secs();
        format!(
            "# HELP ordo_http_requests_total HTTP requests by status class.\n\
             # TYPE ordo_http_requests_total counter\n\
             ordo_http_requests_total{{status=\"2xx\"}} {ok}\n\
             ordo_http_requests_total{{status=\"4xx\"}} {c4}\n\
             ordo_http_requests_total{{status=\"5xx\"}} {c5}\n\
             ordo_http_requests_total{{status=\"other\"}} {other}\n\
             # HELP ordo_http_in_flight In-flight HTTP requests.\n\
             # TYPE ordo_http_in_flight gauge\n\
             ordo_http_in_flight {inflight}\n\
             # HELP ordo_rate_limited_total Requests rejected with 429.\n\
             # TYPE ordo_rate_limited_total counter\n\
             ordo_rate_limited_total {rl}\n\
             # HELP ordo_uptime_seconds Process uptime.\n\
             # TYPE ordo_uptime_seconds gauge\n\
             ordo_uptime_seconds {up}\n\
             # HELP ordo_build_info Build info labels.\n\
             # TYPE ordo_build_info gauge\n\
             ordo_build_info{{version=\"{version}\"}} 1\n",
            ok = requests.ok_2xx,
            c4 = requests.client_err_4xx,
            c5 = requests.server_err_5xx,
            other = requests.other,
            inflight = in_flight,
            rl = rate_limited,
            up = uptime,
            version = env!("CARGO_PKG_VERSION"),
        )
    }
}

impl Default for MetricsHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Sliding-window rate limiter. Per-source-IP bucket of
/// (allowance, window_start). Entries expire after `window * 2` to
/// keep memory bounded under long-lived clients.
#[derive(Clone)]
pub struct RateLimiterHandle {
    inner: Arc<RateLimiterInner>,
}

struct RateLimiterInner {
    limit: u64,
    window: Duration,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

struct Bucket {
    count: u64,
    window_start: Instant,
}

impl RateLimiterHandle {
    pub fn new(limit: u64, window: Duration) -> Self {
        Self {
            inner: Arc::new(RateLimiterInner {
                limit,
                window,
                buckets: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn from_env() -> Self {
        let limit = std::env::var("ORDO_RATELIMIT_RPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        let window_secs = std::env::var("ORDO_RATELIMIT_WINDOW_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        Self::new(limit, Duration::from_secs(window_secs))
    }

    /// Returns `Ok(())` when the request is within budget, or the
    /// current count on rejection. Evicts stale buckets opportunistically.
    pub fn try_consume(&self, ip: IpAddr) -> Result<(), u64> {
        let mut buckets = self.inner.buckets.lock();
        let now = Instant::now();
        let window = self.inner.window;

        // Opportunistic eviction â€” only when the map gets noticeably
        // populous, to keep contention low on the hot path.
        if buckets.len() > 1024 {
            buckets.retain(|_, b| now.duration_since(b.window_start) < window * 2);
        }

        let entry = buckets.entry(ip).or_insert_with(|| Bucket {
            count: 0,
            window_start: now,
        });
        if now.duration_since(entry.window_start) >= window {
            entry.count = 0;
            entry.window_start = now;
        }
        if entry.count >= self.inner.limit {
            return Err(entry.count);
        }
        entry.count += 1;
        Ok(())
    }
}

fn is_rate_limited_path(path: &str) -> bool {
    // Rate-limit exposed surface area. `/metrics`, `/health`, and the
    // dashboard are explicitly exempt so monitoring and uptime checks
    // aren't self-DoS'd.
    !(path == "/" || path == "/health" || path == "/metrics")
}

/// Tower layer middleware implementing the metrics recording + rate
/// limiting. Install with:
///
/// ```ignore
/// router.layer(axum::middleware::from_fn_with_state(
///     (metrics, limiter),
///     traffic_middleware,
/// ))
/// ```
pub async fn traffic_middleware<B>(
    axum::extract::State((metrics, limiter)): axum::extract::State<(
        MetricsHandle,
        RateLimiterHandle,
    )>,
    ConnectInfo(client): ConnectInfo<std::net::SocketAddr>,
    request: Request<B>,
    next: Next,
) -> Result<Response, Response>
where
    B: Send + 'static,
    Request<B>: Into<Request<axum::body::Body>>,
{
    let path = request.uri().path().to_string();
    if is_rate_limited_path(&path) && limiter.try_consume(client.ip()).is_err() {
        metrics.record_rate_limited();
        metrics.record(StatusCode::TOO_MANY_REQUESTS);
        return Err((StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response());
    }

    metrics.inc_in_flight();
    let response = next.run(request.into()).await;
    metrics.dec_in_flight();
    metrics.record(response.status());
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn render_contains_all_declared_metrics() {
        let m = MetricsHandle::new();
        m.record(StatusCode::OK);
        m.record(StatusCode::NOT_FOUND);
        m.record(StatusCode::INTERNAL_SERVER_ERROR);
        m.inc_in_flight();
        m.record_rate_limited();
        let out = m.render();
        assert!(out.contains("ordo_http_requests_total"));
        assert!(out.contains("status=\"2xx\"} 1"));
        assert!(out.contains("status=\"4xx\"} 1"));
        assert!(out.contains("status=\"5xx\"} 1"));
        assert!(out.contains("ordo_http_in_flight 1"));
        assert!(out.contains("ordo_rate_limited_total 1"));
        assert!(out.contains("ordo_build_info"));
        assert!(out.contains("ordo_uptime_seconds"));
    }

    #[test]
    fn rate_limiter_allows_up_to_limit_then_rejects() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let limiter = RateLimiterHandle::new(3, Duration::from_secs(60));
        for _ in 0..3 {
            limiter.try_consume(ip).expect("within budget");
        }
        assert!(limiter.try_consume(ip).is_err());
    }

    #[test]
    fn rate_limiter_resets_after_window() {
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let limiter = RateLimiterHandle::new(1, Duration::from_millis(50));
        limiter.try_consume(ip).expect("first");
        assert!(limiter.try_consume(ip).is_err());
        std::thread::sleep(Duration::from_millis(75));
        limiter.try_consume(ip).expect("after window");
    }

    #[test]
    fn rate_limiter_is_per_ip() {
        let a = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let b = IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8));
        let limiter = RateLimiterHandle::new(1, Duration::from_secs(60));
        limiter.try_consume(a).expect("a ok");
        limiter.try_consume(b).expect("b still ok");
        assert!(limiter.try_consume(a).is_err());
    }

    #[test]
    fn metrics_path_exempt_from_rate_limit() {
        assert!(!is_rate_limited_path("/metrics"));
        assert!(!is_rate_limited_path("/health"));
        assert!(!is_rate_limited_path("/"));
        assert!(is_rate_limited_path("/api/apps"));
        assert!(is_rate_limited_path("/ws/assistant/x"));
    }
}
