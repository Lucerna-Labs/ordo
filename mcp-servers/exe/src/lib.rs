//! Ordo - exe inspection MCP server.
//!
//! Inspects and verifies executable files using goblin 0.9. Provides
//! high-level format detection, section/symbol enumeration, and
//! header verification for build-step validation.
//!
//! Tool ABI: each export takes `(input_ptr: i32, input_len: i32)`
//! and returns a packed i64 (high 32 = out_ptr, low 32 = out_len).

use serde::Deserialize;
use serde_json::{json, Value};

const READ_BUF_BYTES: usize = 8 * 1024 * 1024; // 8 MiB cap

#[link(wasm_import_module = "ordo_mcp_host")]
extern "C" {
    fn host_log(ptr: *const u8, len: i32) -> i32;
    fn host_fs_read(path_ptr: *const u8, path_len: i32, out_ptr: *mut u8) -> i32;
    fn host_now_ms() -> i64;
    fn host_fs_write(
        path_ptr: *const u8,
        path_len: i32,
        bytes_ptr: *const u8,
        bytes_len: i32,
    ) -> i32;
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

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    let buf_ptr = alloc(READ_BUF_BYTES as i32);
    if buf_ptr == 0 { return Err("allocation failed".to_string()); }
    let n = unsafe { host_fs_read(path.as_ptr(), path.len() as i32, buf_ptr as *mut u8) };
    if n <= 0 { return Err(format!("failed to read file '{path}'")); }
    Ok(unsafe { std::slice::from_raw_parts(buf_ptr as *const u8, n as usize).to_vec() })
}

fn write_output(value: &Value) -> i64 {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    let len = bytes.len() as i32;
    let ptr = alloc(len);
    if ptr == 0 || len == 0 { return pack(ptr, len); }
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
struct PathInput {
    path: String,
}

// ─── exe.inspect ───

#[export_name = "exe.inspect"]
pub extern "C" fn exe_inspect(input_ptr: i32, input_len: i32) -> i64 {
    let input: PathInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return write_error(format!("invalid input: {e}")),
    };
    let bytes = match read_file(&input.path) {
        Ok(b) => b,
        Err(e) => return write_error(e),
    };
    if bytes.is_empty() { return write_error("file is empty"); }

    let file_size = bytes.len();
    let obj = match goblin::Object::parse(&bytes) {
        Ok(obj) => obj,
        Err(e) => return write_error(format!("failed to parse: {e}")),
    };

    let (fmt, arch, entry, sections, imports, exports) = match obj {
        goblin::Object::PE(pe) => {
            let arch = match pe.header.coff_header.machine {
                0x014c => "x86", 0x8664 => "x86_64", 0x01c4 => "arm", 0xaa64 => "arm64", _ => "unknown",
            };
            let entry = format!("{:#x}", pe.entry);
            let sections = pe.sections.len();
            let imports = pe.imports.len(); // PE imports is Vec<Import> — flat
            let exports = pe.exports.len(); // PE exports is Vec<Export> — flat
            ("pe".to_string(), arch.to_string(), entry, sections, imports, exports)
        }
        goblin::Object::Elf(elf) => {
            let arch = match elf.header.e_machine {
                0x03 => "x86", 0x3e => "x86_64", 0x28 => "arm", 0xb7 => "aarch64", _ => "unknown",
            };
            let entry = format!("{:#x}", elf.header.e_entry);
            let sections = elf.section_headers.len();
            let imports = elf.dynsyms.iter().filter(|s| s.is_import()).count();
            let exports = elf.dynsyms.iter().filter(|s| !s.is_import() && s.st_shndx != 0).count();
            ("elf".to_string(), arch.to_string(), entry, sections, imports, exports)
        }
        goblin::Object::Mach(_mach) => {
            // Re-parse as Mach to get MachO struct
            match goblin::mach::Mach::parse(&bytes) {
                Ok(goblin::mach::Mach::Binary(macho)) => {
                    let entry = format!("{:#x}", macho.entry);
                    let arch = match macho.header.magic {
                        0xfeedface | 0xcefaedfe => "x86",
                        0xfeedfacf | 0xcffaedfe => "x86_64",
                        _ => "unknown",
                    };
                    let mut nsections = 0usize;
                    for seg in macho.segments.iter() {
                        nsections += seg.into_iter().count();
                    }
                    let mut imports = 0usize;
                    let mut exports = 0usize;
                    for sym_result in macho.symbols() {
                        if let Ok((_name, nlist)) = sym_result {
                            if nlist.is_global() {
                                if nlist.is_undefined() { imports += 1; }
                                else { exports += 1; }
                            }
                        }
                    }
                    ("mach".to_string(), arch.to_string(), entry, nsections, imports, exports)
                }
                Ok(goblin::mach::Mach::Fat(_multi)) => {
                    ("mach".to_string(), "fat_binary".to_string(), "0x0".to_string(), 0, 0, 0)
                }
                Err(e) => return write_error(format!("Mach parse: {e}")),
            }
        }
        _ => return write_error("unknown format".to_string()),
    };

    log(&format!("exe.inspect format={} arch={} size={}", fmt, arch, file_size));
    write_output(&json!({
        "format": fmt, "architecture": arch, "entry_point": entry,
        "section_count": sections, "import_count": imports, "export_count": exports,
        "file_size": file_size,
    }))
}

// ─── exe.verify ───

#[export_name = "exe.verify"]
pub extern "C" fn exe_verify(input_ptr: i32, input_len: i32) -> i64 {
    let input: PathInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return write_error(format!("invalid input: {e}")),
    };
    let bytes = match read_file(&input.path) {
        Ok(b) => b,
        Err(e) => return write_error(e),
    };
    if bytes.is_empty() { return write_error("file is empty"); }

    let obj = match goblin::Object::parse(&bytes) {
        Ok(obj) => obj,
        Err(e) => {
            return write_output(&json!({
                "valid": false, "format": "unknown",
                "issue": format!("not a recognized executable format: {e}"),
            }));
        }
    };

    let (fmt, valid, issue) = match obj {
        goblin::Object::PE(pe) => {
            // goblin 0.9 already validated the optional header during parse.
            // We verify section tables are reachable.
            let sect_end = pe.header.coff_header.size_of_optional_header as usize + 24 + (pe.sections.len() * 40);
            if sect_end > bytes.len() {
                ("pe".to_string(), false, format!("truncated: section table extends beyond EOF (need {sect_end}, got {})", bytes.len()))
            } else if pe.sections.is_empty() {
                ("pe".to_string(), false, "no sections found".to_string())
            } else {
                ("pe".to_string(), true, String::new())
            }
        }
        goblin::Object::Elf(elf) => {
            if elf.header.e_phoff as usize > bytes.len() {
                ("elf".to_string(), false, format!("program header table beyond EOF: offset {} exceeds file size {}", elf.header.e_phoff, bytes.len()))
            } else if elf.header.e_shoff as usize > bytes.len() {
                ("elf".to_string(), false, format!("section header table beyond EOF: offset {} exceeds file size {}", elf.header.e_shoff, bytes.len()))
            } else if elf.header.e_ehsize as usize > bytes.len() {
                ("elf".to_string(), false, format!("ELF header exceeds file: e_ehsize {}", elf.header.e_ehsize))
            } else {
                ("elf".to_string(), true, String::new())
            }
        }
        goblin::Object::Mach(_mach) => {
            match goblin::mach::Mach::parse(&bytes) {
                Ok(goblin::mach::Mach::Binary(macho)) => {
                    if macho.header.sizeofcmds as usize > bytes.len() {
                        ("mach".to_string(), false, format!("load commands exceed file: sizeofcmds {}", macho.header.sizeofcmds))
                    } else {
                        ("mach".to_string(), true, String::new())
                    }
                }
                Ok(goblin::mach::Mach::Fat(_multi)) => {
                    ("mach".to_string(), true, "fat binary".to_string())
                }
                Err(e) => ("mach".to_string(), false, format!("Mach verify error: {e}")),
            }
        }
        _ => return write_error("unknown format"),
    };

    log(&format!("exe.verify format={} valid={}", fmt, valid));
    write_output(&json!({
        "valid": valid, "format": fmt, "issue": issue,
    }))
}
