//! Lifecycle management for `ordo serve` — owns the Servo shell child process,
//! tracks boot progress, and ensures cleanup on exit.
//!
//! When `ordo serve` boots, it:
//! 1. Starts the control API (which serves the boot progress page)
//! 2. Builds Studio if needed (npm install + npm run build)
//! 3. Builds the Servo shell if needed (cargo build)
//! 4. Starts the Servo shell as a child process pointing at localhost
//! 5. When Servo exits, the runtime shuts down — no orphaned processes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Re-export ordo_control's boot state types.
pub use ordo_control::{BootState, BootStateData};

/// Create a new boot state initialized with pending steps.
pub fn new_boot_state() -> BootState {
    let mut steps = std::collections::HashMap::new();
    for s in &["build_studio", "build_runtime", "build_servo", "start_runtime", "open_window"] {
        steps.insert(s.to_string(), "pending".to_string());
    }
    Arc::new(Mutex::new(BootStateData {
        steps,
        subtitle: "Preparing your workspace…".to_string(),
        status_text: String::new(),
        error: None,
        all_done: false,
    }))
}

pub struct ServoChild {
    child: Option<Child>,
}

impl ServoChild {
    /// Spawn the Servo shell pointing at the control API URL.
    pub async fn spawn(
        workspace_root: &Path,
        control_url: &str,
        width: u32,
        height: u32,
        boot_state: BootState,
    ) -> Result<Self, String> {
        {
            let mut bs = boot_state.lock().await;
            bs.steps.insert("open_window".into(), "active".into());
            bs.status_text = "Launching Ordo window…".into();
        }

        let exe = find_servo_shell(workspace_root)
            .ok_or_else(|| "Could not find ordo-servo-shell binary".to_string())?;

        let mut cmd = Command::new(&exe);
        cmd.arg("--target").arg(control_url)
           .arg("--width").arg(width.to_string())
           .arg("--height").arg(height.to_string())
           .current_dir(workspace_root);

        // On Windows, the child should NOT create its own console window
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let child = cmd.spawn()
            .map_err(|e| format!("Failed to start Servo shell: {e}"))?;

        {
            let mut bs = boot_state.lock().await;
            bs.steps.insert("open_window".into(), "done".into());
            bs.all_done = true;
            bs.subtitle = "Ordo is running".into();
            bs.status_text = String::new();
        }

        Ok(Self { child: Some(child) })
    }

    /// Wait for the Servo shell to exit.
    pub async fn wait(&mut self) -> Result<i32, String> {
        if let Some(child) = &mut self.child {
            let status = child.wait().await
                .map_err(|e| format!("Servo shell wait failed: {e}"))?;
            Ok(status.code().unwrap_or(-1))
        } else {
            Ok(0)
        }
    }

    /// Forcefully kill the child process.
    pub fn kill(&mut self) {
        if let Some(child) = &mut self.child {
            // start_kill is non-blocking
            let _ = child.start_kill();
        }
    }
}

impl Drop for ServoChild {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Find the ordo-servo-shell binary — checks portable bin, then target/debug.
fn find_servo_shell(root: &Path) -> Option<PathBuf> {
    let ext = if cfg!(windows) { ".exe" } else { "" };

    // 1. Portable bin (from bootstrap zip)
    let portable = root.join("bin").join("portable")
        .join(format!("ordo-servo-shell{ext}"));
    if portable.is_file() {
        return Some(portable);
    }

    // 2. Built from source
    let built = root.join("ordo-servo-shell").join("target").join("debug")
        .join(format!("ordo-servo-shell{ext}"));
    if built.is_file() {
        return Some(built);
    }

    // 3. Release build
    let release = root.join("ordo-servo-shell").join("target").join("release")
        .join(format!("ordo-servo-shell{ext}"));
    if release.is_file() {
        return Some(release);
    }

    None
}

/// Check if Studio UI is already built.
pub fn studio_is_built(root: &Path) -> bool {
    root.join("ordo-studio").join("dist").join("index.html").is_file()
}

/// Build the Studio UI (npm ci + npm run build).
pub async fn build_studio(root: &Path, boot_state: BootState) -> Result<(), String> {
    {
        let mut bs = boot_state.lock().await;
        bs.steps.insert("build_studio".into(), "active".into());
        bs.status_text = "Building Studio interface (npm install + build)…".into();
    }

    let studio_dir = root.join("ordo-studio");

    // npm ci (or npm install)
    let ci = Command::new("npm")
        .arg("ci")
        .current_dir(&studio_dir)
        .output()
        .await
        .map_err(|e| format!("npm ci failed to start: {e}"))?;

    if !ci.status.success() {
        // Fall back to npm install
        let install = Command::new("npm")
            .arg("install")
            .current_dir(&studio_dir)
            .output()
            .await
            .map_err(|e| format!("npm install failed: {e}"))?;

        if !install.status.success() {
            let stderr = String::from_utf8_lossy(&install.stderr);
            let mut bs = boot_state.lock().await;
            bs.steps.insert("build_studio".into(), "error".into());
            bs.error = Some(format!("npm install failed:\n{}", stderr));
            return Err(format!("npm install failed: {stderr}"));
        }
    }

    // npm run build
    let build = Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(&studio_dir)
        .output()
        .await
        .map_err(|e| format!("npm run build failed: {e}"))?;

    if !build.status.success() {
        let stderr = String::from_utf8_lossy(&build.stderr);
        let mut bs = boot_state.lock().await;
        bs.steps.insert("build_studio".into(), "error".into());
        bs.error = Some(format!("Studio build failed:\n{}", stderr));
        return Err(format!("Studio build failed: {stderr}"));
    }

    let mut bs = boot_state.lock().await;
    bs.steps.insert("build_studio".into(), "done".into());
    Ok(())
}

/// Build the Servo shell if not already built.
pub async fn ensure_servo_shell(
    root: &Path,
    boot_state: BootState,
) -> Result<(), String> {
    // Skip if already built
    if find_servo_shell(root).is_some() {
        let mut bs = boot_state.lock().await;
        bs.steps.insert("build_servo".into(), "done".into());
        return Ok(());
    }

    {
        let mut bs = boot_state.lock().await;
        bs.steps.insert("build_servo".into(), "active".into());
        bs.status_text = "Compiling embedded Servo shell (~5-10 min first time)…".into();
    }

    let servo_dir = root.join("ordo-servo-shell");
    let output = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(servo_dir.join("Cargo.toml"))
        .arg("--features")
        .arg("servo-engine")
        .current_dir(root)
        .output()
        .await
        .map_err(|e| format!("cargo build servo-shell failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut bs = boot_state.lock().await;
        bs.steps.insert("build_servo".into(), "error".into());
        bs.error = Some(format!("Servo shell build failed:\n{}", stderr));
        return Err(format!("Servo shell build failed: {stderr}"));
    }

    let mut bs = boot_state.lock().await;
    bs.steps.insert("build_servo".into(), "done".into());
    Ok(())
}

/// Ensure the ANGLE DLLs are present (Windows only).
#[cfg(target_os = "windows")]
pub fn ensure_angle_dlls(root: &Path) -> Result<(), String> {
    let servo_shell_exe = find_servo_shell(root)
        .ok_or("Servo shell not found")?;
    let target_dir = servo_shell_exe.parent()
        .ok_or("Cannot determine Servo target dir")?;

    let dlls = ["libEGL.dll", "libGLESv2.dll"];
    for dll in &dlls {
        if !target_dir.join(dll).exists() {
            // Check if they exist in the servo nightly dir
            let nightly = root.join("bin").join("servo-nightly").join("servo");
            if nightly.join(dll).exists() {
                std::fs::copy(nightly.join(dll), target_dir.join(dll))
                    .map_err(|e| format!("Failed to copy {dll}: {e}"))?;
            }
            // If not available locally, Servo will try to download them at runtime
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn ensure_angle_dlls(_root: &Path) -> Result<(), String> {
    Ok(())
}
