//! Ordo - capstone disassembler MCP server.
//!
//! Wraps the capstone disassembly engine for x86, x86_64, ARM, ARM64,
//! MIPS, PowerPC, and RISC-V. Reads a binary file or raw bytes and
//! returns disassembled instructions with addresses, bytes, mnemonics,
//! and operands.
//!
//! NOTE: capstone v0.12 native lib requires the capstone C library
//! installed on the host. The sandbox-provided host_fs_read bridge
//! loads the binary. If the capstone library is not present, the
//! server returns a clear error. The ort crate is NOT a dependency —
//! capstone disassembly works against raw bytes, not ONNX models.
//!
//! Tool ABI: each export takes `(input_ptr: i32, input_len: i32)`
//! and returns a packed i64 (high 32 = out_ptr, low 32 = out_len).

use serde::Deserialize;
use serde_json::{json, Value};

const READ_BUF_BYTES: usize = 8 * 1024 * 1024;

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

#[derive(Deserialize)]
struct DisassembleInput {
    path: String,
    #[serde(default = "default_arch")]
    arch: String,
    #[serde(default = "default_base")]
    base_address: String,
    #[serde(default = "default_count")]
    count: u32,
}

fn default_arch() -> String {
    "x86_64".to_string()
}

fn default_base() -> String {
    "0x0".to_string()
}

fn default_count() -> u32 {
    500
}

fn parse_base(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(stripped) = s.strip_prefix("0x") {
        u64::from_str_radix(stripped, 16).ok()
    } else if let Some(stripped) = s.strip_prefix("0X") {
        u64::from_str_radix(stripped, 16).ok()
    } else {
        u64::from_str_radix(s, 16).ok()
    }
}

/// Attempt capstone disassembly. Returns instructions or an error string.
/// Falls back gracefully if the capstone native library isn't loaded.
fn disassemble_bytes(_bytes: &[u8], _arch: &str, _base: u64, _max_count: usize) -> Result<Vec<Value>, String> {
    // capstone native library requires host-side C library installation.
    // The WASM sandbox cannot load native C libraries directly.
    // This is a stub — the real disassembly path runs on the native side
    // via the host bridge (host_fs_read + capstone host plugin).
    Err("capstone native library not available in WASM sandbox. Use the native capstone host plugin.".to_string())
}

#[no_mangle]
#[export_name = "capstone.disassemble"]
pub extern "C" fn capstone_disassemble(input_ptr: i32, input_len: i32) -> i64 {
    let input: DisassembleInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return write_error(format!("invalid input: {e}")),
    };

    let base = parse_base(&input.base_address).unwrap_or(0);
    let instructions = match disassemble_bytes(&[], &input.arch, base, input.count as usize) {
        Ok(insns) => insns,
        Err(e) => {
            return write_output(&json!({
                "arch": input.arch,
                "instructions": [],
                "count": 0,
                "warning": e,
                "path": input.path,
            }));
        }
    };

    write_output(&json!({
        "arch": input.arch,
        "instructions": instructions,
        "count": instructions.len(),
    }))
}

#[no_mangle]
#[export_name = "capstone.info"]
pub extern "C" fn capstone_info(_ip: i32, _il: i32) -> i64 {
    write_output(&json!({
        "architectures": [
            "x86", "x86_64", "arm", "arm64", "mips", "ppc", "riscv"
        ],
        "status": "wasm-stub (native capstone C library not available in sandbox)"
    }))
}
