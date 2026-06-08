//! `AppsProvider` â€” exposes `AppsService` as a `CapabilityProvider`
//! so the Assistant (and future MCP bridge) can reach it through the
//! same tool gateway as every other capability.
//!
//! Rule 2: this is how new capability surface area joins the platform.
//! Every tool shape is published with a JSON Schema (Rule 9 â€”
//! descriptive, not enforced; runtime still hands arguments through
//! as `Value`).

use ordo_protocol::{CapabilityActivation, CapabilityDescriptor, CapabilityTier};
use serde_json::{json, Value};

use crate::service::AppsService;
use crate::types::{AppRef, AppUpdate, AppsQuery, NewApp};

const PROVIDER_NAME: &str = "ordo-apps";

pub struct AppsProvider {
    service: AppsService,
}

impl AppsProvider {
    pub fn new(service: AppsService) -> Self {
        Self { service }
    }

    fn describe(cap: &str, description: &str, input_schema: Value) -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            cap,
            PROVIDER_NAME,
            description,
            CapabilityTier::Optional,
            CapabilityActivation::Lazy,
        )
        .with_input_schema(input_schema)
    }

    async fn dispatch(&self, capability: &str, arguments: &Value) -> Result<Value, String> {
        match capability {
            "apps.list" => {
                let query: AppsQuery =
                    serde_json::from_value(arguments.clone()).unwrap_or_default();
                let apps = self.service.list(query).map_err(|e| e.to_string())?;
                Ok(json!({ "apps": apps }))
            }
            "apps.get" => {
                let app_ref = parse_ref(arguments)?;
                let app = self
                    .service
                    .get(&app_ref)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "app not found".to_string())?;
                Ok(json!({ "app": app }))
            }
            "apps.create" => {
                let new_app: NewApp = serde_json::from_value(arguments.clone())
                    .map_err(|e| format!("invalid NewApp: {e}"))?;
                let actor = arguments
                    .get("actor")
                    .and_then(|v| v.as_str())
                    .unwrap_or("operator")
                    .to_string();
                let app = self
                    .service
                    .create(new_app, &actor)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "app": app }))
            }
            "apps.update" => {
                let app_ref = parse_ref(arguments)?;
                let patch: AppUpdate = arguments
                    .get("patch")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|e| format!("invalid patch: {e}"))?
                    .unwrap_or_default();
                let app = self
                    .service
                    .update(&app_ref, patch)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "app": app }))
            }
            "apps.publish" => self.status_transition(arguments, Transition::Publish).await,
            "apps.unpublish" => {
                self.status_transition(arguments, Transition::Unpublish)
                    .await
            }
            "apps.archive" => self.status_transition(arguments, Transition::Archive).await,
            "apps.unarchive" => {
                self.status_transition(arguments, Transition::Unarchive)
                    .await
            }
            "apps.events" => {
                let app_ref = parse_ref(arguments)?;
                let app = self
                    .service
                    .get(&app_ref)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "app not found".to_string())?;
                let events = self.service.events(app.id).map_err(|e| e.to_string())?;
                Ok(json!({ "events": events }))
            }
            "apps.state_at_version" => {
                let app_ref = parse_ref(arguments)?;
                let app = self
                    .service
                    .get(&app_ref)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "app not found".to_string())?;
                let up_to_seq = arguments
                    .get("seq")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "missing `seq`".to_string())?;
                let historical = self
                    .service
                    .state_at_version(app.id, up_to_seq)
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "app": historical }))
            }
            other => Err(format!("unknown apps capability: {other}")),
        }
    }

    async fn status_transition(
        &self,
        arguments: &Value,
        transition: Transition,
    ) -> Result<Value, String> {
        let app_ref = parse_ref(arguments)?;
        let actor = arguments
            .get("actor")
            .and_then(|v| v.as_str())
            .unwrap_or("operator");
        let app = match transition {
            Transition::Publish => self.service.publish(&app_ref, actor).await,
            Transition::Unpublish => self.service.unpublish(&app_ref, actor).await,
            Transition::Archive => self.service.archive(&app_ref, actor).await,
            Transition::Unarchive => self.service.unarchive(&app_ref, actor).await,
        }
        .map_err(|e| e.to_string())?;
        Ok(json!({ "app": app }))
    }
}

enum Transition {
    Publish,
    Unpublish,
    Archive,
    Unarchive,
}

fn parse_ref(arguments: &Value) -> Result<AppRef, String> {
    // Accept either { "id": "..." } or { "workspace_id": "...", "slug": "..." }.
    if let Some(id) = arguments.get("id").and_then(|v| v.as_str()) {
        return uuid::Uuid::parse_str(id)
            .map(AppRef::Id)
            .map_err(|e| format!("invalid id: {e}"));
    }
    let slug = arguments
        .get("slug")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing `id` or `slug`".to_string())?;
    let workspace_id = arguments
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .unwrap_or("local")
        .to_string();
    Ok(AppRef::Slug {
        workspace_id,
        slug: slug.to_string(),
    })
}

// -- CapabilityProvider impl wrapped via a thin adapter. The trait
//    lives in ordo-mcp-host, which this crate doesn't depend on (avoids a
//    cycle through ordo-mcp-host â†’ ordo-assistant â†’ ordo-apps). The runtime
//    wires an adapter at startup; see `ordo-mcp-host` for the glue.
//    Exposing the dispatch function and descriptors keeps the wiring
//    code trivial on the runtime side.

impl AppsProvider {
    pub fn capabilities_list() -> Vec<&'static str> {
        vec![
            "apps.list",
            "apps.get",
            "apps.create",
            "apps.update",
            "apps.publish",
            "apps.unpublish",
            "apps.archive",
            "apps.unarchive",
            "apps.events",
            "apps.state_at_version",
        ]
    }

    pub fn descriptors() -> Vec<CapabilityDescriptor> {
        let slug_or_id = json!({
            "type": "object",
            "description": "Reference an app by UUID (`id`) or by (`workspace_id`, `slug`).",
            "properties": {
                "id": {"type": "string", "format": "uuid"},
                "workspace_id": {"type": "string"},
                "slug": {"type": "string"},
            },
            "oneOf": [
                {"required": ["id"]},
                {"required": ["slug"]}
            ]
        });
        vec![
            Self::describe(
                "apps.list",
                "List apps in a workspace, optionally filtered by status.",
                json!({
                    "type": "object",
                    "properties": {
                        "workspace_id": {"type": "string", "default": "local"},
                        "status": {"type": "string", "enum": ["draft", "published", "archived"]},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 500}
                    }
                }),
            ),
            Self::describe("apps.get", "Fetch a single app by id or slug.", slug_or_id.clone()),
            Self::describe(
                "apps.create",
                "Create a new app. `slug` is derived from `name` when omitted.",
                json!({
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": {"type": "string", "minLength": 1},
                        "description": {"type": "string"},
                        "slug": {"type": "string"},
                        "workspace_id": {"type": "string", "default": "local"},
                        "metadata": {"type": "object", "additionalProperties": true},
                        "actor": {"type": "string", "default": "operator"}
                    }
                }),
            ),
            Self::describe(
                "apps.update",
                "Patch an app's name, description, and metadata. Metadata keys set to `null` are removed.",
                json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "format": "uuid"},
                        "workspace_id": {"type": "string"},
                        "slug": {"type": "string"},
                        "patch": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "description": {"type": "string"},
                                "metadata_patch": {"type": "object", "additionalProperties": true},
                                "actor": {"type": "string"}
                            }
                        }
                    }
                }),
            ),
            Self::describe(
                "apps.publish",
                "Transition a draft app to published. Destructive operation â€” review-gated when a ReviewService is wired.",
                slug_or_id.clone(),
            ),
            Self::describe(
                "apps.unpublish",
                "Revert a published app to draft. Destructive â€” review-gated when wired.",
                slug_or_id.clone(),
            ),
            Self::describe(
                "apps.archive",
                "Archive an app. Destructive â€” review-gated when wired.",
                slug_or_id.clone(),
            ),
            Self::describe(
                "apps.unarchive",
                "Restore an archived app to draft.",
                slug_or_id.clone(),
            ),
            Self::describe(
                "apps.events",
                "Return the full event log for an app in sequence order.",
                slug_or_id.clone(),
            ),
            Self::describe(
                "apps.state_at_version",
                "Reconstruct the state of an app at a historical event sequence. Version fold primitive for rollback and diff flows.",
                json!({
                    "type": "object",
                    "required": ["seq"],
                    "properties": {
                        "id": {"type": "string", "format": "uuid"},
                        "workspace_id": {"type": "string"},
                        "slug": {"type": "string"},
                        "seq": {"type": "integer", "minimum": 0, "description": "Last event sequence number to include (inclusive)."}
                    }
                }),
            ),
        ]
    }

    pub async fn invoke(&self, capability: &str, arguments: &Value) -> Result<Value, String> {
        self.dispatch(capability, arguments).await
    }
}
