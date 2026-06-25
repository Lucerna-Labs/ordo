use crate::*;

// =========================================================================
// Review provider — exposes the `ordo-review` service as a capability lane
// so agents and plugins call `review.request_approval` like any other tool.
// Lives here (not in `ordo-review`) because it needs `CapabilityProvider`
// and we can't let `ordo-review` depend on `ordo-mcp-host` without a cycle.
// =========================================================================

pub const REVIEW_REQUEST_APPROVAL: &str = "review.request_approval";
pub const REVIEW_LIST_PENDING: &str = "review.list_pending";
pub const REVIEW_APPROVE: &str = "review.approve";
pub const REVIEW_DENY: &str = "review.deny";
pub const REVIEW_EDIT: &str = "review.edit";

const REVIEW_CAPABILITIES: &[&str] = &[
    REVIEW_REQUEST_APPROVAL,
    REVIEW_LIST_PENDING,
    REVIEW_APPROVE,
    REVIEW_DENY,
    REVIEW_EDIT,
];

const REVIEW_DEFAULT_WAIT_SECS: u64 = 300;

fn review_description(capability: &str) -> &'static str {
    match capability {
        REVIEW_REQUEST_APPROVAL => {
            "Queues an artifact for operator review. Optionally waits for the decision and returns the approved (possibly edited) content."
        }
        REVIEW_LIST_PENDING => "Lists every review request still awaiting operator action.",
        REVIEW_APPROVE => "Approves a queued review request by id.",
        REVIEW_DENY => "Denies a queued review request by id.",
        REVIEW_EDIT => "Edits the artifact and approves the request in one call.",
        _ => "Review capability.",
    }
}

pub struct ReviewProvider {
    service: ordo_review::ReviewService,
}

impl ReviewProvider {
    pub fn new(service: ordo_review::ReviewService) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &ordo_review::ReviewService {
        &self.service
    }
}

#[async_trait]
impl CapabilityProvider for ReviewProvider {
    fn name(&self) -> &str {
        "review"
    }

    fn capabilities(&self) -> Vec<String> {
        REVIEW_CAPABILITIES
            .iter()
            .map(|c| (*c).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        REVIEW_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    review_description(capability),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        let outcome: Result<Value, ordo_review::ReviewError> = match capability {
            REVIEW_REQUEST_APPROVAL => review_do_request(&self.service, arguments).await,
            REVIEW_LIST_PENDING => review_do_list_pending(&self.service),
            REVIEW_APPROVE => review_do_approve(&self.service, arguments),
            REVIEW_DENY => review_do_deny(&self.service, arguments),
            REVIEW_EDIT => review_do_edit(&self.service, arguments),
            _ => return None,
        };
        Some(match outcome {
            Ok(value) => ToolCallResult::Completed { result: value },
            Err(err) => ToolCallResult::Failed {
                error: err.to_string(),
            },
        })
    }
}

async fn review_do_request(
    service: &ordo_review::ReviewService,
    arguments: &Value,
) -> Result<Value, ordo_review::ReviewError> {
    let title = arguments
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_review::ReviewError::InvalidArgument("missing 'title'".into()))?
        .to_string();
    let content = arguments
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_review::ReviewError::InvalidArgument("missing 'content'".into()))?
        .to_string();
    let content_type = arguments
        .get("content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("text/markdown")
        .to_string();
    let origin_capability = arguments
        .get("origin_capability")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let origin_plugin = arguments
        .get("origin_plugin")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let metadata = arguments
        .get("metadata")
        .and_then(|v| v.as_object())
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let wait_seconds = arguments
        .get("wait_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(REVIEW_DEFAULT_WAIT_SECS);
    let async_only = arguments
        .get("async")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let new_request = ordo_review::NewReviewRequest {
        origin_capability,
        origin_plugin,
        title,
        content_type,
        content,
        metadata,
    };

    if async_only || wait_seconds == 0 {
        let queued = service.request(new_request)?;
        return Ok(serialize_review_request(&queued, false));
    }

    let resolved = service
        .request_and_wait(new_request, std::time::Duration::from_secs(wait_seconds))
        .await?;
    Ok(serialize_review_request(&resolved, true))
}

fn review_do_list_pending(
    service: &ordo_review::ReviewService,
) -> Result<Value, ordo_review::ReviewError> {
    let pending = service.pending()?;
    let serialized: Vec<Value> = pending
        .iter()
        .map(|r| serialize_review_request(r, false))
        .collect();
    Ok(json!({
        "count": serialized.len(),
        "pending": serialized,
    }))
}

fn review_do_approve(
    service: &ordo_review::ReviewService,
    arguments: &Value,
) -> Result<Value, ordo_review::ReviewError> {
    let id = parse_review_id(arguments)?;
    let note = arguments
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let resolved = service.decide(id, ordo_review::ReviewDecisionKind::Approve { note })?;
    Ok(serialize_review_request(&resolved, true))
}

fn review_do_deny(
    service: &ordo_review::ReviewService,
    arguments: &Value,
) -> Result<Value, ordo_review::ReviewError> {
    let id = parse_review_id(arguments)?;
    let note = arguments
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let resolved = service.decide(id, ordo_review::ReviewDecisionKind::Deny { note })?;
    Ok(serialize_review_request(&resolved, true))
}

fn review_do_edit(
    service: &ordo_review::ReviewService,
    arguments: &Value,
) -> Result<Value, ordo_review::ReviewError> {
    let id = parse_review_id(arguments)?;
    let content = arguments
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_review::ReviewError::InvalidArgument("missing 'content'".into()))?
        .to_string();
    let note = arguments
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let resolved = service.decide(id, ordo_review::ReviewDecisionKind::Edit { content, note })?;
    Ok(serialize_review_request(&resolved, true))
}

fn parse_review_id(arguments: &Value) -> Result<uuid::Uuid, ordo_review::ReviewError> {
    let raw = arguments
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ordo_review::ReviewError::InvalidArgument("missing 'id'".into()))?;
    uuid::Uuid::parse_str(raw)
        .map_err(|err| ordo_review::ReviewError::InvalidArgument(err.to_string()))
}

fn serialize_review_request(request: &ordo_review::ReviewRequest, include_content: bool) -> Value {
    let mut value = json!({
        "id": request.id,
        "created_at": request.created_at,
        "resolved_at": request.resolved_at,
        "origin_capability": request.origin_capability,
        "origin_plugin": request.origin_plugin,
        "title": request.title,
        "content_type": request.content_type,
        "state": request.state.label(),
        "has_edited_content": request.edited_content.is_some(),
        "decision_note": request.decision_note,
        "metadata": request.metadata,
    });
    if include_content {
        if let Some(object) = value.as_object_mut() {
            object.insert("content".into(), Value::String(request.content.clone()));
            object.insert(
                "effective_content".into(),
                Value::String(request.effective_content().to_string()),
            );
        }
    }
    value
}

