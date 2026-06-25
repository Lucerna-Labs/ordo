use crate::*;
use crate::helpers::*;

pub struct SelfHealToolsProvider {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
    store: SelfHealStorageTask,
}

impl SelfHealToolsProvider {
    pub fn new(store: SelfHealStorageTask, bus: Arc<dyn Bus>) -> Self {
        Self {
            node_id: NodeId::new(),
            bus,
            store,
        }
    }

    async fn list_cases(&self, limit: usize) -> Result<Value, String> {
        let cases = self
            .store
            .list_cases(limit)
            .await
            .map_err(|err| format!("failed to list self-heal cases: {err}"))?;
        Ok(json!({
            "count": cases.len(),
            "results": cases
                .into_iter()
                .map(|case| self.case_json(&case))
                .collect::<Vec<_>>(),
        }))
    }

    async fn forget_case(&self, fingerprint: &str) -> Result<Value, String> {
        let removed = self
            .store
            .forget_case(fingerprint.to_string())
            .await
            .map_err(|err| format!("failed to forget self-heal case: {err}"))?;
        Ok(json!({
            "fingerprint": fingerprint,
            "removed": removed,
        }))
    }

    fn case_json(&self, case: &SelfHealCaseSummary) -> Value {
        json!({
            "fingerprint": case.fingerprint,
            "component": case.component,
            "symptom": case.symptom,
            "summary": case.summary,
            "why": case.why,
            "actions": case.actions,
            "source": case.source,
            "occurrence_count": case.occurrence_count,
            "updated_at": case.updated_at,
        })
    }

    fn pinned_case_note(case: &SelfHealCaseSummary) -> String {
        let action_lines = case
            .actions
            .iter()
            .map(|action| format!("- {action}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Self-heal fix: {fingerprint}\nComponent: {component}\nSymptom: {symptom}\nSummary: {summary}\nWhy: {why}\nSource: {source}\nOccurrences: {occurrence_count}\nActions:\n{actions}",
            fingerprint = case.fingerprint,
            component = case.component,
            symptom = case.symptom,
            summary = case.summary,
            why = case.why,
            source = case.source,
            occurrence_count = case.occurrence_count,
            actions = action_lines,
        )
    }

    fn export_case_markdown(case: &SelfHealCaseSummary) -> String {
        let actions = case
            .actions
            .iter()
            .map(|action| format!("- {action}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "# Self-heal case: {fingerprint}\n\n## Summary\n{summary}\n\n## Component\n{component}\n\n## Symptom\n{symptom}\n\n## Why it worked\n{why}\n\n## Source\n{source}\n\n## Occurrences\n{occurrence_count}\n\n## Actions\n{actions}\n",
            fingerprint = case.fingerprint,
            summary = case.summary,
            component = case.component,
            symptom = case.symptom,
            why = case.why,
            source = case.source,
            occurrence_count = case.occurrence_count,
            actions = actions,
        )
    }

    fn export_case_filename(fingerprint: &str) -> String {
        let safe = fingerprint
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
                _ => '-',
            })
            .collect::<String>();
        format!("self-heal-{safe}.md")
    }

    async fn export_case(&self, fingerprint: &str) -> Result<Value, String> {
        let Some(case) = self
            .store
            .get_case(fingerprint.to_string())
            .await
            .map_err(|err| format!("failed to query self-heal case: {err}"))?
        else {
            return Err(format!(
                "no remembered self-heal case for fingerprint '{fingerprint}'"
            ));
        };

        Ok(json!({
            "fingerprint": fingerprint,
            "filename": Self::export_case_filename(fingerprint),
            "case": self.case_json(&case),
            "markdown": Self::export_case_markdown(&case),
        }))
    }

    async fn replay_case(&self, fingerprint: &str) -> Result<Value, String> {
        let Some(case) = self
            .store
            .get_case(fingerprint.to_string())
            .await
            .map_err(|err| format!("failed to query self-heal case: {err}"))?
        else {
            return Err(format!(
                "no remembered self-heal case for fingerprint '{fingerprint}'"
            ));
        };

        let incident = SelfHealIncident {
            incident_id: Uuid::new_v4(),
            component: case.component.clone(),
            symptom: case.symptom.clone(),
            fingerprint: case.fingerprint.clone(),
            urgency: SelfHealUrgency::Medium,
            logs: vec![
                "operator replay requested from remembered case".to_string(),
                format!("replaying remembered fix for {}", case.summary),
            ],
        };
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::SelfHealRequested {
                incident: incident.clone(),
            },
        )
        .with_correlation(correlation_id.clone());
        let mut sub = self
            .bus
            .subscribe(topics::SELF_HEAL_RESPONSE)
            .await
            .map_err(|err| format!("failed to subscribe for self-heal response: {err}"))?;
        self.bus
            .publish(topics::SELF_HEAL_REQUEST, envelope)
            .await
            .map_err(|err| format!("failed to publish self-heal replay request: {err}"))?;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.next()).await {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }

                    if let OrdoMessage::SelfHealPlanned {
                        incident_id,
                        fingerprint: seen_fingerprint,
                        plan,
                    } = event.payload
                    {
                        if incident_id == incident.incident_id
                            && seen_fingerprint == case.fingerprint
                        {
                            return Ok(json!({
                                "fingerprint": case.fingerprint,
                                "incident_id": incident_id,
                                "replayed": true,
                                "plan": {
                                    "summary": plan.summary,
                                    "why": plan.why,
                                    "actions": plan.actions,
                                    "source": format!("{:?}", plan.source),
                                    "reused_previous_fix": plan.reused_previous_fix,
                                    "memory_hits": plan.memory_hits,
                                },
                            }));
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for self-heal replay result".to_string());
                }
            }
        }
    }

    async fn pin_case(&self, fingerprint: &str) -> Result<Value, String> {
        let Some(case) = self
            .store
            .get_case(fingerprint.to_string())
            .await
            .map_err(|err| format!("failed to query self-heal case: {err}"))?
        else {
            return Err(format!(
                "no remembered self-heal case for fingerprint '{fingerprint}'"
            ));
        };

        let content = Self::pinned_case_note(&case);
        let prefix = format!("Self-heal fix: {fingerprint}\n");
        let existing = self.list_pinned_memory(256).await?;
        let mut replaced_existing = 0usize;
        for previous in existing {
            if previous.starts_with(&prefix)
                && previous != content
                && self.remove_pinned_memory(previous).await?
            {
                replaced_existing += 1;
            }
        }
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryStoreRequested {
                content: content.clone(),
                tier: MemoryTier::Pinned,
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
                        if seen_content == content && seen_tier == MemoryTier::Pinned {
                            return Ok(json!({
                                "fingerprint": fingerprint,
                                "replaced_existing": replaced_existing,
                                "stored": stored,
                                "tier": "pinned",
                                "content": content,
                            }));
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    return Err("timed out waiting for memory store confirmation".to_string());
                }
            }
        }
    }

    async fn list_pinned_memory(&self, limit: usize) -> Result<Vec<String>, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryListRequested {
                tier: MemoryTier::Pinned,
                limit,
            },
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
                        if seen_tier == MemoryTier::Pinned {
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

    async fn remove_pinned_memory(&self, content: String) -> Result<bool, String> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(
            self.node_id.clone(),
            OrdoMessage::MemoryRemoveRequested {
                content: content.clone(),
                tier: MemoryTier::Pinned,
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
                        if seen_content == content && seen_tier == MemoryTier::Pinned {
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
}

#[async_trait]
impl CapabilityProvider for SelfHealToolsProvider {
    fn name(&self) -> &str {
        "self-heal"
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "self_heal.list_cases".to_string(),
            "self_heal.forget_case".to_string(),
            "self_heal.pin_case".to_string(),
            "self_heal.replay_case".to_string(),
            "self_heal.export_case".to_string(),
        ]
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        vec![
            CapabilityDescriptor::new(
                "self_heal.list_cases",
                self.name(),
                "Lists remembered self-heal fixes and incident fingerprints.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "self_heal.forget_case",
                self.name(),
                "Removes a remembered self-heal case and its retained attempts.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "self_heal.pin_case",
                self.name(),
                "Pins a remembered self-heal case into always-available memory.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "self_heal.replay_case",
                self.name(),
                "Replays a remembered self-heal case through the live self-heal lane.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
            CapabilityDescriptor::new(
                "self_heal.export_case",
                self.name(),
                "Exports a remembered self-heal case as an operator-friendly memory pack.",
                CapabilityTier::Core,
                CapabilityActivation::Eager,
            ),
        ]
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        let lowered = requirement.to_ascii_lowercase();
        if lowered.contains("self-heal history")
            || lowered.contains("remembered fixes")
            || lowered.contains("repair history")
        {
            Some(CapabilityMatch {
                capability: "self_heal.list_cases".to_string(),
                description: "Lists remembered self-heal fixes and incident fingerprints."
                    .to_string(),
            })
        } else if lowered.contains("forget self-heal")
            || lowered.contains("delete remembered fix")
            || lowered.contains("remove repair memory")
        {
            Some(CapabilityMatch {
                capability: "self_heal.forget_case".to_string(),
                description: "Removes a remembered self-heal case and its retained attempts."
                    .to_string(),
            })
        } else if lowered.contains("pin self-heal")
            || lowered.contains("promote repair memory")
            || lowered.contains("save remembered fix")
        {
            Some(CapabilityMatch {
                capability: "self_heal.pin_case".to_string(),
                description: "Pins a remembered self-heal case into always-available memory."
                    .to_string(),
            })
        } else if lowered.contains("replay self-heal")
            || lowered.contains("re-run remembered fix")
            || lowered.contains("retry remembered repair")
        {
            Some(CapabilityMatch {
                capability: "self_heal.replay_case".to_string(),
                description: "Replays a remembered self-heal case through the live repair lane."
                    .to_string(),
            })
        } else if lowered.contains("export self-heal")
            || lowered.contains("export repair memory")
            || lowered.contains("share remembered fix")
        {
            Some(CapabilityMatch {
                capability: "self_heal.export_case".to_string(),
                description: "Exports a remembered self-heal case as a reusable memory pack."
                    .to_string(),
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
            "self_heal.list_cases" => {
                let limit = match parse_limit_argument(arguments, "limit", 10) {
                    Ok(limit) => limit,
                    Err(error) => return Some(ToolCallResult::Failed { error }),
                };
                Some(match self.list_cases(limit).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "self_heal.forget_case" => {
                let fingerprint = match require_string_argument(arguments, "fingerprint") {
                    Ok(value) => value.trim().to_string(),
                    Err(failed) => return Some(failed),
                };
                if fingerprint.is_empty() {
                    return Some(ToolCallResult::Failed {
                        error: "fingerprint must not be empty".to_string(),
                    });
                }
                Some(match self.forget_case(&fingerprint).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "self_heal.pin_case" => {
                let fingerprint = match require_string_argument(arguments, "fingerprint") {
                    Ok(value) => value.trim().to_string(),
                    Err(failed) => return Some(failed),
                };
                if fingerprint.is_empty() {
                    return Some(ToolCallResult::Failed {
                        error: "fingerprint must not be empty".to_string(),
                    });
                }
                Some(match self.pin_case(&fingerprint).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "self_heal.replay_case" => {
                let fingerprint = match require_string_argument(arguments, "fingerprint") {
                    Ok(value) => value.trim().to_string(),
                    Err(failed) => return Some(failed),
                };
                if fingerprint.is_empty() {
                    return Some(ToolCallResult::Failed {
                        error: "fingerprint must not be empty".to_string(),
                    });
                }
                Some(match self.replay_case(&fingerprint).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            "self_heal.export_case" => {
                let fingerprint = match require_string_argument(arguments, "fingerprint") {
                    Ok(value) => value.trim().to_string(),
                    Err(failed) => return Some(failed),
                };
                if fingerprint.is_empty() {
                    return Some(ToolCallResult::Failed {
                        error: "fingerprint must not be empty".to_string(),
                    });
                }
                Some(match self.export_case(&fingerprint).await {
                    Ok(result) => ToolCallResult::Completed { result },
                    Err(error) => ToolCallResult::Failed { error },
                })
            }
            _ => None,
        }
    }
}
