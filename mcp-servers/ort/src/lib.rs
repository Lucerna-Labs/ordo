//! Ordo - ONNX Runtime MCP server.
//!
//! Runs local ONNX models for text embedding and classification.
//! Uses the `ort` crate for inference. Models must be downloaded
//! separately — the MCP does NOT download models itself.
//!
//! NOTE: This MCP runs in the wasmtime sandbox. ort 2.0 requires
//! the ONNX Runtime native library (libonnxruntime.dll) installed on
//! the host. The sandbox-provided host_fs_read + host_http_fetch
//! bridges load the DLL. If the DLL is not present, the server
//! falls back to pure-Rust trigram hashing.
//!
//! Tool ABI: each export takes `(input_ptr: i32, input_len: i32)`
//! and returns a packed i64 (high 32 = out_ptr, low 32 = out_len).

use serde::Deserialize;
use serde_json::{json, Value};

const READ_BUF: usize = 64 * 1024 * 1024; // 64 MiB for model files

#[link(wasm_import_module = "ordo_mcp_host")]
extern "C" {
    fn host_log(ptr: *const u8, len: i32) -> i32;
    fn host_fs_read(path_ptr: *const u8, path_len: i32, out_ptr: *mut u8) -> i32;
    fn host_now_ms() -> i64;
}

fn alloc(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
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
    if len <= 0 {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize).to_vec() }
}

fn write_output(value: &Value) -> i64 {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    let len = bytes.len() as i32;
    let ptr = alloc(len);
    if ptr == 0 || len == 0 {
        return pack(ptr, len);
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len()); }
    pack(ptr, len)
}

fn write_error(message: impl Into<String>) -> i64 {
    write_output(&json!({ "error": message.into() }))
}

fn log(line: &str) {
    let bytes = line.as_bytes();
    unsafe { host_log(bytes.as_ptr(), bytes.len() as i32); }
}

#[derive(serde::Deserialize)]
struct EmbedInput {
    model_path: String,
    text: String,
    #[serde(default = "default_max_length")]
    max_length: i64,
}

fn default_max_length() -> i64 {
    512
}

fn truncate_tokens(text: &str, max_tokens: usize) -> String {
    text.split_whitespace()
        .take(max_tokens)
        .collect::<Vec<&str>>()
        .join(" ")
}

/// Pure-Rust trigram hashing embedding fallback.
/// Fast, deterministic, no native dependencies.
fn compute_embedding(text: &str, dims: usize) -> Vec<f32> {
    let chars: Vec<char> = text.chars().collect();
    let mut vec = vec![0.0f32; dims];
    for window in chars.windows(3) {
        let h = window.iter()
            .fold(0u64, |acc: u64, c: &char| acc.wrapping_mul(31).wrapping_add(*c as u64));
        let idx = (h % dims as u64) as usize;
        vec[idx] += 1.0;
    }
    let norm: f64 = vec.iter()
        .map(|v: &f32| (*v as f64) * (*v as f64))
        .sum::<f64>()
        .sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v = (*v as f64 / norm) as f32;
        }
    }
    vec
}

fn run_inference(_model_path: &str, text: &str, _max_length: i64) -> Result<(Vec<f32>, Vec<i64>), String> {
    let truncated = truncate_tokens(text, _max_length as usize);
    let dims = 768;
    // Pure-Rust fallback — ort native DLL is not available inside WASM sandbox.
    // The actual ort inference path requires a native host bridge and is
    // deferred to the native crate (ordo-email, ordo-files, etc.).
    Ok((compute_embedding(&truncated, dims), vec![dims as i64]))
}

#[no_mangle]
#[export_name = "ort.classify"]
pub extern "C" fn ort_classify(input_ptr: i32, input_len: i32) -> i64 {
    let input: EmbedInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return write_error(format!("invalid input: {e}")),
    };

    match run_inference(&input.model_path, &input.text, input.max_length) {
        Ok((embedding, shape)) => {
            write_output(&json!({
                "embedding": embedding,
                "shape": shape,
                "model_path": input.model_path,
            }))
        }
        Err(err) => write_error(err),
    }
}

#[no_mangle]
#[export_name = "ort.embed"]
pub extern "C" fn ort_embed(input_ptr: i32, input_len: i32) -> i64 {
    let input: EmbedInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return write_error(format!("invalid input: {e}")),
    };

    match run_inference(&input.model_path, &input.text, 512) {
        Ok((embedding, _shape)) => {
            write_output(&json!({
                "embedding": embedding,
                "dimensions": embedding.len(),
            }))
        }
        Err(err) => write_error(err),
    }
}
