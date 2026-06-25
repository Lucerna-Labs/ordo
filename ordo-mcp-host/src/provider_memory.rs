use crate::*;
use crate::helpers::*;

pub struct MemoryToolsProvider {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
}

impl MemoryToolsProvider {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        Self {
            node_id: NodeId::new(),
            bus,
        }
    }

    fn note_content(arguments: &Value) -> Result<String, String> {
        let content = if let Some(content) = arguments.as_str() {
            content.trim()
        } else {
            arguments
                .get("content")
                .or_else(|| arguments.get("note"))
                .or_else(|| arguments.get("text"))
                .or_else(|| arguments.get("message"))
                .or_else(|| arguments.get("body"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .ok_or_else(|| {
                    "content, note, text, message, or body must be provided as a string".to_string()
                })?
        };
        if content.is_empty() {
            Err("content must not be empty".to_string())
        } else {
            Ok(content.to_string())
        }
    }

    async fn store_memory(&self, content: String, tier: MemoryTier) -> Result<bool, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryStoreRequested {
                content: content.clone(),
                tier,
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::MEMORY_STORE_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for memory store response: {err}"))?;
        self.bus
            .publish(topics::MEMORY_STORE_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish memory store request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::MemoryStoreCompleted {
                        content: seen_content,
                        tier: seen_tier,
                        stored,
                    } = event.payload
                    {
                        if seen_content == content && seen_tier == tier {
                            return Ok(stored);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory store confirmation".to_string());
                }
            }
        }
    }

    async fn remove_memory(&self, content: String, tier: MemoryTier) -> Result<bool, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryRemoveRequested {
                content: content.clone(),
                tier,
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::MEMORY_REMOVE_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for memory remove response: {err}"))?;
        self.bus
            .publish(topics::MEMORY_REMOVE_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish memory remove request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::MemoryRemoveCompleted {
                        content: seen_content,
                        tier: seen_tier,
                        removed,
                    } = event.payload
                    {
                        if seen_content == content && seen_tier == tier {
                            return Ok(removed);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory remove confirmation".to_string());
                }
            }
        }
    }

    async fn list_memory(&self, tier: MemoryTier, limit: usize) -> Result<Vec<String>, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryListRequested { tier, limit },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::MEMORY_LIST_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for memory list response: {err}"))?;
        self.bus
            .publish(topics::MEMORY_LIST_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish memory list request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::MemoryListed {
                        tier: seen_tier,
                        results,
                    } = event.payload
                    {
                        if seen_tier == tier {
                            return Ok(results);
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory list response".to_string());
                }
            }
        }
    }
}

#[async_trait]
impl CapabilityProvider for MemoryToolsProvider {
    fn name(&self) -> &str {
        "memory"
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "memory.pin_note".to_string(),
            "memory.unpin_note".to_string(),
            "memory.remember_note".to_string(),
            "memory.list_pinned".to_string(),
            "memory.list_working".to_string(),
        ]
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        vec![
            CapabilityDescriptor::new(
                "memory.pin_note",
                self.name(),
                "Pins an important memory so it stays in the always-available lane.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "memory.unpin_note",
                self.name(),
                "Removes a pinned memory from the always-available lane.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "memory.remember_note",
                self.name(),
                "Stores a normal working-memory note.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "memory.list_pinned",
                self.name(),
                "Lists recently pinned memories for review or UI display.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "memory.list_working",
                self.name(),
                "Lists recent working-memory notes for review or UI display.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
        ]
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        let lowered = requirement.to_ascii_lowercase();
        if lowered.contains("pin memory")
            || lowered.contains("important memory")
            || lowered.contains("always available memory")
        {
            Some(CapabilityMatch {
                capability: "memory.pin_note".to_string(),
                description: "Pins an important memory into the reserved memory lane.".to_string(),
            })
        } else if lowered.contains("unpin memory")
            || lowered.contains("remove pinned memory")
            || lowered.contains("delete important memory")
        {
            Some(CapabilityMatch {
                capability: "memory.unpin_note".to_string(),
                description: "Removes a pinned memory from the reserved memory lane.".to_string(),
            })
        } else if lowered.contains("remember this") || lowered.contains("save memory note") {
            Some(CapabilityMatch {
                capability: "memory.remember_note".to_string(),
                description: "Stores a note in working memory.".to_string(),
            })
        } else if lowered.contains("list pinned memory")
            || lowered.contains("show pinned memory")
            || lowered.contains("important memories")
        {
            Some(CapabilityMatch {
                capability: "memory.list_pinned".to_string(),
                description: "Lists recently pinned memories.".to_string(),
            })
        } else if lowered.contains("list working memory")
            || lowered.contains("show working memory")
            || lowered.contains("recent memory notes")
        {
            Some(CapabilityMatch {
                capability: "memory.list_working".to_string(),
                description: "Lists recent working-memory notes.".to_string(),
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
            "memory.pin_note" => {
                let content = match Self::note_content(arguments) {
                    Ok(content) => content,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(
                    match self.store_memory(content.clone(), MemoryTier::Pinned).await {
                        Ok(stored) => ToolCallResult::Completed {
                            result: json!({
                                "content": content,
                                "tier": "pinned",
                                "stored": stored,
                            }),
                        },
                        Err(error) => ToolCallResult::Failed { error },
                    },
                )
            }
            "memory.unpin_note" => {
                let content = match Self::note_content(arguments) {
                    Ok(content) => content,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(
                    match self
                        .remove_memory(content.clone(), MemoryTier::Pinned)
                        .await
                    {
                        Ok(removed) => ToolCallResult::Completed {
                            result: json!({
                                "content": content,
                                "tier": "pinned",
                                "removed": removed,
                            }),
                        },
                        Err(error) => ToolCallResult::Failed { error },
                    },
                )
            }
            "memory.remember_note" => {
                let content = match Self::note_content(arguments) {
                    Ok(content) => content,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(
                    match self
                        .store_memory(content.clone(), MemoryTier::Working)
                        .await
                    {
                        Ok(stored) => ToolCallResult::Completed {
                            result: json!({
                                "content": content,
                                "tier": "working",
                                "stored": stored,
                            }),
                        },
                        Err(error) => ToolCallResult::Failed { error },
                    },
                )
            }
            "memory.list_pinned" => {
                let limit = match parse_limit_argument(arguments, "limit", 10) {
                    Ok(limit) => limit,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(match self.list_memory(MemoryTier::Pinned, limit).await {
                    Ok(results) => ToolCallResult::Completed {
                        result: json!({
                            "tier": "pinned",
                            "count": results.len(),
                            "results": results,
                        }),
                    },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "memory.list_working" => {
                let limit = match parse_limit_argument(arguments, "limit", 10) {
                    Ok(limit) => limit,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(match self.list_memory(MemoryTier::Working, limit).await {
                    Ok(results) => ToolCallResult::Completed {
                        result: json!({
                            "tier": "working",
                            "count": results.len(),
                            "results": results,
                        }),
                    },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            _ => None,
        }
    }
}

fn parse_runtime_profile_argument(value: &Value) -> Result<String, String> {
    let profile = value
        .as_str()
        .ok_or_else(|| "profile must be a string".to_string())?
        .to_ascii_lowercase();
    if matches!(profile.as_str(), "minimal" | "standard" | "full") {
        Ok(profile)
    } else {
        Err(format!(
            "unsupported profile '{}'; expected minimal, standard, or full",
            profile
        ))
    }
}
