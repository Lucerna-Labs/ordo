#![cfg_attr(windows, windows_subsystem = "windows")]

mod backend;
mod types;

use backend::{
    delete_local_plugin, delete_local_skill, detect_local_llm, emit_log, find_local_binary,
    get_local_health, get_local_runtime_profile, get_local_runtime_settings,
    get_local_runtime_storage, get_local_session_taint, get_local_skill,
    init_new_crate, install_local_api_key_env, install_local_plugin, list_local_apps,
    list_local_assistant_facts, list_local_capabilities, list_local_cloud_credentials,
    list_local_connection_types, list_local_files, list_local_mcp_capabilities,
    list_local_mcp_servers, list_local_modes, list_local_pinned_memory, list_local_plugins,
    list_local_rag_collections, list_local_review_pending, list_local_review_recent,
    list_local_security_audit, list_local_security_rules, list_local_self_heal_cases,
    list_local_webhooks, list_local_working_memory, preview_local_rag_collections,
    set_local_plugin_enabled, update_local_plugin,
    update_local_skill, StudioState,
};
use types::LogLevel;

fn main() {
    tauri::Builder::default()
        // Native file dialogs (file picker for the MCP tab's
        // "Browse…" button). Plugin must be initialized before the
        // window opens so frontend invokes resolve cleanly.
        .plugin(tauri_plugin_dialog::init())
        .manage(StudioState::default())
        .setup(|app| {
            let app_handle = app.handle().clone();
            emit_log(
                &app_handle,
                "SHELL",
                "Ordo Studio online. Liquid Glass 2026 shell aligned.",
                LogLevel::Info,
            )?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            init_new_crate,
            list_local_modes,
            list_local_plugins,
            install_local_plugin,
            update_local_plugin,
            set_local_plugin_enabled,
            delete_local_plugin,
            get_local_skill,
            update_local_skill,
            delete_local_skill,
            list_local_mcp_servers,
            list_local_mcp_capabilities,
            list_local_capabilities,
            get_local_runtime_profile,
            get_local_runtime_storage,
            get_local_runtime_settings,
            list_local_rag_collections,
            preview_local_rag_collections,
            list_local_pinned_memory,
            list_local_working_memory,
            detect_local_llm,
            install_local_api_key_env,
            list_local_cloud_credentials,
            list_local_webhooks,
            list_local_connection_types,
            list_local_apps,
            list_local_files,
            list_local_security_rules,
            list_local_security_audit,
            list_local_review_pending,
            list_local_review_recent,
            list_local_self_heal_cases,
            list_local_assistant_facts,
            get_local_session_taint,
            find_local_binary,
            get_local_health
        ])
        .run(tauri::generate_context!())
        .expect("error while running Ordo Studio");
}
