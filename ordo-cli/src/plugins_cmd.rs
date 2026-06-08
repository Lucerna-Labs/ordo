//! `ordo plugins` subcommands â€” operator-side plugin management.
//!
//! Every command here operates on the **manifest directory** (default
//! `user-files/plugins/`) rather than live subprocesses, so they are
//! safe to run while the runtime is up. Enable/disable flip the
//! manifest's `enabled` flag; the change takes effect on next runtime
//! boot (or `ordo runtime reload-plugins`, which forwards through the
//! control API).

use std::path::{Path, PathBuf};

use ordo_plugins::{discover_plugins, LoadedManifest, PluginManifest};
use ordo_runtime::RuntimeConfig;
use serde_json::{json, Value};

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn plugins_dir() -> PathBuf {
    RuntimeConfig::local_default().plugins_path
}

pub fn run(args: &[String]) -> Result<(), DynError> {
    match args.first().map(String::as_str) {
        Some("list") | None => run_list(),
        Some("enable") => run_set_enabled(&args[1..], true),
        Some("disable") => run_set_enabled(&args[1..], false),
        Some("install") => run_install(&args[1..]),
        Some("uninstall") | Some("remove") | Some("rm") => run_uninstall(&args[1..]),
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown `ordo plugins` subcommand: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Usage:\n  \
         ordo plugins list\n  \
         ordo plugins enable <name>\n  \
         ordo plugins disable <name>\n  \
         ordo plugins install <source-dir> [--name <override>]\n  \
         ordo plugins uninstall <name>\n\n\
         Default plugin directory: {}\n\
         Override with ORDO_PLUGINS_PATH.",
        plugins_dir().display()
    );
}

fn run_list() -> Result<(), DynError> {
    let dir = plugins_dir();
    let report = discover_plugins(&dir);
    let plugins: Vec<Value> = report
        .loaded
        .iter()
        .map(|loaded| {
            json!({
                "name": loaded.manifest.name,
                "version": loaded.manifest.version,
                "enabled": loaded.manifest.enabled,
                "description": loaded.manifest.description,
                "expected_lanes": loaded.manifest.expected_lanes,
                "manifest_path": loaded.manifest_path.display().to_string(),
            })
        })
        .collect();
    let errors: Vec<Value> = report
        .errors
        .iter()
        .map(|err| {
            json!({
                "manifest_path": err.path.display().to_string(),
                "error": err.error,
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "plugins_dir": dir.display().to_string(),
            "count": plugins.len(),
            "plugins": plugins,
            "errors": errors,
        }))?
    );
    Ok(())
}

fn run_set_enabled(args: &[String], enabled: bool) -> Result<(), DynError> {
    let name = args
        .first()
        .ok_or_else(|| "expected plugin name".to_string())?
        .clone();
    let manifest_path = locate_manifest(&name)?;
    let mut manifest = parse_manifest(&manifest_path)?;
    if manifest.enabled == enabled {
        println!("plugin '{name}' already {}", state_word(enabled));
        return Ok(());
    }
    manifest.enabled = enabled;
    write_manifest(&manifest_path, &manifest)?;
    println!(
        "plugin '{name}' {} (manifest: {})",
        state_word(enabled),
        manifest_path.display()
    );
    println!("(restart the runtime to apply changes)");
    Ok(())
}

fn state_word(enabled: bool) -> &'static str {
    if enabled {
        "enabled"
    } else {
        "disabled"
    }
}

fn run_install(args: &[String]) -> Result<(), DynError> {
    // `ordo plugins install <source_dir> [--name <override>]`
    // Copies <source_dir> into user-files/plugins/<name>/, preserving
    // the manifest. Designed for local dev; URL-based install comes
    // in a future phase.
    let source = args
        .first()
        .ok_or_else(|| "expected <source-dir> path to a plugin directory".to_string())?;
    let source_path = Path::new(source)
        .canonicalize()
        .map_err(|err| format!("cannot resolve source directory '{source}': {err}"))?;
    let source_manifest = source_path.join("plugin.json");
    let manifest = parse_manifest(&source_manifest)?;

    let mut install_name = manifest.name.clone();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--name" {
            i += 1;
            install_name = args
                .get(i)
                .cloned()
                .ok_or_else(|| "--name expected a value".to_string())?;
        } else {
            return Err(format!("unknown flag: {}", args[i]).into());
        }
        i += 1;
    }
    if !install_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("invalid install name '{install_name}'").into());
    }

    let dest_root = plugins_dir();
    std::fs::create_dir_all(&dest_root)?;
    let dest = dest_root.join(&install_name);
    if dest.exists() {
        return Err(format!(
            "plugin '{install_name}' is already installed at {}",
            dest.display()
        )
        .into());
    }
    copy_dir_recursive(&source_path, &dest)?;

    // Security: newly installed plugins land *disabled*. The operator
    // must explicitly `ordo plugins enable <name>` after reviewing the
    // manifest, which forces a conscious "I trust this code to run in
    // my workspace" decision.
    let installed_manifest_path = dest.join("plugin.json");
    let mut installed_manifest = parse_manifest(&installed_manifest_path)?;
    installed_manifest.enabled = false;
    write_manifest(&installed_manifest_path, &installed_manifest)?;

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "installed": install_name,
            "destination": dest.display().to_string(),
            "expected_lanes": manifest.expected_lanes,
            "enabled": false,
            "next_step": format!("ordo plugins enable {install_name}"),
        }))?
    );
    println!("(plugin installed in disabled state â€” review the manifest, then `ordo plugins enable` to activate)");
    Ok(())
}

fn run_uninstall(args: &[String]) -> Result<(), DynError> {
    let name = args
        .first()
        .ok_or_else(|| "expected plugin name".to_string())?;
    let plugin_root = plugins_dir().join(name);
    if !plugin_root.exists() {
        return Err(format!("no plugin '{name}' at {}", plugin_root.display()).into());
    }
    std::fs::remove_dir_all(&plugin_root)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "uninstalled": name,
            "removed": plugin_root.display().to_string(),
        }))?
    );
    Ok(())
}

fn locate_manifest(name: &str) -> Result<PathBuf, DynError> {
    let dir = plugins_dir();
    let report = discover_plugins(&dir);
    for loaded in report.loaded {
        if loaded.manifest.name == name {
            return Ok(loaded.manifest_path);
        }
    }
    Err(format!("no plugin named '{name}' under {}", dir.display()).into())
}

fn parse_manifest(path: &Path) -> Result<PluginManifest, DynError> {
    let raw =
        std::fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let manifest: PluginManifest =
        serde_json::from_str(&raw).map_err(|err| format!("parse {}: {err}", path.display()))?;
    Ok(manifest)
}

fn write_manifest(path: &Path, manifest: &PluginManifest) -> Result<(), DynError> {
    let raw = serde_json::to_string_pretty(manifest)?;
    std::fs::write(path, raw)?;
    Ok(())
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), DynError> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else if file_type.is_file() {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn _unused_loaded_manifest(_: LoadedManifest) {}
