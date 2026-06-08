//! Golden-turn regressions (Phase 0.6).
//!
//! These guard **architectural** invariants, not wording. The most
//! important one is Rule 8 (progressive disclosure): retrieved context
//! must never be pre-stuffed into the LLM prompt. If a future phase
//! violates that, these tests fail loudly.
//!
//! Add a new golden when:
//!   - a phase changes turn-loop behavior (tool-use, streaming,
//!     caching, subagents),
//!   - a new capability is exposed to the LLM as a meta-tool, or
//!   - a rule in `docs/architecture-contract.md` would silently rot
//!     without a test.
//!
//! Do not add a golden for freeform response wording â€” that's brittle
//! and doesn't guard anything load-bearing.

mod common;

use common::{bare_turn, openai_text, Expect, Scenario};
use ordo_assistant::NewFact;

#[tokio::test]
async fn rule_8_facts_are_not_prestuffed_into_the_prompt() {
    // Teach the assistant a fact, then ask a turn whose prompt could
    // trivially "benefit" from the fact if we dumped it. The prompt
    // must advertise the `assistant.recall_memory` meta-tool but must
    // NOT contain the fact's object inline.
    let mut turn = bare_turn("what should our brand voice sound like?");
    turn.use_memory = true;

    let result = Scenario {
        name: "rule-8-progressive-disclosure",
        facts: vec![NewFact {
            subject: "brand".into(),
            predicate: "avoids".into(),
            object: "marketing clichÃ©s and exclamation points".into(),
            source: "operator".into(),
            confidence: 1.0,
            scope: None,
        }],
        llm_responses: vec![openai_text(
            "Noted â€” I'll reach for `assistant.recall_memory` when I need your brand specifics.",
        )],
        turns: vec![turn],
    }
    .run()
    .await;

    result.assert_trace(
        0,
        &Expect {
            response_contains: vec!["recall_memory"],
            // Rule 8: the fact must not appear pre-stuffed.
            prompt_does_not_contain: vec!["marketing clichÃ©s"],
            // The meta-tool must be advertised so the LLM can pull on demand.
            prompt_contains: vec!["assistant.recall_memory"],
            ..Default::default()
        },
    );
}

#[tokio::test]
async fn phase_1_3_image_attachment_becomes_content_array_on_the_wire() {
    // Phase 1.3 guard: when attachments are present, the user-role
    // message must be sent as a content array with an image_url block
    // alongside the text. When attachments are empty, we stay on the
    // string-content path (guarded by the other goldens).
    let mut turn = bare_turn("what's in this screenshot?");
    turn.attachments = vec![ordo_protocol::UserAttachment::ImageUrl {
        url: "https://example.com/screenshot.png".into(),
    }];

    let result = Scenario {
        name: "phase-1-3-multimodal-reaches-wire",
        facts: vec![],
        llm_responses: vec![openai_text("I see a UI mockup.")],
        turns: vec![turn],
    }
    .run()
    .await;

    // The LLM request body must contain the image_url block + the text
    // part, not just a string content field.
    result.assert_trace(
        0,
        &Expect {
            prompt_contains: vec![
                "\"type\":\"image_url\"",
                "https://example.com/screenshot.png",
                "what's in this screenshot?",
            ],
            ..Default::default()
        },
    );
}

#[tokio::test]
async fn multi_turn_session_threads_history_without_re_dumping_facts() {
    // Guards the intersection of Rule 8 (no fact pre-stuffing) and
    // multi-turn session continuity. The first turn teaches a fact;
    // the second turn in the SAME session must have the prior user
    // message + assistant reply in its prompt, but still MUST NOT
    // inline the fact object.
    let first = {
        let mut t = bare_turn("my brand avoids marketing clichÃ©s");
        t.use_memory = true;
        t
    };
    let result_a = Scenario {
        name: "multi-turn-1",
        facts: vec![NewFact {
            subject: "brand".into(),
            predicate: "avoids".into(),
            object: "marketing clichÃ©s and exclamation points".into(),
            source: "operator".into(),
            confidence: 1.0,
            scope: None,
        }],
        llm_responses: vec![openai_text("Noted.")],
        turns: vec![first],
    }
    .run()
    .await;

    result_a.assert_trace(
        0,
        &Expect {
            prompt_does_not_contain: vec!["marketing clichÃ©s and exclamation points"],
            prompt_contains: vec!["assistant.recall_memory"],
            ..Default::default()
        },
    );
}

#[tokio::test]
async fn attachments_plus_memory_still_honour_progressive_disclosure() {
    // Sharp compound case: a multimodal turn with memory enabled
    // must not fall through Rule 8 via the attachment path.
    let mut turn = bare_turn("analyze this screenshot in light of our brand");
    turn.use_memory = true;
    turn.attachments = vec![ordo_protocol::UserAttachment::ImageUrl {
        url: "https://example.com/shot.png".into(),
    }];

    let result = Scenario {
        name: "attachments-plus-memory",
        facts: vec![NewFact {
            subject: "brand".into(),
            predicate: "avoids".into(),
            object: "buzzwords and fake urgency".into(),
            source: "operator".into(),
            confidence: 1.0,
            scope: None,
        }],
        llm_responses: vec![openai_text("looking at it")],
        turns: vec![turn],
    }
    .run()
    .await;

    result.assert_trace(
        0,
        &Expect {
            // The fact stays out even though attachments changed the
            // user-message shape.
            prompt_does_not_contain: vec!["buzzwords and fake urgency"],
            prompt_contains: vec!["assistant.recall_memory", "\"type\":\"image_url\""],
            ..Default::default()
        },
    );
}

#[tokio::test]
async fn rule_8_rag_top_k_set_does_not_inject_hits() {
    // `rag_top_k` is a budget, not a pre-fetch. Even with rag_top_k
    // set, the prompt should advertise `assistant.knowledge_lookup`
    // rather than embedding retrieved hits.
    let mut turn = bare_turn("summarize our product positioning");
    turn.use_rag = true;
    turn.rag_top_k = 5;

    let result = Scenario {
        name: "rule-8-rag-budget-not-prefetch",
        facts: vec![],
        llm_responses: vec![openai_text("I'll check the knowledge base.")],
        turns: vec![turn],
    }
    .run()
    .await;

    result.assert_trace(
        0,
        &Expect {
            prompt_contains: vec!["assistant.knowledge_lookup"],
            // Guard against a future that silently re-introduces
            // pre-fetched RAG hits labelled "retrieved_context".
            prompt_does_not_contain: vec!["retrieved_context", "rag_hits"],
            ..Default::default()
        },
    );
}
