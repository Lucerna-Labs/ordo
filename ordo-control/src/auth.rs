//! Bearer-token auth middleware (Phase 2.5).
//!
//! Default mode is **Off** â€” single-operator localhost deployments
//! need zero config and no auth. When `AuthConfig::StaticTokens` is
//! set, every request under `/api/*` and `/ws/*` must carry a
//! matching `Authorization: Bearer <token>` header. Two public paths
//! are always allowed: `/` (dashboard) and `/health` (readiness
//! probe) â€” monitors need to reach `/health` without credentials, and
//! the dashboard is an HTML page that loads its own protected
//! resources.
//!
//! OIDC / JWKS validation is deferred to a follow-up â€” the static-
//! token shape is enough for the initial MCP-bridge-from-another-
//! machine use case, and the middleware interface is designed so
//! plugging in JWT validation later doesn't change the router.
//!
//! **Rule 3 compliance:** this file adds no new business logic. It
//! only gates access to existing logic.
//!
//! **Contract side-note:** the "auth-off" configuration is a
//! first-class supported mode with its own integration test â€” the
//! guarantee that future changes can't accidentally require a token.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde::{Deserialize, Serialize};

/// How the control API authenticates requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[derive(Default)]
pub enum AuthConfig {
    /// No authentication. Default. Only safe on a trusted network
    /// (localhost, dev sandbox).
    #[default]
    Off,
    /// Accept any token from the given set. Constant-time comparison
    /// is applied to avoid timing leaks.
    StaticTokens { tokens: Vec<String> },
}

impl AuthConfig {
    /// Parse from env. `ORDO_AUTH_TOKENS` = comma-separated
    /// list; empty / unset means `Off`. Chosen over JSON config so
    /// operators can rotate tokens with a single env edit.
    pub fn from_env() -> Self {
        match std::env::var("ORDO_AUTH_TOKENS") {
            Ok(value) => {
                let tokens: Vec<String> = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if tokens.is_empty() {
                    Self::Off
                } else {
                    Self::StaticTokens { tokens }
                }
            }
            Err(_) => Self::Off,
        }
    }

    pub fn is_enforced(&self) -> bool {
        matches!(self, Self::StaticTokens { .. })
    }
}

/// Shared handle embedded in the router state via `Arc` so cloning
/// the state stays cheap.
pub type AuthHandle = Arc<AuthConfig>;

/// Paths that are reachable without auth even when auth is on.
fn is_public(path: &str) -> bool {
    matches!(path, "/" | "/health")
}

/// Tower middleware that rejects unauthenticated requests when auth
/// is enforced. Install via
/// `Router::layer(axum::middleware::from_fn_with_state(handle, require_auth))`.
pub async fn require_auth<B>(
    axum::extract::State(handle): axum::extract::State<AuthHandle>,
    request: Request<B>,
    next: Next,
) -> Result<Response, StatusCode>
where
    B: Send + 'static,
    Request<B>: Into<Request<axum::body::Body>>,
{
    let path = request.uri().path();
    if is_public(path) {
        return Ok(next.run(request.into()).await);
    }
    match &*handle {
        AuthConfig::Off => Ok(next.run(request.into()).await),
        AuthConfig::StaticTokens { tokens } => {
            let header = request
                .headers()
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok());
            let presented = header.and_then(extract_bearer);
            match presented {
                Some(token) if accept(tokens, token) => Ok(next.run(request.into()).await),
                _ => Err(StatusCode::UNAUTHORIZED),
            }
        }
    }
}

fn extract_bearer(header: &str) -> Option<&str> {
    let rest = header.strip_prefix("Bearer ")?;
    // Strip surrounding whitespace but keep the token itself intact.
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Constant-time comparison so a fast-fail attacker can't profile
/// which byte of a token they got wrong. We accept any token in the
/// configured set; the per-token compare is constant-time, the
/// iteration is not (the token count is public by design).
fn accept(allowed: &[String], presented: &str) -> bool {
    let presented_bytes = presented.as_bytes();
    for token in allowed {
        if ct_eq(token.as_bytes(), presented_bytes) {
            return true;
        }
    }
    false
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bearer_requires_prefix() {
        assert_eq!(extract_bearer("Bearer abc"), Some("abc"));
        assert_eq!(extract_bearer("Bearer    spaced   "), Some("spaced"));
        assert_eq!(extract_bearer("Basic abc"), None);
        assert_eq!(extract_bearer("Bearer"), None);
        assert_eq!(extract_bearer("Bearer "), None);
    }

    #[test]
    fn accept_matches_any_configured_token() {
        let tokens = vec!["alpha".to_string(), "beta".to_string()];
        assert!(accept(&tokens, "alpha"));
        assert!(accept(&tokens, "beta"));
        assert!(!accept(&tokens, "gamma"));
    }

    #[test]
    fn public_paths_bypass_enforcement() {
        assert!(is_public("/"));
        assert!(is_public("/health"));
        assert!(!is_public("/api/apps"));
        assert!(!is_public("/ws/assistant/xyz"));
    }

    #[test]
    fn ct_eq_catches_length_mismatch_and_diff() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }

    // ---- Integration tests over a minimal router -------------------
    //
    // These verify the contract: "auth off" is first-class tested,
    // and "auth on" actually blocks unauthenticated requests. If a
    // future PR flips the default or silently conflates the two,
    // these fail.

    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn tiny_router(config: AuthConfig) -> Router {
        let base = Router::new()
            .route("/", get(|| async { "dashboard" }))
            .route("/health", get(|| async { "ok" }))
            .route("/api/hello", get(|| async { "hello" }));
        crate::with_auth(base, config)
    }

    async fn status_of(router: Router, uri: &str, auth: Option<&str>) -> axum::http::StatusCode {
        use axum::body::Body;
        use axum::http::Request;
        let mut req = Request::get(uri);
        if let Some(value) = auth {
            req = req.header(axum::http::header::AUTHORIZATION, value);
        }
        let response = router
            .oneshot(req.body(Body::empty()).expect("req"))
            .await
            .expect("response");
        response.status()
    }

    #[tokio::test]
    async fn auth_off_allows_everything_including_api() {
        let router = tiny_router(AuthConfig::Off);
        assert_eq!(status_of(router.clone(), "/", None).await, StatusCode::OK);
        assert_eq!(
            status_of(router.clone(), "/health", None).await,
            StatusCode::OK
        );
        assert_eq!(status_of(router, "/api/hello", None).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_on_permits_public_paths_without_token() {
        let router = tiny_router(AuthConfig::StaticTokens {
            tokens: vec!["secret".into()],
        });
        // Dashboard + health stay public so monitors and the studio
        // pre-auth UI shell can still reach them.
        assert_eq!(status_of(router.clone(), "/", None).await, StatusCode::OK);
        assert_eq!(status_of(router, "/health", None).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_on_rejects_missing_or_wrong_token() {
        let router = tiny_router(AuthConfig::StaticTokens {
            tokens: vec!["secret".into()],
        });
        assert_eq!(
            status_of(router.clone(), "/api/hello", None).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(router.clone(), "/api/hello", Some("Bearer wrong")).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(router, "/api/hello", Some("Basic secret")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn auth_on_accepts_any_configured_token() {
        let router = tiny_router(AuthConfig::StaticTokens {
            tokens: vec!["alpha".into(), "beta".into()],
        });
        assert_eq!(
            status_of(router.clone(), "/api/hello", Some("Bearer alpha")).await,
            StatusCode::OK
        );
        assert_eq!(
            status_of(router, "/api/hello", Some("Bearer beta")).await,
            StatusCode::OK
        );
    }

    #[test]
    fn from_env_parses_comma_list() {
        let original = std::env::var("ORDO_AUTH_TOKENS").ok();
        std::env::set_var("ORDO_AUTH_TOKENS", "one, two,  three ");
        match AuthConfig::from_env() {
            AuthConfig::StaticTokens { tokens } => {
                assert_eq!(tokens, vec!["one", "two", "three"]);
            }
            other => panic!("expected StaticTokens, got {other:?}"),
        }
        std::env::set_var("ORDO_AUTH_TOKENS", "");
        assert!(matches!(AuthConfig::from_env(), AuthConfig::Off));
        match original {
            Some(v) => std::env::set_var("ORDO_AUTH_TOKENS", v),
            None => std::env::remove_var("ORDO_AUTH_TOKENS"),
        }
    }
}
