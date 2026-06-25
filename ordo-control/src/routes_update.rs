use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::{ControlApiError, ControlApiState};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_REPO: &str = "Lucerna-Labs/ordo";

/// Compare two semver-like strings (e.g. "0.1.0" vs "0.1.3").
/// Returns true if `remote` is newer than `local`.
fn is_newer(local: &str, remote: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .filter_map(|n| n.trim().parse::<u64>().ok())
            .collect()
    };
    let l = parse(local);
    let r = parse(remote);
    for i in 0..l.len().max(r.len()) {
        let li = l.get(i).unwrap_or(&0);
        let ri = r.get(i).unwrap_or(&0);
        if ri > li {
            return true;
        }
        if ri < li {
            return false;
        }
    }
    false
}

/// `GET /api/update/check` — queries GitHub for the latest release and
/// returns whether an update is available.
pub(crate) async fn check_for_update(
    State(_state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::Client::builder()
        .user_agent("ordo-update-checker")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ControlApiError::internal(&format!("HTTP client error: {e}")))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| ControlApiError::internal(&format!("GitHub API request failed: {e}")))?;

    if !resp.status().is_success() {
        // Don't fail hard — just report "no info"
        return Ok(Json(json!({
            "current": CURRENT_VERSION,
            "latest": null,
            "update_available": false,
            "error": format!("GitHub API returned {}", resp.status()),
        })));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| ControlApiError::internal(&format!("Failed to parse GitHub response: {e}")))?;

    let latest_tag = body["tag_name"].as_str().unwrap_or("").to_string();
    let latest_version = latest_tag.trim_start_matches('v').to_string();

    // Extract release notes (body) and HTML URL
    let release_notes = body["body"].as_str().unwrap_or("").to_string();
    let release_url = body["html_url"].as_str().unwrap_or("").to_string();

    // Find download assets for the user's platform
    let assets: Vec<Value> = body["assets"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let platform_asset = if cfg!(target_os = "linux") {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str()?;
            if name.ends_with("_amd64.deb") || name.ends_with(".deb") {
                Some(json!({
                    "name": name,
                    "url": a["browser_download_url"],
                    "size": a["size"],
                }))
            } else {
                None
            }
        })
    } else {
        // Windows: no prebuilt binary yet, so null — user updates via git pull
        None
    };

    let update_available = is_newer(CURRENT_VERSION, &latest_version);

    Ok(Json(json!({
        "current": CURRENT_VERSION,
        "latest": latest_version,
        "latest_tag": latest_tag,
        "update_available": update_available,
        "release_url": release_url,
        "release_notes": release_notes,
        "platform_asset": platform_asset,
    })))
}

/// `POST /api/update/apply` — runs the update for the current platform.
///
/// On Windows/macOS (source install): runs `git pull` in the workspace,
/// rebuilds Studio, and returns. The user then restarts Ordo.
///
/// On Linux (.deb): downloads the new .deb and installs it via dpkg.
pub(crate) async fn apply_update(
    State(state): State<ControlApiState>,
) -> Result<Json<Value>, ControlApiError> {
    // First, check what's available
    let check_url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::Client::builder()
        .user_agent("ordo-update-checker")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ControlApiError::internal(&format!("HTTP client error: {e}")))?;

    let resp = client
        .get(&check_url)
        .send()
        .await
        .map_err(|e| ControlApiError::internal(&format!("GitHub API request failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(ControlApiError::internal(
            "Could not reach GitHub to check for the latest release.",
        ));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| ControlApiError::internal(&format!("Failed to parse GitHub response: {e}")))?;

    let latest_tag = body["tag_name"].as_str().unwrap_or("").to_string();
    let latest_version = latest_tag.trim_start_matches('v').to_string();

    if !is_newer(CURRENT_VERSION, &latest_version) {
        return Ok(Json(json!({
            "updated": false,
            "message": "Already running the latest version.",
        })));
    }

    // ── Platform-specific update path ──────────────────────────

    #[cfg(target_os = "linux")]
    {
        // Find the .deb asset
        let assets = body["assets"].as_array();
        let deb_url = assets.and_then(|a| {
            a.iter().find_map(|asset| {
                let name = asset["name"].as_str()?;
                if name.ends_with("_amd64.deb") || name.ends_with(".deb") {
                    Some(asset["browser_download_url"].as_str()?.to_string())
                } else {
                    None
                }
            })
        });

        if let Some(url) = deb_url {
            // Download to temp
            let tmp = std::env::temp_dir().join(format!("ordo-update-{}.deb", latest_version));
            let download_resp = client
                .get(&url)
                .send()
                .await
                .map_err(|e| ControlApiError::internal(&format!("Download failed: {e}")))?;

            let bytes = download_resp
                .bytes()
                .await
                .map_err(|e| ControlApiError::internal(&format!("Failed to read download: {e}")))?;

            std::fs::write(&tmp, &bytes)
                .map_err(|e| ControlApiError::internal(&format!("Failed to write .deb: {e}")))?;

            // Install via dpkg
            let output = Command::new("sudo")
                .args(["dpkg", "-i", tmp.to_str().unwrap()])
                .output()
                .await
                .map_err(|e| ControlApiError::internal(&format!("dpkg failed to start: {e}")))?;

            let _ = std::fs::remove_file(&tmp);

            if !output.status.success() {
                // Try dependency repair
                let _ = Command::new("sudo")
                    .args(["apt-get", "install", "-f", "-y"])
                    .output()
                    .await;

                let retry = Command::new("sudo")
                    .args(["dpkg", "-i", tmp.to_str().unwrap()])
                    .output()
                    .await;

                if retry.map(|o| !o.status.success()).unwrap_or(true) {
                    return Err(ControlApiError::internal(
                        "Package install failed. Try manually: sudo dpkg -i ordo_*.deb",
                    ));
                }
            }

            return Ok(Json(json!({
                "updated": true,
                "old_version": CURRENT_VERSION,
                "new_version": latest_version,
                "message": "Update installed. Restart Ordo to complete.",
            })));
        }
    }

    // ── Source install path (Windows, or Linux without .deb) ──
    // git pull + npm build in the workspace root.
    let workspace = std::env::current_dir()
        .map_err(|e| ControlApiError::internal(&format!("Cannot determine workspace: {e}")))?;

    // git pull
    let git_output = Command::new("git")
        .arg("pull")
        .arg("--ff-only")
        .current_dir(&workspace)
        .output()
        .await
        .map_err(|e| ControlApiError::internal(&format!("git pull failed: {e}")))?;

    if !git_output.status.success() {
        let stderr = String::from_utf8_lossy(&git_output.stderr);
        return Err(ControlApiError::internal(&format!(
            "git pull failed: {stderr}"
        )));
    }

    // Rebuild Studio
    let studio_dir = workspace.join("ordo-studio");
    let npm_ci = Command::new("npm")
        .arg("ci")
        .current_dir(&studio_dir)
        .output()
        .await
        .map_err(|e| ControlApiError::internal(&format!("npm ci failed: {e}")))?;

    let npm_build = Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(&studio_dir)
        .output()
        .await
        .map_err(|e| ControlApiError::internal(&format!("npm run build failed: {e}")))?;

    if !npm_build.status.success() {
        let stderr = String::from_utf8_lossy(&npm_build.stderr);
        return Err(ControlApiError::internal(&format!(
            "Studio build failed: {stderr}"
        )));
    }

    Ok(Json(json!({
        "updated": true,
        "old_version": CURRENT_VERSION,
        "new_version": latest_version,
        "message": "Update applied. Restart Ordo to complete.",
    })))
}
