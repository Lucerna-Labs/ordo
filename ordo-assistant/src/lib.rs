//! The assistant layer â€” the "lawyer on top of the RAGs."
//!
//! Every operator interaction with Ordo is supposed to route
//! through this layer. The Assistant holds durable memory about the
//! operator (facts, preferences, persona), keeps a running conversation
//! thread, pulls from the specialized RAG collections as needed, and is
//! the single gate through which the LLM is called.
//!
//! Push 1 (this crate) delivers:
//! - SQLite-persisted sessions, turns, and facts
//! - Semantic fact recall via the shared embedder
//! - Deterministic router: fact recall + RAG grounding + history window
//! - One-LLM-call-per-turn conversation
//!
//! Push 2 will add: autonomous tool use (agentic loops over `*.`
//! capabilities), streaming responses, the studio chat UI, and the
//! auto-extraction pass that mines facts from past turns.

pub mod events;
pub mod extractor;
pub mod knowledge;
pub mod orchestration;
pub mod prompt;
pub mod recall;
pub mod seeder;
pub mod service;
pub mod store;
pub mod summarizer;
pub mod tools;
pub mod types;

pub use summarizer::{
    MechanicalSummarizer, ScriptedSummarizer, Summarizer, SummarizerError, SummarizerResult,
};

pub use events::{EventBroadcaster, TurnEvent};
pub use knowledge::KnowledgeStore;
pub use orchestration::AssistantOrchestration;
pub use prompt::{KNOWLEDGE_PREAMBLE, MEMORY_PREAMBLE};
pub use recall::FactStore;
pub use seeder::{KnowledgeSeeder, SeedReport};
pub use service::{sanitize_untrusted_turn_request, AssistantService};
pub use store::{embedding_from_bytes, embedding_to_bytes, fact_summaries, AssistantStore};
pub use tools::{ToolGateway, DEFAULT_ALLOWED_LANES, RESERVED_FROM_ASSISTANT};
pub use types::{
    AssistantError, AssistantResult, AssistantSession, Fact, FactSummary, KnowledgeEntry,
    KnowledgeKind, KnowledgeSummary, NewFact, NewKnowledge, RagHitSummary, RecalledFact,
    RecalledKnowledge, SessionWithTurns, SpeechRequest, SpeechResponse, ToolInvocation,
    TranscribeRequest, TranscriptResponse, Turn, TurnContext, TurnRequest, TurnResult,
    MAX_SUBAGENT_DEPTH,
};
