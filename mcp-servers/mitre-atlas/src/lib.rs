//! Ordo - MITRE ATLAS threat detection MCP server.
//!
//! Implements MITRE ATLAS technique detection for LLM security. Covers
//! prompt injection, jailbreaking, credential exfiltration, model extraction,
//! and more. Pure-Rust implementation with keyword + pattern detection.
//!
//! Full mitre-atlas-rs crate integration pending; this v0.1 ships the
//! technique definitions inline. Upgrade path: swap detection backend
//! without changing the MCP interface.
//!
//! Tool ABI: each export takes (input_ptr: i32, input_len: i32)
//! and returns a packed i64 (high 32 = out_ptr, low 32 = out_len).

use serde_json::json;

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
    write_output(&json!({ "error": message.into() }))
}

fn log_msg(msg: &str) {
    let bytes = msg.as_bytes();
    unsafe { host_log(bytes.as_ptr(), bytes.len() as i32); }
}

// ─── Technique definitions ───

struct Technique {
    id: &'static str,
    name: &'static str,
    tactic: &'static str,
    keywords: &'static [&'static str],
    severity: &'static str,
}

impl Technique {
    fn detect(&self, text: &str) -> f64 {
        let lower = text.to_ascii_lowercase();
        let hits = self.keywords.iter().filter(|kw| lower.contains(*kw)).count() as f64;
        let max = self.keywords.len() as f64;
        if max == 0.0 { 0.0 } else { (hits / max).min(1.0) }
    }
}

const TECHNIQUES: &[Technique] = &[
    Technique { id: "AML.T0000", name: "Prompt Probing", tactic: "reconnaissance", keywords: &["test prompt", "testing the model", "checking capabilities", "probing boundaries"], severity: "low" },
    Technique { id: "AML.T0001", name: "System Prompt Extraction", tactic: "reconnaissance", keywords: &["what is your system prompt", "show me your instructions", "reveal your rules", "base instructions", "underlying prompt", "originally programmed"], severity: "medium" },
    Technique { id: "AML.T0010", name: "Direct Prompt Injection", tactic: "initial_access", keywords: &["ignore previous instructions", "ignore all previous", "new instructions", "forget your training", "you are now DAN"], severity: "critical" },
    Technique { id: "AML.T0011", name: "Indirect Prompt Injection", tactic: "initial_access", keywords: &["hidden text", "invisible instructions", "webpage content says", "email body contains", "document says"], severity: "high" },
    Technique { id: "AML.T0012", name: "Multi-turn Injection", tactic: "initial_access", keywords: &["earlier you said", "remember when", "previously established", "as discussed", "from now on"], severity: "high" },
    Technique { id: "AML.T0013", name: "Encoding Obfuscation", tactic: "initial_access", keywords: &["base64", "rot13", "hex", "encoded", "decode this", "translate from"], severity: "medium" },
    Technique { id: "AML.T0020", name: "LLM-Assisted Malicious Code", tactic: "execution", keywords: &["generate exploit", "write malware", "create virus", "hacking code", "exploit vulnerability", "shellcode"], severity: "critical" },
    Technique { id: "AML.T0021", name: "Payload Generation", tactic: "execution", keywords: &["generate payload", "create payload", "craft payload", "build payload"], severity: "high" },
    Technique { id: "AML.T0030", name: "Model Memory Poisoning", tactic: "persistence", keywords: &["remember this", "save this", "learn this", "training update", "add to memory", "store knowledge"], severity: "high" },
    Technique { id: "AML.T0031", name: "Backdoor Injection", tactic: "persistence", keywords: &["backdoor", "hidden trigger", "secret command", "override permanently", "always respond with"], severity: "critical" },
    Technique { id: "AML.T0032", name: "RAG Poisoning", tactic: "persistence", keywords: &["upload document", "index this", "store document", "add to knowledge base", "ingest content"], severity: "high" },
    Technique { id: "AML.T0040", name: "Content Evasion", tactic: "defense_evasion", keywords: &["bypass filter", "circumvent detection", "evade moderation", "dodge guardrails"], severity: "medium" },
    Technique { id: "AML.T0041", name: "Policy Bypass", tactic: "defense_evasion", keywords: &["policy exception", "override safety", "disable moderation", "turn off safety"], severity: "high" },
    Technique { id: "AML.T0042", name: "Roleplay Evasion", tactic: "defense_evasion", keywords: &["roleplay", "pretend", "imagine you are", "in a fictional world", "hypothetically"], severity: "medium" },
    Technique { id: "AML.T0050", name: "System Prompt Theft", tactic: "collection", keywords: &["output your prompt", "print your instructions", "show your system", "display your configuration", "dump your rules"], severity: "high" },
    Technique { id: "AML.T0051", name: "Training Data Extraction", tactic: "collection", keywords: &["training examples", "sample outputs", "training data", "data you were trained on", "example pairs"], severity: "critical" },
    Technique { id: "AML.T0060", name: "Credential Harvesting", tactic: "exfiltration", keywords: &["send to external", "email to", "post to", "upload to", "transmit data", "forward to", "share with"], severity: "critical" },
    Technique { id: "AML.T0061", name: "Data Leak via Output", tactic: "exfiltration", keywords: &["output contains", "response includes", "generated text contains", "return value"], severity: "high" },
    Technique { id: "AML.T0070", name: "Hallucination Exploitation", tactic: "impact", keywords: &["false information", "misinformation", "fake news", "incorrect claim", "fabricated data"], severity: "medium" },
    Technique { id: "AML.T0071", name: "Reputation Damage", tactic: "impact", keywords: &["offensive output", "harmful content", "dangerous advice", "hate speech"], severity: "high" },
];

fn run_detection(text: &str) -> Vec<serde_json::Value> {
    TECHNIQUES.iter()
        .filter_map(|tech| {
            let conf = tech.detect(text);
            if conf > 0.3 {
                Some(json!({
                    "id": tech.id,
                    "name": tech.name,
                    "tactic": tech.tactic,
                    "confidence": conf,
                    "severity": tech.severity,
                }))
            } else {
                None
            }
        })
        .collect()
}

#[derive(serde::Deserialize)]
struct DetectInput {
    text: String,
    #[serde(default)]
    context: Option<String>,
}

#[export_name = "mitre-atlas.detect"]
pub extern "C" fn mitre_atlas_detect(input_ptr: i32, input_len: i32) -> i64 {
    let input: DetectInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return error(format!("invalid input: {}", e)),
    };

    let techniques = run_detection(&input.text);
    let matched = !techniques.is_empty();

    let summary = if matched {
        let tactic_set: std::collections::HashSet<&str> = techniques.iter()
            .filter_map(|t| t.get("tactic").and_then(|v| v.as_str()))
            .collect();
        format!("Detected {} technique(s) across {} tactic(s): {}", techniques.len(), tactic_set.len(), tactic_set.into_iter().collect::<Vec<_>>().join(", "))
    } else {
        "No MITRE ATLAS techniques detected.".to_string()
    };

    log_msg(&format!("mitre-atlas.detect techniques={}", techniques.len()));
    write_output(&json!({
        "matched": matched,
        "techniques": techniques,
        "summary": summary,
    }))
}

#[export_name = "mitre-atlas.audit"]
pub extern "C" fn mitre_atlas_audit(input_ptr: i32, input_len: i32) -> i64 {
    let input: DetectInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return error(format!("invalid input: {}", e)),
    };

    let techniques = run_detection(&input.text);
    let detected = techniques.len();

    let severity_score: f64 = techniques.iter()
        .map(|t| match t.get("severity").and_then(|v| v.as_str()).unwrap_or("low") {
            "critical" => 10.0,
            "high" => 6.0,
            "medium" => 3.0,
            _ => 1.0,
        })
        .sum();

    let total_possible = TECHNIQUES.len() as f64 * 10.0;
    let risk_score = if total_possible > 0.0 { (severity_score / total_possible).min(1.0) } else { 0.0 };
    let risk_level = if risk_score > 0.7 { "critical" } else if risk_score > 0.4 { "high" } else if risk_score > 0.15 { "medium" } else { "low" };

    let details: Vec<serde_json::Value> = techniques.into_iter()
        .map(|t| json!({ "id": t["id"], "name": t["name"], "tactic": t["tactic"], "confidence": t["confidence"] }))
        .collect();

    log_msg(&format!("mitre-atlas.audit detected={} risk={}", detected, risk_level));
    write_output(&json!({
        "risk_score": risk_score,
        "risk_level": risk_level,
        "techniques_detected": detected,
        "techniques_total": TECHNIQUES.len(),
        "details": details,
        "summary": format!("ATLAS audit complete: {} of {} techniques triggered. Risk score: {:.0}% ({}).", detected, TECHNIQUES.len(), risk_score * 100.0, risk_level),
    }))
}
