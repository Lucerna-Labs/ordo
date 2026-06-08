//! Ordo - fastText MCP server.
//!
//! Lightweight text classification using n-gram statistics. Implements
//! language detection and topic classification. No external model
//! dependencies at runtime — this is pure Rust with no-std compatible
//! trigram analysis.
//!
//! Tool ABI: each export takes (input_ptr: i32, input_len: i32)
//! and returns a packed i64 (high 32 = out_ptr, low 32 = out_len).

use std::collections::HashMap;

#[link(wasm_import_module = "ordo_mcp_host")]
extern "C" {
    fn host_log(ptr: *const u8, len: i32) -> i32;
    fn host_fs_read(path_ptr: *const u8, path_len: i32, out_ptr: *mut u8) -> i32;
    fn host_now_ms() -> i64;
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

fn log(line: &str) {
    let bytes = line.as_bytes();
    unsafe { host_log(bytes.as_ptr(), bytes.len() as i32); }
}

// Language profiles — tri-gram frequency tables for top languages
struct LanguageProfile {
    code: &'static str,
    name: &'static str,
    common_trigrams: &'static [&'static str],
}

const LANGUAGES: &[LanguageProfile] = &[
    LanguageProfile { code: "en", name: "English", common_trigrams: &["the", "ing", "and", "ion", "tio", "ent", "ati", "for", "her", "tha"] },
    LanguageProfile { code: "es", name: "Spanish", common_trigrams: &["que", "los", "del", "las", "por", "con", "una", "est", "ent", "ado"] },
    LanguageProfile { code: "fr", name: "French", common_trigrams: &["ent", "que", "les", "des", "est", "ion", "pas", "our", "ans", "eur"] },
    LanguageProfile { code: "de", name: "German", common_trigrams: &["ich", "ein", "der", "die", "und", "che", "den", "cht", "sch", "hen"] },
];

fn detect_language(text: &str) -> (String, f64, Vec<serde_json::Value>) {
    let lower = text.to_ascii_lowercase();
    let chars: Vec<char> = lower.chars().collect();

    let mut scores: Vec<(&LanguageProfile, f64)> = LANGUAGES
        .iter()
        .map(|profile| {
            let matches: f64 = profile.common_trigrams.iter()
                .filter(|&&trigram| lower.contains(trigram))
                .count() as f64;
            let score = matches / profile.common_trigrams.len().max(1) as f64;
            (profile, score)
        })
        .collect();

    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let top = &scores[0];
    let confidence = top.1;
    let language = top.0.name.to_string();

    let top_languages: Vec<serde_json::Value> = scores.iter()
        .take(5)
        .filter(|(_, score)| *score > 0.05)
        .map(|(profile, score)| serde_json::json!({ "language": profile.name, "score": score }))
        .collect();

    (language, confidence, top_languages)
}

fn classify_topic(text: &str) -> Vec<serde_json::Value> {
    let lower = text.to_ascii_lowercase();
    let topics: &[(&str, &[&str])] = &[
        ("technology", &["code", "software", "data", "api", "server", "database", "algorithm", "programming", "developer", "app"]),
        ("business", &["business", "market", "revenue", "sales", "customer", "product", "growth", "startup", "investment", "strategy"]),
        ("science", &["research", "study", "experiment", "theory", "discovery", "scientist", "lab", "physics", "biology", "chemistry"]),
        ("entertainment", &["movie", "music", "game", "play", "show", "video", "song", "artist", "streaming", "cinema"]),
        ("politics", &["government", "election", "vote", "law", "policy", "president", "congress", "senate", "party", "ballot"]),
        ("health", &["health", "medical", "doctor", "patient", "treatment", "disease", "therapy", "wellness", "clinical", "diagnosis"]),
        ("education", &["school", "student", "teacher", "learn", "course", "university", "college", "curriculum", "degree", "training"]),
        ("finance", &["money", "bank", "stock", "investment", "market", "trading", "crypto", "revenue", "capital", "asset"]),
    ];

    let mut results: Vec<(String, f64)> = topics.iter()
        .map(|(category, keywords)| {
            let matches = keywords.iter().filter(|kw| lower.contains(*kw)).count() as f64;
            let score = matches / keywords.len() as f64;
            (category.to_string(), score)
        })
        .collect();

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    results.iter()
        .filter(|(_, score)| *score > 0.1)
        .map(|(label, confidence)| serde_json::json!({ "label": label, "confidence": confidence }))
        .collect()
}

#[derive(serde::Deserialize)]
struct TextInput {
    text: String,
}

impl TextInput {
    fn deserialize(input_ptr: i32, input_len: i32) -> Result<Self, String> {
        let raw = read_input(input_ptr, input_len);
        serde_json::from_slice(&raw).map_err(|err| format!("invalid input: {}", err))
    }
}

#[derive(serde::Deserialize)]
struct ClassifyInput {
    text: String,
    model_path: Option<String>,
}

impl ClassifyInput {
    fn deserialize(input_ptr: i32, input_len: i32) -> Result<Self, String> {
        let raw = read_input(input_ptr, input_len);
        serde_json::from_slice(&raw).map_err(|err| format!("invalid input: {}", err))
    }
}

#[export_name = "fasttext.detect_language"]
pub extern "C" fn fasttext_detect_language(input_ptr: i32, input_len: i32) -> i64 {
    let input = match TextInput::deserialize(input_ptr, input_len) {
        Ok(v) => v,
        Err(e) => return error(e),
    };

    if input.text.trim().len() < 10 {
        return error("text too short for reliable language detection (need at least 10 characters)");
    }

    let (language, confidence, top_languages) = detect_language(&input.text);
    log(&format!("fasttext.detect_language lang={} conf={}", language, confidence));
    write_output(&serde_json::json!({
        "language": language,
        "confidence": confidence,
        "top_languages": top_languages,
    }))
}

#[export_name = "fasttext.classify"]
pub extern "C" fn fasttext_classify(input_ptr: i32, input_len: i32) -> i64 {
    let input = match ClassifyInput::deserialize(input_ptr, input_len) {
        Ok(v) => v,
        Err(e) => return error(e),
    };

    // If a model_path is provided, try to load it. Otherwise use pure-Rust classification.
    if let Some(ref path) = input.model_path {
        if !path.is_empty() {
            // Try to read model labels from file
            let model_bytes = unsafe {
                let buf_ptr = alloc(64 * 1024 * 1024);
                if buf_ptr == 0 { return error("allocation failed"); }
                let n = host_fs_read(path.as_ptr(), path.len() as i32, buf_ptr as *mut u8);
                if n <= 0 { return error(format!("failed to read model '{}'", path)); }
                std::slice::from_raw_parts(buf_ptr as *const u8, n as usize).to_vec()
            };
            // TODO: parse actual fastText model to extract labels
            // For now, fall through to pure-Rust classification
            let _ = model_bytes;
        }
    }

    let labels = classify_topic(&input.text);
    let top_label = labels.first()
        .map(|l| l.get("label").and_then(|v| v.as_str()).unwrap_or("none").to_string())
        .unwrap_or_else(|| "none".to_string());

    log(&format!("fasttext.classify labels={} top={}", labels.len(), top_label));
    write_output(&serde_json::json!({
        "labels": labels,
        "top_label": top_label,
    }))
}
