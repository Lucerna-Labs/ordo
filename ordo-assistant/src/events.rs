//! Per-session event stream so the studio can show what the assistant
//! is doing *while* it's doing it (tool call starts/completes, final
//! reply, errors). The control-API WebSocket layer subscribes to this
//! broadcast and fans events out to connected clients.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::types::{RagHitSummary, RecalledFact, ReviewOutcome, Turn};

const BROADCAST_CAPACITY: usize = 64;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TurnEvent {
    /// Emitted at the very start of a turn — the studio can show a
    /// "typing" indicator until it sees `turn_completed`.
    TurnStarted {
        session_id: Uuid,
        user_message: String,
    },
    /// Router finished picking context.
    ContextRetrieved {
        session_id: Uuid,
        facts: Vec<RecalledFact>,
        rag_hits: Vec<RagHitSummary>,
    },
    /// Assistant decided to call a capability.
    ToolCallStarted {
        session_id: Uuid,
        invocation_id: Uuid,
        capability: String,
        arguments: Value,
    },
    /// Capability returned a value.
    ToolCallCompleted {
        session_id: Uuid,
        invocation_id: Uuid,
        capability: String,
        result: Value,
    },
    /// Capability failed — the assistant sees the error too and may
    /// try a different tool on the next iteration.
    ToolCallFailed {
        session_id: Uuid,
        invocation_id: Uuid,
        capability: String,
        error: String,
    },
    /// Assistant draft was submitted to the `review.*` queue. The
    /// studio can use this to flip the session into \"awaiting review\"
    /// UI state.
    ReviewRequested {
        session_id: Uuid,
        review_request_id: Uuid,
        draft: String,
    },
    /// Operator decided on the draft (approve / edit / deny / expire).
    /// Fires once per review submission.
    ReviewResolved {
        session_id: Uuid,
        outcome: ReviewOutcome,
    },
    /// Streaming token chunk from the LLM. Emitted only for
    /// non-tool-using OpenAI turns today (push 6). The studio
    /// concatenates these to render a live \"typing\" effect.
    TokenDelta { session_id: Uuid, delta: String },
    /// Final LLM reply persisted.
    TurnCompleted { session_id: Uuid, turn: Turn },
    /// Terminal error before a turn could be persisted.
    TurnFailed { session_id: Uuid, error: String },

    // ─── Mode-scoped workspace events (ordo-modes step 8) ──────
    //
    // The mode subsystem fires these so the insight trace can show
    // "why this turn loaded the memory it loaded, why it had the
    // tools it had." Each event is a snapshot of the resolved
    // mode at the moment it took effect.
    /// Emitted once per turn at the moment the runtime resolves the
    /// session's stored mode id to a manifest. Carries the active
    /// scope summary (memory_scope list, RAG domains, tool lane
    /// count) so the studio can render the "you're operating in
    /// mode X with these constraints" banner without a second API
    /// call. Skipped when the assistant has no registry attached.
    ModeBound {
        session_id: Uuid,
        mode_id: String,
        mode_label: String,
        memory_scope: Vec<String>,
        rag_domains: Vec<String>,
        allowed_tool_lane_count: usize,
        blocked_tool_capability_count: usize,
    },
    /// Emitted when the assistant.recall_memory meta-tool returns,
    /// noting how many facts the mode's scope filter let through.
    /// The studio's audit / debug view can answer "why didn't this
    /// recall surface fact X?" by inspecting this event's
    /// `visible_scopes` list.
    ModeMemoryScopeApplied {
        session_id: Uuid,
        mode_id: String,
        visible_scopes: Vec<String>,
        facts_visible: usize,
    },
    /// Emitted when the LLM tool-schema build filters out
    /// capabilities that the active mode forbids. `filtered_count`
    /// is the number of bus-advertised caps the mode dropped from
    /// the LLM's view this turn. Always-zero turns are silent.
    ModeToolFilterApplied {
        session_id: Uuid,
        mode_id: String,
        kept_capabilities: usize,
        filtered_count: usize,
    },

    /// Emitted when one mode asks another mode's agent for bounded
    /// advice. This is consultation, not context borrowing: the
    /// active mode receives only the target agent's answer, never the
    /// target mode's raw RAG or memory.
    CrossModeConsultRequested {
        session_id: Uuid,
        active_mode: String,
        target_mode: String,
        reason: String,
        question: String,
    },
    /// Emitted when the target mode's policy allows consultation.
    CrossModeConsultApproved {
        session_id: Uuid,
        active_mode: String,
        target_mode: String,
    },
    /// Emitted when the target mode's policy denies consultation.
    CrossModeConsultDenied {
        session_id: Uuid,
        active_mode: String,
        target_mode: String,
        reason: String,
    },
    /// Emitted after the target mode subagent returns.
    CrossModeConsultCompleted {
        session_id: Uuid,
        active_mode: String,
        target_mode: String,
        turn_id: Uuid,
    },
}

/// Per-session broadcast channels. Cheap to clone (Arc<Mutex<HashMap>>).
#[derive(Clone, Default)]
pub struct EventBroadcaster {
    channels: Arc<Mutex<HashMap<Uuid, broadcast::Sender<TurnEvent>>>>,
}

impl EventBroadcaster {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, session_id: Uuid, event: TurnEvent) {
        let maybe_sender = self.channels.lock().get(&session_id).cloned();
        if let Some(sender) = maybe_sender {
            let _ = sender.send(event);
        }
    }

    pub fn subscribe(&self, session_id: Uuid) -> broadcast::Receiver<TurnEvent> {
        let mut channels = self.channels.lock();
        let sender = channels
            .entry(session_id)
            .or_insert_with(|| broadcast::channel(BROADCAST_CAPACITY).0);
        sender.subscribe()
    }

    /// Remove the channel for a session. Called when the last
    /// subscriber goes away; not strictly necessary because
    /// `broadcast::Sender` with zero receivers just drops messages.
    pub fn forget(&self, session_id: Uuid) {
        self.channels.lock().remove(&session_id);
    }
}
