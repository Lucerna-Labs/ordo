use std::path::PathBuf;

use async_trait::async_trait;
use tokio::process::Command;

pub type ModelResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub text: String,
}

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn complete(&self, request: ModelRequest) -> ModelResult<ModelResponse>;
}

#[derive(Debug, Clone)]
pub struct StaticModelClient {
    response: ModelResponse,
}

impl StaticModelClient {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            response: ModelResponse { text: text.into() },
        }
    }
}

#[async_trait]
impl ModelClient for StaticModelClient {
    async fn complete(&self, _request: ModelRequest) -> ModelResult<ModelResponse> {
        Ok(self.response.clone())
    }
}

#[derive(Debug, Clone)]
pub struct LlamaCppConfig {
    pub binary_path: PathBuf,
    pub model_path: PathBuf,
    pub context_size: usize,
    pub max_tokens: usize,
    pub temperature: f32,
    pub extra_args: Vec<String>,
}

impl LlamaCppConfig {
    pub fn is_usable(&self) -> bool {
        self.binary_path.exists() && self.model_path.exists()
    }
}

#[derive(Debug, Clone)]
pub struct LlamaCppClient {
    config: LlamaCppConfig,
}

impl LlamaCppClient {
    pub fn new(config: LlamaCppConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &LlamaCppConfig {
        &self.config
    }
}

#[async_trait]
impl ModelClient for LlamaCppClient {
    async fn complete(&self, request: ModelRequest) -> ModelResult<ModelResponse> {
        let mut command = Command::new(&self.config.binary_path);
        command
            .arg("-m")
            .arg(&self.config.model_path)
            .arg("-p")
            .arg(&request.prompt)
            .arg("-c")
            .arg(self.config.context_size.to_string())
            .arg("-n")
            .arg(self.config.max_tokens.to_string())
            .arg("--temp")
            .arg(self.config.temperature.to_string())
            .arg("--no-display-prompt");

        for argument in &self.config.extra_args {
            command.arg(argument);
        }

        let output = command.output().await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("llama.cpp exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(message.into());
        }

        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(ModelResponse { text })
    }
}

#[derive(Debug, Clone)]
pub struct EmbeddingRequest {
    pub input: String,
}

#[derive(Debug, Clone)]
pub struct EmbeddingResponse {
    pub vector: Vec<f32>,
}

pub trait EmbeddingClient: Send + Sync {
    fn embed(&self, request: EmbeddingRequest) -> ModelResult<EmbeddingResponse>;
    fn dimensions(&self) -> usize;
    fn backend_name(&self) -> &str;
}

#[derive(Debug, Clone)]
pub struct HashingEmbedder {
    dimensions: usize,
}

impl Default for HashingEmbedder {
    fn default() -> Self {
        Self { dimensions: 384 }
    }
}

impl HashingEmbedder {
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions: dimensions.max(8),
        }
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }
}

impl EmbeddingClient for HashingEmbedder {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn backend_name(&self) -> &str {
        "hashing"
    }

    fn embed(&self, request: EmbeddingRequest) -> ModelResult<EmbeddingResponse> {
        let mut vector = vec![0.0f32; self.dimensions];
        let tokens = lexical_tokens(&request.input);

        for token in &tokens {
            add_hashed_feature(&mut vector, "tok", token, 1.0);

            for variant in lexical_variants(token) {
                add_hashed_feature(&mut vector, "variant", &variant, 0.45);
            }

            for gram in token.as_bytes().windows(3) {
                add_hashed_bytes_feature(&mut vector, "c3", gram, 0.28);
            }
            for gram in token.as_bytes().windows(4) {
                add_hashed_bytes_feature(&mut vector, "c4", gram, 0.18);
            }
        }

        for window in tokens.windows(2) {
            let phrase = format!("{} {}", window[0], window[1]);
            add_hashed_feature(&mut vector, "bi", &phrase, 0.75);
        }
        for window in tokens.windows(3) {
            let phrase = format!("{} {} {}", window[0], window[1], window[2]);
            add_hashed_feature(&mut vector, "tri", &phrase, 0.35);
        }

        normalize(&mut vector);
        Ok(EmbeddingResponse { vector })
    }
}

#[derive(Debug, Clone)]
pub struct LlamaCppEmbeddingConfig {
    pub binary_path: PathBuf,
    pub model_path: PathBuf,
    pub context_size: usize,
    pub extra_args: Vec<String>,
}

impl LlamaCppEmbeddingConfig {
    pub fn is_usable(&self) -> bool {
        self.binary_path.exists() && self.model_path.exists()
    }
}

pub struct LlamaCppEmbedder {
    config: LlamaCppEmbeddingConfig,
    dimensions: usize,
}

impl LlamaCppEmbedder {
    pub fn new(config: LlamaCppEmbeddingConfig, dimensions: usize) -> Self {
        Self {
            config,
            dimensions: dimensions.max(8),
        }
    }

    pub fn config(&self) -> &LlamaCppEmbeddingConfig {
        &self.config
    }
}

impl EmbeddingClient for LlamaCppEmbedder {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn backend_name(&self) -> &str {
        "llama.cpp"
    }

    fn embed(&self, request: EmbeddingRequest) -> ModelResult<EmbeddingResponse> {
        use std::process::Command;

        let mut command = Command::new(&self.config.binary_path);
        command
            .arg("-m")
            .arg(&self.config.model_path)
            .arg("-p")
            .arg(&request.input)
            .arg("-c")
            .arg(self.config.context_size.to_string())
            .arg("--embd-normalize")
            .arg("2")
            .arg("--embd-output-format")
            .arg("json")
            .arg("--log-disable");

        for argument in &self.config.extra_args {
            command.arg(argument);
        }

        let output = command.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("llama.cpp embedding exited with status {}", output.status)
            } else {
                stderr
            };
            return Err(message.into());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let raw = stdout.trim();
        let vector = parse_llama_cpp_embedding(raw, self.dimensions)?;
        Ok(EmbeddingResponse { vector })
    }
}

/// Embeds via a running Ollama server's `/api/embed` endpoint (e.g.
/// `nomic-embed-text`). Unlike `LlamaCppEmbedder`, this does NOT reload a
/// model per call — Ollama keeps the model warm — and it uses whatever
/// embedding model the operator already has pulled. Synchronous on
/// purpose (the trait is sync); the HTTP call uses `ureq` so it is safe to
/// invoke from a Tokio worker thread.
#[derive(Debug, Clone)]
pub struct OllamaEmbeddingConfig {
    /// Base URL of the Ollama server, e.g. `http://127.0.0.1:11434`.
    pub base_url: String,
    /// Embedding model name, e.g. `nomic-embed-text`.
    pub model: String,
}

pub struct OllamaEmbedder {
    config: OllamaEmbeddingConfig,
    dimensions: usize,
}

impl OllamaEmbedder {
    pub fn new(config: OllamaEmbeddingConfig, dimensions: usize) -> Self {
        Self {
            config,
            dimensions: dimensions.max(8),
        }
    }

    pub fn config(&self) -> &OllamaEmbeddingConfig {
        &self.config
    }
}

impl EmbeddingClient for OllamaEmbedder {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn backend_name(&self) -> &str {
        "ollama"
    }

    fn embed(&self, request: EmbeddingRequest) -> ModelResult<EmbeddingResponse> {
        let url = format!("{}/api/embed", self.config.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.config.model,
            "input": request.input,
        })
        .to_string();

        let response = ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|err| -> Box<dyn std::error::Error + Send + Sync> {
                format!("ollama embed request to {url} failed: {err}").into()
            })?;
        let text = response.into_string()?;
        let vector = parse_ollama_embedding(&text, self.dimensions)?;
        Ok(EmbeddingResponse { vector })
    }
}

/// Parse an Ollama embeddings response, tolerating the three shapes Ollama
/// has shipped: `/api/embed` -> `{"embeddings": [[...]]}`; older
/// `/api/embeddings` -> `{"embedding": [...]}`; OpenAI-compatible
/// `/v1/embeddings` -> `{"data": [{"embedding": [...]}]}`.
fn parse_ollama_embedding(raw: &str, expected_dimensions: usize) -> ModelResult<Vec<f32>> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|err| -> Box<dyn std::error::Error + Send + Sync> {
            format!("invalid ollama embedding JSON: {err}").into()
        })?;

    let parsed = value
        .get("embeddings")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(json_floats)
        .or_else(|| value.get("embedding").and_then(json_floats))
        .or_else(|| {
            value
                .get("data")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first| first.get("embedding"))
                .and_then(json_floats)
        });

    let mut vector = parsed.ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
        "ollama response contained no embedding vector".into()
    })?;
    if vector.is_empty() {
        return Err("ollama returned an empty embedding".into());
    }
    resize_embedding(&mut vector, expected_dimensions);
    normalize(&mut vector);
    Ok(vector)
}

fn json_floats(value: &serde_json::Value) -> Option<Vec<f32>> {
    let array = value.as_array()?;
    let mut out = Vec::with_capacity(array.len());
    for item in array {
        out.push(item.as_f64()? as f32);
    }
    Some(out)
}

fn parse_llama_cpp_embedding(raw: &str, expected_dimensions: usize) -> ModelResult<Vec<f32>> {
    // Try JSON array format first: [[0.1, 0.2, ...]] or [0.1, 0.2, ...]
    if let Ok(outer) = serde_json::from_str::<Vec<Vec<f32>>>(raw) {
        if let Some(vector) = outer.into_iter().next() {
            let mut vector = vector;
            resize_embedding(&mut vector, expected_dimensions);
            normalize(&mut vector);
            return Ok(vector);
        }
    }
    if let Ok(vector) = serde_json::from_str::<Vec<f32>>(raw) {
        let mut vector = vector;
        resize_embedding(&mut vector, expected_dimensions);
        normalize(&mut vector);
        return Ok(vector);
    }

    // Fallback: space-separated floats (e.g. "embedding 0: 0.1 0.2 ...")
    let floats: Vec<f32> = raw
        .split_whitespace()
        .filter_map(|token| token.parse::<f32>().ok())
        .collect();
    if floats.is_empty() {
        return Err(format!(
            "could not parse llama.cpp embedding output ({} bytes)",
            raw.len()
        )
        .into());
    }

    let mut vector = floats;
    resize_embedding(&mut vector, expected_dimensions);
    normalize(&mut vector);
    Ok(vector)
}

fn resize_embedding(vector: &mut Vec<f32>, target: usize) {
    if vector.len() > target {
        vector.truncate(target);
    } else if vector.len() < target {
        vector.resize(target, 0.0);
    }
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || right.is_empty() || left.len() != right.len() {
        return 0.0;
    }

    left.iter()
        .zip(right.iter())
        .map(|(lhs, rhs)| lhs * rhs)
        .sum::<f32>()
}

fn normalize(vector: &mut [f32]) {
    let magnitude = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if magnitude <= f32::EPSILON {
        return;
    }

    for value in vector {
        *value /= magnitude;
    }
}

fn lexical_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if ch.is_alphanumeric() {
            for lowered in ch.to_lowercase() {
                current.push(lowered);
            }
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn lexical_variants(token: &str) -> Vec<String> {
    let mut variants = Vec::new();
    push_unique_variant(&mut variants, token.strip_suffix("'s"));
    push_unique_variant(
        &mut variants,
        token.strip_suffix('s').filter(|_| token.len() > 4),
    );
    push_unique_variant(
        &mut variants,
        token.strip_suffix("ing").filter(|_| token.len() > 6),
    );
    push_unique_variant(
        &mut variants,
        token.strip_suffix("ed").filter(|_| token.len() > 5),
    );
    variants
}

fn push_unique_variant(variants: &mut Vec<String>, candidate: Option<&str>) {
    let Some(candidate) = candidate else {
        return;
    };
    if candidate.len() < 3 || variants.iter().any(|value| value == candidate) {
        return;
    }
    variants.push(candidate.to_string());
}

fn add_hashed_feature(vector: &mut [f32], namespace: &str, feature: &str, weight: f32) {
    add_hashed_bytes_feature(vector, namespace, feature.as_bytes(), weight);
}

fn add_hashed_bytes_feature(vector: &mut [f32], namespace: &str, feature: &[u8], weight: f32) {
    if vector.is_empty() || feature.is_empty() {
        return;
    }

    let mut keyed = Vec::with_capacity(namespace.len() + 1 + feature.len());
    keyed.extend_from_slice(namespace.as_bytes());
    keyed.push(0);
    keyed.extend_from_slice(feature);

    let hash = stable_hash_bytes(&keyed);
    let index = hash as usize % vector.len();
    let sign = if hash & (1 << 63) == 0 { 1.0 } else { -1.0 };
    vector[index] += weight * sign;
}

fn stable_hash_bytes(input: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in input {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{
        cosine_similarity, parse_llama_cpp_embedding, parse_ollama_embedding, EmbeddingClient,
        EmbeddingRequest, HashingEmbedder, ModelClient, ModelRequest, StaticModelClient,
    };

    #[test]
    fn parses_ollama_api_embed_shape() {
        let raw = r#"{"model":"nomic-embed-text","embeddings":[[0.1,0.2,0.3,0.4]]}"#;
        let vector = parse_ollama_embedding(raw, 4).expect("parse /api/embed");
        assert_eq!(vector.len(), 4);
    }

    #[test]
    fn parses_ollama_legacy_and_openai_shapes() {
        let legacy = r#"{"embedding":[1.0,0.0,0.0,0.0]}"#;
        assert_eq!(parse_ollama_embedding(legacy, 4).unwrap().len(), 4);
        let openai = r#"{"data":[{"embedding":[0.0,1.0,0.0,0.0]}]}"#;
        assert_eq!(parse_ollama_embedding(openai, 4).unwrap().len(), 4);
    }

    #[test]
    fn ollama_resizes_to_expected_dimensions() {
        // A 4-d vector requested at 8 dims is zero-padded to 8.
        let raw = r#"{"embeddings":[[0.5,0.5,0.5,0.5]]}"#;
        assert_eq!(parse_ollama_embedding(raw, 8).unwrap().len(), 8);
    }

    #[test]
    fn ollama_empty_vector_is_error() {
        assert!(parse_ollama_embedding(r#"{"embeddings":[[]]}"#, 4).is_err());
        assert!(parse_ollama_embedding(r#"{"unexpected":true}"#, 4).is_err());
    }

    #[test]
    fn hashing_embedder_is_deterministic() {
        let embedder = HashingEmbedder::default();
        let left = embedder
            .embed(EmbeddingRequest {
                input: "transport relay fallback".to_string(),
            })
            .expect("left embedding");
        let right = embedder
            .embed(EmbeddingRequest {
                input: "transport relay fallback".to_string(),
            })
            .expect("right embedding");

        assert_eq!(left.vector, right.vector);
    }

    #[test]
    fn cosine_similarity_prefers_related_inputs() {
        let embedder = HashingEmbedder::default();
        let anchor = embedder
            .embed(EmbeddingRequest {
                input: "transport relay fallback".to_string(),
            })
            .expect("anchor");
        let related = embedder
            .embed(EmbeddingRequest {
                input: "relay transport fallback design".to_string(),
            })
            .expect("related");
        let distant = embedder
            .embed(EmbeddingRequest {
                input: "filesystem write content".to_string(),
            })
            .expect("distant");

        assert!(
            cosine_similarity(&anchor.vector, &related.vector)
                > cosine_similarity(&anchor.vector, &distant.vector)
        );
    }

    #[tokio::test]
    async fn static_model_client_returns_scripted_response() {
        let client = StaticModelClient::new("SUMMARY: fine");
        let response = client
            .complete(ModelRequest {
                prompt: "ignored".to_string(),
            })
            .await
            .expect("static model response");

        assert_eq!(response.text, "SUMMARY: fine");
    }

    #[test]
    fn hashing_embedder_reports_backend_name() {
        let embedder = HashingEmbedder::default();
        assert_eq!(embedder.backend_name(), "hashing");
        assert_eq!(embedder.dimensions(), 384);
    }

    #[test]
    fn parse_llama_cpp_json_array_of_arrays() {
        let raw = "[[0.1, 0.2, 0.3, 0.4]]";
        let vector = parse_llama_cpp_embedding(raw, 4).expect("parsed embedding");
        assert_eq!(vector.len(), 4);
        assert!(vector.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn parse_llama_cpp_flat_json_array() {
        let raw = "[0.5, 0.3, 0.1, 0.8]";
        let vector = parse_llama_cpp_embedding(raw, 4).expect("parsed embedding");
        assert_eq!(vector.len(), 4);
    }

    #[test]
    fn parse_llama_cpp_resizes_to_target_dimensions() {
        let raw = "[0.1, 0.2]";
        let vector = parse_llama_cpp_embedding(raw, 4).expect("parsed embedding");
        assert_eq!(vector.len(), 4);
    }

    #[test]
    fn parse_llama_cpp_truncates_to_target_dimensions() {
        let raw = "[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]";
        let vector = parse_llama_cpp_embedding(raw, 4).expect("parsed embedding");
        assert_eq!(vector.len(), 4);
    }
}
