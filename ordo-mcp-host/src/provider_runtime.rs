use crate::*;
use crate::helpers::*;

#[derive(Debug, Clone)]
pub struct RuntimePolicySnapshot {
    pub profile: String,
    pub control_api_bind: Option<String>,
    pub rag_enabled: bool,
    pub knowledge_enabled: bool,
    pub rag_activation_mode: String,
    pub knowledge_activation_mode: String,
    pub rag_budget_bytes: usize,
    pub memory_working_budget_bytes: usize,
    pub memory_pinned_budget_bytes: usize,
    pub self_heal_history_budget_bytes: usize,
    pub self_heal_llama_cpp_binary: Option<String>,
    pub self_heal_model_path: Option<String>,
    pub self_heal_model_context_size: usize,
    pub self_heal_model_max_tokens: usize,
    pub self_heal_model_temperature: f32,
    pub llama_cpp_configured: bool,
    pub embedding_backend: String,
    pub embedding_dimensions: usize,
    pub embedding_llama_cpp_binary: Option<String>,
    pub embedding_model_path: Option<String>,
    pub embedding_context_size: usize,
    pub embedding_ollama_url: Option<String>,
    pub embedding_ollama_model: Option<String>,
}

#[derive(Clone)]
pub struct RuntimeInfoProvider {
    snapshot: RuntimePolicySnapshot,
    settings_task: Option<RuntimeSettingsTask>,
}

impl RuntimeInfoProvider {
    pub fn new(snapshot: RuntimePolicySnapshot) -> Self {
        Self {
            snapshot,
            settings_task: None,
        }
    }

    pub fn with_settings_task(
        snapshot: RuntimePolicySnapshot,
        settings_task: RuntimeSettingsTask,
    ) -> Self {
        Self {
            snapshot,
            settings_task: Some(settings_task),
        }
    }

    pub fn with_settings_path(snapshot: RuntimePolicySnapshot, settings_path: PathBuf) -> Self {
        let settings_task = RuntimeSettingsTask::open(settings_path)
            .expect("open runtime settings task for runtime info provider");
        Self::with_settings_task(snapshot, settings_task)
    }

    fn supports_settings_management(&self) -> bool {
        self.settings_task.is_some()
    }

    async fn load_persisted_settings(&self) -> Result<Value, String> {
        let Some(settings_task) = &self.settings_task else {
            return Ok(json!({
                "profile": Value::Null,
                "rag_budget_bytes": Value::Null,
                "memory_working_budget_bytes": Value::Null,
                "memory_pinned_budget_bytes": Value::Null,
                "self_heal_history_budget_bytes": Value::Null,
                "self_heal_llama_cpp_binary": Value::Null,
                "self_heal_model_path": Value::Null,
                "self_heal_model_context_size": Value::Null,
                "self_heal_model_max_tokens": Value::Null,
                "self_heal_model_temperature": Value::Null,
                "embedding_llama_cpp_binary": Value::Null,
                "embedding_model_path": Value::Null,
                "embedding_dimensions": Value::Null,
                "embedding_context_size": Value::Null,
                "embedding_ollama_url": Value::Null,
                "embedding_ollama_model": Value::Null,
            }));
        };

        let settings = settings_task
            .load()
            .await
            .map_err(|err| format!("failed to load runtime settings: {err}"))?;
        Ok(runtime_settings_json(&settings))
    }

    async fn persist_settings_update(&self, arguments: &Value) -> Result<Value, String> {
        let Some(settings_task) = &self.settings_task else {
            return Err("runtime settings persistence is not configured".to_string());
        };

        let profile = arguments
            .get("profile")
            .map(parse_runtime_profile_argument)
            .transpose()?;
        let rag_budget_bytes = parse_runtime_budget_argument(arguments, "rag_budget_bytes")?;
        let memory_working_budget_bytes =
            parse_runtime_budget_argument(arguments, "memory_working_budget_bytes")?;
        let memory_pinned_budget_bytes =
            parse_runtime_budget_argument(arguments, "memory_pinned_budget_bytes")?;
        let self_heal_history_budget_bytes =
            parse_runtime_budget_argument(arguments, "self_heal_history_budget_bytes")?;
        let self_heal_llama_cpp_binary =
            parse_runtime_optional_string_argument(arguments, "self_heal_llama_cpp_binary")?;
        let self_heal_model_path =
            parse_runtime_optional_string_argument(arguments, "self_heal_model_path")?;
        let self_heal_model_context_size =
            parse_runtime_budget_argument(arguments, "self_heal_model_context_size")?;
        let self_heal_model_max_tokens =
            parse_runtime_budget_argument(arguments, "self_heal_model_max_tokens")?;
        let self_heal_model_temperature =
            parse_runtime_f32_string_argument(arguments, "self_heal_model_temperature")?;

        let embedding_llama_cpp_binary =
            parse_runtime_optional_string_argument(arguments, "embedding_llama_cpp_binary")?;
        let embedding_model_path =
            parse_runtime_optional_string_argument(arguments, "embedding_model_path")?;
        let embedding_dimensions =
            parse_runtime_budget_argument(arguments, "embedding_dimensions")?;
        let embedding_context_size =
            parse_runtime_budget_argument(arguments, "embedding_context_size")?;
        let embedding_ollama_url =
            parse_runtime_optional_string_argument(arguments, "embedding_ollama_url")?;
        let embedding_ollama_model =
            parse_runtime_optional_string_argument(arguments, "embedding_ollama_model")?;

        let update = RuntimeSettingsUpdate {
            profile,
            rag_budget_bytes,
            memory_working_budget_bytes,
            memory_pinned_budget_bytes,
            self_heal_history_budget_bytes,
            self_heal_llama_cpp_binary,
            self_heal_model_path,
            self_heal_model_context_size,
            self_heal_model_max_tokens,
            self_heal_model_temperature,
            embedding_llama_cpp_binary,
            embedding_model_path,
            embedding_dimensions,
            embedding_context_size,
            embedding_ollama_url,
            embedding_ollama_model,
        };

        if update == RuntimeSettingsUpdate::default() {
            // Client sent an empty/no-op update. This is a validation error, not
            // a server fault — phrase it so the control API's classifier
            // (`classify_tool_failure`) maps it to 400, not 500.
            return Err("at least one runtime settings field is required".to_string());
        }

        let persisted = settings_task
            .update(update)
            .await
            .map_err(|err| format!("failed to update runtime settings: {err}"))?;

        Ok(json!({
            "persisted": runtime_settings_json(&persisted),
            "restart_required": true,
        }))
    }
}

#[async_trait]
impl CapabilityProvider for RuntimeInfoProvider {
    fn name(&self) -> &str {
        "runtime"
    }

    fn capabilities(&self) -> Vec<String> {
        let mut capabilities = vec![
            "runtime.describe_profile".to_string(),
            "runtime.describe_storage".to_string(),
        ];
        if self.supports_settings_management() {
            capabilities.push("runtime.describe_settings".to_string());
            capabilities.push("runtime.update_settings".to_string());
        }
        capabilities
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        let mut descriptors = vec![
            CapabilityDescriptor::new(
                "runtime.describe_profile",
                self.name(),
                "Reports the active runtime profile and which optional lanes are enabled.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "runtime.describe_storage",
                self.name(),
                "Reports the current storage and self-heal retention budgets.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
        ];
        if self.supports_settings_management() {
            descriptors.push(CapabilityDescriptor::new(
                "runtime.describe_settings",
                self.name(),
                "Reports persisted runtime settings that a future UI can manage.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ));
            descriptors.push(CapabilityDescriptor::new(
                "runtime.update_settings",
                self.name(),
                "Persists runtime profile and storage settings for the next restart.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ));
        }
        descriptors
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        let lowered = requirement.to_ascii_lowercase();
        if self.supports_settings_management()
            && (lowered.contains("update runtime settings")
                || lowered.contains("change runtime profile")
                || lowered.contains("save storage budget"))
        {
            Some(CapabilityMatch {
                capability: "runtime.update_settings".to_string(),
                description: "Persists runtime profile and storage settings for the next restart."
                    .to_string(),
            })
        } else if self.supports_settings_management()
            && (lowered.contains("runtime settings")
                || lowered.contains("storage settings")
                || lowered.contains("settings ui"))
        {
            Some(CapabilityMatch {
                capability: "runtime.describe_settings".to_string(),
                description: "Reports persisted runtime settings for UI and restart planning."
                    .to_string(),
            })
        } else if lowered.contains("runtime profile") || lowered.contains("runtime mode") {
            Some(CapabilityMatch {
                capability: "runtime.describe_profile".to_string(),
                description: "Reports the active runtime profile and enabled capability lanes."
                    .to_string(),
            })
        } else if lowered.contains("storage budget")
            || lowered.contains("memory budget")
            || lowered.contains("rag budget")
        {
            Some(CapabilityMatch {
                capability: "runtime.describe_storage".to_string(),
                description: "Reports current storage and retention budgets.".to_string(),
            })
        } else {
            None
        }
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        match capability {
            "runtime.describe_profile" => Some(ToolCallResult::Completed {
                result: json!({
                    "profile": self.snapshot.profile,
                    "control_api_bind": self.snapshot.control_api_bind,
                    "control_api_enabled": self.snapshot.control_api_bind.is_some(),
                    "rag_enabled": self.snapshot.rag_enabled,
                    "knowledge_enabled": self.snapshot.knowledge_enabled,
                    "rag_activation_mode": self.snapshot.rag_activation_mode,
                    "knowledge_activation_mode": self.snapshot.knowledge_activation_mode,
                    "llama_cpp_configured": self.snapshot.llama_cpp_configured,
                    "embedding_backend": self.snapshot.embedding_backend,
                    "embedding_dimensions": self.snapshot.embedding_dimensions,
                }),
            }),
            "runtime.describe_storage" => Some(ToolCallResult::Completed {
                result: json!({
                    "rag_budget_bytes": self.snapshot.rag_budget_bytes,
                    "memory_working_budget_bytes": self.snapshot.memory_working_budget_bytes,
                    "memory_pinned_budget_bytes": self.snapshot.memory_pinned_budget_bytes,
                    "self_heal_history_budget_bytes": self.snapshot.self_heal_history_budget_bytes,
                    "self_heal_model_context_size": self.snapshot.self_heal_model_context_size,
                    "self_heal_model_max_tokens": self.snapshot.self_heal_model_max_tokens,
                    "self_heal_model_temperature": rounded_runtime_float(
                        self.snapshot.self_heal_model_temperature,
                    ),
                }),
            }),
            "runtime.describe_settings" => match self.load_persisted_settings().await {
                Ok(persisted) => Some(ToolCallResult::Completed {
                    result: json!({
                        "effective": {
                            "profile": self.snapshot.profile,
                            "control_api_bind": self.snapshot.control_api_bind,
                            "control_api_enabled": self.snapshot.control_api_bind.is_some(),
                            "rag_enabled": self.snapshot.rag_enabled,
                            "knowledge_enabled": self.snapshot.knowledge_enabled,
                            "rag_activation_mode": self.snapshot.rag_activation_mode,
                            "knowledge_activation_mode": self.snapshot.knowledge_activation_mode,
                            "rag_budget_bytes": self.snapshot.rag_budget_bytes,
                            "memory_working_budget_bytes": self.snapshot.memory_working_budget_bytes,
                            "memory_pinned_budget_bytes": self.snapshot.memory_pinned_budget_bytes,
                            "self_heal_history_budget_bytes": self.snapshot.self_heal_history_budget_bytes,
                            "self_heal_llama_cpp_binary": self.snapshot.self_heal_llama_cpp_binary,
                            "self_heal_model_path": self.snapshot.self_heal_model_path,
                            "self_heal_model_context_size": self.snapshot.self_heal_model_context_size,
                            "self_heal_model_max_tokens": self.snapshot.self_heal_model_max_tokens,
                            "self_heal_model_temperature": rounded_runtime_float(
                                self.snapshot.self_heal_model_temperature,
                            ),
                            "llama_cpp_configured": self.snapshot.llama_cpp_configured,
                            "embedding_backend": self.snapshot.embedding_backend,
                            "embedding_dimensions": self.snapshot.embedding_dimensions,
                            "embedding_llama_cpp_binary": self.snapshot.embedding_llama_cpp_binary,
                            "embedding_model_path": self.snapshot.embedding_model_path,
                            "embedding_context_size": self.snapshot.embedding_context_size,
                            "embedding_ollama_url": self.snapshot.embedding_ollama_url,
                            "embedding_ollama_model": self.snapshot.embedding_ollama_model,
                        },
                        "persisted": persisted,
                        "restart_required_for_changes": true,
                    }),
                }),
                Err(error) => Some(ToolCallResult::Failed { error }),
            },
            "runtime.update_settings" => match self.persist_settings_update(arguments).await {
                Ok(result) => Some(ToolCallResult::Completed { result }),
                Err(error) => Some(ToolCallResult::Failed { error }),
            },
            _ => None,
        }
    }
}
