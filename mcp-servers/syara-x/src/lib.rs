//! Ordo - syara-x semantic YARA MCP server.
//!
//! Detects prompt injection, jailbreaks, phishing, credential exfiltration,
//! model extraction, and toxic content using semantic matching.
//!
//! This is a lightweight pure-Rust implementation. The full syara-x crate
//! with ML models is TBD; this v0.1 ships a fast path with pattern detection
//! and trigram similarity. Upgrade to full syara-x when it stabilizes.
//!
//! Tool ABI: each export takes (input_ptr: i32, input_len: i32)
//! and returns a packed i64 (high 32 = out_ptr, low 32 = out_len).

use std::collections::HashMap;

#[link(wasm_import_module = "ordo_mcp_host")]
extern "C" {
    fn host_log(ptr: *const u8, len: i32) -> i32;
}

fn alloc(n: i32) -> i32 {
    if n <= 0 { return 0; }
    let mut buf: Vec<u8> = Vec::with_capacity(n as usize);
    unsafe { buf.set_len(n as usize); }
    let ptr = buf.as_mut_ptr() as i32;
    std::mem::forget(buf);
    ptr
}

fn pack(ptr: i32, len: i32) -> i64 {
    ((ptr as u32 as u64) << 32 | (len as u32 as u64)) as i64
}

fn read_input(ptr: i32, len: i32) -> Vec<u8> {
    if len <= 0 { Vec::new() }
    else { unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize).to_vec() } }
}

fn write_output(value: &serde_json::Value) -> i64 {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    let len = bytes.len() as i32;
    let ptr = alloc(len);
    if ptr == 0 || len == 0 { return pack(ptr, len); }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len()); }
    pack(ptr, len)
}

fn error(message: impl Into<String>) -> i64 {
    write_output(&serde_json::json!({ "error": message.into() }))
}

fn log_msg(msg: &str) {
    let bytes = msg.as_bytes();
    unsafe { host_log(bytes.as_ptr(), bytes.len() as i32); }
}

// ─── Pattern dictionaries ───

const INJECTION_PATTERNS: &[&str] = &[
    "ignore previous instructions",
    "ignore all previous",
    "system prompt override",
    "you are now DAN",
    "developer mode",
    "jailbreak",
    "pretend you are",
    "act as if",
    "bypass content policy",
    "ethical guidelines do not apply",
    "new instructions override",
    "forget your training",
    "you must comply with",
    "do anything now",
];

const PHISHING_PATTERNS: &[&str] = &[
    "urgent action required",
    "verify your account",
    "account suspended",
    "security alert",
    "login attempt",
    "confirm your identity",
    "verify your credentials",
    "password reset",
    "unusual activity",
    "limited time offer",
    "click here",
    "download attachment",
];

const CREDENTIAL_PATTERNS: &[&str] = &[
    "api_key",
    "apikey",
    "api secret",
    "access_key",
    "secret_key",
    "private_key",
    "password",
    "token",
    "bearer",
    "authorization:",
];

fn check_keywords(text: &str, patterns: &[&str], min_hits: usize) -> (bool, Vec<serde_json::Value>) {
    let lower = text.to_ascii_lowercase();
    let matches: Vec<serde_json::Value> = patterns.iter()
        .filter_map(|pattern| {
            lower.match_indices(*pattern).next().map(|(offset, _)| {
                let end = (offset + pattern.len()).min(text.len());
                serde_json::json!({
                    "offset": offset,
                    "length": pattern.len(),
                    "snippet": &text[offset..end],
                })
            })
        })
        .collect();
    (matches.len() >= min_hits, matches)
}

fn trigrams(text: &str) -> HashMap<u32, usize> {
    let lower = text.to_ascii_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    if chars.len() >= 3 {
        chars.windows(3)
            .map(|w| {
                let a = w[0] as u32;
                let b = w[1] as u32;
                let c = w[2] as u32;
                a.wrapping_mul(65536).wrapping_add(b.wrapping_mul(256)).wrapping_add(c)
            })
            .fold(HashMap::new(), |mut acc, h| { *acc.entry(h).or_insert(0) += 1; acc })
    } else {
        chars.windows(2)
            .map(|w| {
                let a = w[0] as u32;
                let b = w[1] as u32;
                a.wrapping_mul(256).wrapping_add(b)
            })
            .fold(HashMap::new(), |mut acc, h| { *acc.entry(h).or_insert(0) += 1; acc })
    }
}

fn jaccard(a: &HashMap<u32, usize>, b: &HashMap<u32, usize>) -> f64 {
    let intersection = a.keys().filter(|k| b.contains_key(k)).count();
    let union = a.len() + b.len() - intersection;
    if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
}

#[derive(serde::Deserialize)]
struct ScanInput {
    text: String,
    rule: String,
}

#[derive(serde::Deserialize)]
struct ClassifyInput {
    text: String,
    categories: Vec<String>,
}

#[export_name = "syara-x.scan"]
pub extern "C" fn syara_scan(input_ptr: i32, input_len: i32) -> i64 {
    let input: ScanInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return error(format!("invalid input: {}", e)),
    };

    let (matched, confidence, reason, matches) = match input.rule.as_str() {
        "prompt_injection" => {
            let (m, m_matches) = check_keywords(&input.text, INJECTION_PATTERNS, 1);
            let conf = if m { 0.85 - 0.1 * (1.0 - (m_matches.len() as f64 / 4.0).min(1.0)) } else { 0.05 };
            (m, conf.max(0.0), "Semantic scan for prompt injection patterns".to_string(), m_matches)
        }
        "jailbreak" => {
            let (m1, pmatches) = check_keywords(&input.text, INJECTION_PATTERNS, 1);
            let (m2, _) = check_keywords(&input.text, &["illegal", "unethical", "harmful"], 2);
            let matched = m1 || m2;
            let conf = if matched { 0.75 } else { 0.1 };
            (matched, conf, "Combined semantic + ethical-boundary scan".to_string(), pmatches)
        }
        "phishing" => {
            let (m, pmatches) = check_keywords(&input.text, PHISHING_PATTERNS, 1);
            let conf = if m { 0.8 } else { 0.05 };
            (m, conf, "Phishing pattern detection".to_string(), pmatches)
        }
        "credential_leak" => {
            let (m, cmatches) = check_keywords(&input.text, CREDENTIAL_PATTERNS, 1);
            let has_b64 = input.text.contains("eyJ") || input.text.contains("sk-") || input.text.contains("AKIA");
            let matched = m || has_b64;
            let conf = if matched { 0.9 } else { 0.02 };
            (matched, conf, "Credential and secret pattern detection".to_string(), cmatches)
        }
        "model_extraction" => {
            let has_system = input.text.contains("system") || input.text.contains("system prompt");
            let asks_instructions = input.text.contains("instructions") || input.text.contains("rules") || input.text.contains("training");
            let matched = has_system && asks_instructions;
            let conf = if matched { 0.7 } else { 0.05 };
            (matched, conf, "Model extraction attempt detection".to_string(), Vec::new())
        }
        "toxic_content" => {
            let toxic = &["hate", "violence", "threat", "harass", "abuse", "discriminat"][..];
            let (m, tmatches) = check_keywords(&input.text, toxic, 1);
            let conf = if m { 0.8 } else { 0.05 };
            (m, conf, "Toxic content detection".to_string(), tmatches)
        }
        other => return error(format!("unknown rule: {}", other)),
    };

    log_msg(&format!("syara-x.scan rule={} matched={}", input.rule, matched));
    write_output(&serde_json::json!({
        "rule": input.rule,
        "matched": matched,
        "confidence": confidence,
        "reason": reason,
        "matches": matches,
    }))
}

#[export_name = "syara-x.classify"]
pub extern "C" fn syara_classify(input_ptr: i32, input_len: i32) -> i64 {
    let input: ClassifyInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return error(format!("invalid input: {}", e)),
    };

    let text_trigrams = trigrams(&input.text);

    let mut scores: Vec<(String, f64)> = input.categories.iter().map(|cat| {
        let cat_trigrams = trigrams(cat);
        let js = jaccard(&text_trigrams, &cat_trigrams);
        let boost = if input.text.to_ascii_lowercase().contains(&cat.to_ascii_lowercase()) { 0.2 } else { 0.0 };
        (cat.clone(), js + boost)
    }).collect();

    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let top_hit = scores.first().map(|(cat, _)| cat.clone()).unwrap_or_else(|| "none".to_string());
    let top_score = scores.first().map(|(_, s)| *s).unwrap_or(0.0);
    let flagged = top_score > 0.4;

    let results: Vec<serde_json::Value> = scores.into_iter()
        .map(|(category, score)| serde_json::json!({ "category": category, "score": score }))
        .collect();

    log_msg(&format!("syara-x.classify categories={} flagged={}", results.len(), flagged));
    write_output(&serde_json::json!({
        "results": results,
        "top_hit": top_hit,
        "flagged": flagged,
    }))
}
