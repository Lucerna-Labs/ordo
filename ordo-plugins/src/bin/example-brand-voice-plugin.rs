//! A slightly more interesting reference plugin: exposes
//! `brand.voice_check` which scores a piece of copy against a tiny set
//! of brand-voice heuristics (hedge words, exclamation density,
//! forbidden marketing clichés). Shows how a plugin can contribute to
//! a *new* lane (`brand.*`) without touching the core runtime.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

const HEDGE_WORDS: &[&str] = &["maybe", "perhaps", "sort of", "kind of", "somewhat"];
const CLICHES: &[&str] = &[
    "game-changer",
    "synergy",
    "move the needle",
    "at the end of the day",
    "best-in-class",
];

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();

        let response = match (method, id) {
            ("initialize", Some(id)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {
                        "name": "example-brand-voice-plugin",
                        "version": "0.1.0",
                    }
                }
            }),
            ("notifications/initialized", _) => continue,
            ("tools/list", Some(id)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "brand.voice_check",
                        "description": "Score a block of copy against basic brand-voice heuristics (hedges, exclamation density, clichés).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" }
                            },
                            "required": ["text"]
                        }
                    }]
                }
            }),
            ("tools/call", Some(id)) => {
                let name = message
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if name != "brand.voice_check" {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": format!("unknown tool '{name}'")}
                    })
                } else {
                    let text = message
                        .pointer("/params/arguments/text")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let lower = text.to_lowercase();
                    let hedges: Vec<&&str> =
                        HEDGE_WORDS.iter().filter(|w| lower.contains(**w)).collect();
                    let clichés: Vec<&&str> =
                        CLICHES.iter().filter(|w| lower.contains(**w)).collect();
                    let exclamations = text.matches('!').count();
                    let word_count = text.split_whitespace().count().max(1);
                    let exclamation_rate = exclamations as f64 / word_count as f64;
                    let score = 1.0
                        - (hedges.len() as f64 * 0.08
                            + clichés.len() as f64 * 0.15
                            + exclamation_rate * 0.4)
                            .min(1.0);
                    let summary = format!(
                        "Brand voice score: {:.2} ({} hedges, {} clichés, {} exclamations)",
                        score,
                        hedges.len(),
                        clichés.len(),
                        exclamations,
                    );
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": summary}],
                            "isError": false,
                            "_structured": {
                                "score": score,
                                "hedge_words": hedges.iter().map(|s| **s).collect::<Vec<_>>(),
                                "cliches": clichés.iter().map(|s| **s).collect::<Vec<_>>(),
                                "exclamations": exclamations,
                                "word_count": word_count,
                            }
                        }
                    })
                }
            }
            _ => continue,
        };
        let _ = writeln!(out, "{response}");
        let _ = out.flush();
    }
}
