use std::sync::Arc;
use std::sync::OnceLock;
use std::collections::HashMap;
use axum::extract::State;
use axum::response::{Html, Json};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::{ControlApiError, ControlApiState};

/// The boot progress HTML page.
pub(crate) const BOOT_PROGRESS_HTML: &str = include_str!("boot_progress.html");

/// Boot state data — mirrors ordo_runtime::BootStateData but lives here
/// to avoid a circular dependency (ordo-control can't depend on ordo-runtime).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct BootStateData {
    pub steps: HashMap<String, String>,
    pub subtitle: String,
    pub status_text: String,
    pub error: Option<String>,
    pub all_done: bool,
}

pub type BootState = Arc<Mutex<BootStateData>>;

static BOOT_STATE: OnceLock<BootState> = OnceLock::new();

/// Set the global boot state (called once during startup by ordo-cli).
pub fn set_boot_state(state: BootState) {
    let _ = BOOT_STATE.set(state);
}

/// `GET /boot` — serves the boot progress page.
pub(crate) async fn boot_progress_page() -> Html<&'static str> {
    Html(BOOT_PROGRESS_HTML)
}

/// `GET /api/boot/status` — returns the current boot step states.
pub(crate) async fn boot_status(
    State(_state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    match BOOT_STATE.get() {
        Some(state) => {
            let bs = state.lock().await;
            Ok(Json(json!({
                "steps": bs.steps,
                "subtitle": bs.subtitle,
                "status_text": bs.status_text,
                "error": bs.error,
                "all_done": bs.all_done,
            })))
        }
        None => {
            // No boot state — plain `ordo serve` without the integrated launcher.
            let mut steps = HashMap::new();
            for s in &["build_studio", "build_runtime", "build_servo", "start_runtime", "open_window"] {
                steps.insert(s.to_string(), "done".to_string());
            }
            Ok(Json(json!({
                "steps": steps,
                "subtitle": "Ordo is running",
                "status_text": "",
                "error": null,
                "all_done": true,
            })))
        }
    }
}
