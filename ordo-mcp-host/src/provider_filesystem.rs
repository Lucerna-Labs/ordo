use crate::*;
use crate::helpers::*;

#[derive(Debug, Clone, Default)]
pub struct FilesystemProvider {
    root: Option<PathBuf>,
}

impl FilesystemProvider {
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    fn capability_description(&self) -> String {
        match &self.root {
            Some(root) => format!("Reads and writes files under {}.", root.display()),
            None => "Reads and writes files from the local disk.".to_string(),
        }
    }

    fn resolve_path(&self, requested: &str) -> Result<PathBuf, String> {
        let requested_path = PathBuf::from(requested);
        let Some(root) = &self.root else {
            return Ok(requested_path);
        };

        let normalized_root = normalize_path(root);
        let combined = if requested_path.is_absolute() {
            requested_path
        } else {
            normalized_root.join(requested_path)
        };
        let normalized = normalize_path(&combined);
        if normalized.starts_with(&normalized_root) {
            Ok(normalized)
        } else {
            Err(format!(
                "path '{}' escapes configured root {}",
                requested,
                normalized_root.display()
            ))
        }
    }
}

#[async_trait]
impl CapabilityProvider for FilesystemProvider {
    fn name(&self) -> &str {
        "filesystem"
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "filesystem.read_file".to_string(),
            "filesystem.write_file".to_string(),
        ]
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        vec![
            CapabilityDescriptor::new(
                "filesystem.read_file",
                self.name(),
                self.capability_description(),
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "filesystem.write_file",
                self.name(),
                self.capability_description(),
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
        ]
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        if requirement.contains("read file") {
            Some(CapabilityMatch {
                capability: "filesystem.read_file".to_string(),
                description: self.capability_description(),
            })
        } else {
            None
        }
    }

    async fn handle_run(&self, goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        let normalized_goal = goal.to_ascii_lowercase();
        if normalized_goal.contains("read file") || normalized_goal.contains("read") {
            let Some(path) = extract_read_path(goal) else {
                return Some(ProviderRun {
                    steps: vec![ProviderStep {
                        capability: "filesystem.read_file".to_string(),
                        name: "filesystem.read_file".to_string(),
                        status: ProviderRunStatus::Failed {
                            error: "run goal did not include a readable file path".to_string(),
                        },
                    }],
                });
            };

            let resolved_path = match self.resolve_path(&path) {
                Ok(path) => path,
                Err(error) => {
                    return Some(ProviderRun {
                        steps: vec![ProviderStep {
                            capability: "filesystem.read_file".to_string(),
                            name: "filesystem.read_file".to_string(),
                            status: ProviderRunStatus::Failed { error },
                        }],
                    });
                }
            };

            let status = match std::fs::read_to_string(&resolved_path) {
                Ok(contents) => ProviderRunStatus::Completed {
                    output: format!(
                        "read {} bytes from {} preview='{}'",
                        contents.len(),
                        resolved_path.display(),
                        preview_text(&contents)
                    ),
                },
                Err(err) => ProviderRunStatus::Failed {
                    error: format!("failed to read {}: {}", resolved_path.display(), err),
                },
            };
            Some(ProviderRun {
                steps: vec![ProviderStep {
                    capability: "filesystem.read_file".to_string(),
                    name: "filesystem.read_file".to_string(),
                    status,
                }],
            })
        } else {
            None
        }
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        match capability {
            "filesystem.read_file" => {
                let path = match require_string_argument(arguments, "path") {
                    Ok(value) => value,
                    Err(failed) => return Some(failed),
                };
                let resolved_path = match self.resolve_path(path) {
                    Ok(path) => path,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(match std::fs::read_to_string(&resolved_path) {
                    Ok(contents) => {
                        let mut result = json!({
                            "path": resolved_path.display().to_string(),
                            "bytes": contents.len(),
                            "preview": preview_text(&contents),
                        });
                        attach_context_to_output(&mut result, arguments);
                        ToolCallResult::Completed { result }
                    }
                    Err(err) => ToolCallResult::Failed {
                        error: format!("failed to read {}: {}", resolved_path.display(), err),
                    },
                })
            }
            "filesystem.write_file" => {
                let path = match require_string_argument(arguments, "path") {
                    Ok(value) => value,
                    Err(failed) => return Some(failed),
                };
                let content = match require_string_argument(arguments, "content") {
                    Ok(value) => value,
                    Err(failed) => return Some(failed),
                };
                let resolved_path = match self.resolve_path(path) {
                    Ok(path) => path,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                if let Some(parent) = resolved_path.parent() {
                    if let Err(err) = std::fs::create_dir_all(parent) {
                        return Some(ToolCallResult::Failed {
                            error: format!(
                                "failed to prepare parent directory for {}: {}",
                                resolved_path.display(),
                                err
                            ),
                        });
                    }
                }
                Some(match std::fs::write(&resolved_path, content) {
                    Ok(()) => {
                        let mut result = json!({
                            "path": resolved_path.display().to_string(),
                            "bytes": content.len(),
                            "status": "written",
                        });
                        attach_context_to_output(&mut result, arguments);
                        ToolCallResult::Completed { result }
                    }
                    Err(err) => ToolCallResult::Failed {
                        error: format!("failed to write {}: {}", resolved_path.display(), err),
                    },
                })
            }
            _ => None,
        }
    }
}

