//! Ordo - goblin binary parser MCP server.
//!
//! Wraps goblin 0.9 for PE, ELF, and Mach-O binary parsing.
//!
//! Tool ABI: each export takes (input_ptr: i32, input_len: i32)
//! and returns a packed i64 (high 32 = out_ptr, low 32 = out_len).

use serde_json::{json, Value};

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

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    let buf_ptr = alloc(8 * 1024 * 1024);
    if buf_ptr == 0 { return Err("allocation failed".to_string()); }
    let n = unsafe { host_fs_read(path.as_ptr(), path.len() as i32, buf_ptr as *mut u8) };
    if n <= 0 { return Err(format!("failed to read file '{}'", path)); }
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

fn error(message: impl Into<String>) -> i64 {
    write_output(&json!({ "error": message.into() }))
}

fn log_msg(msg: &str) {
    let bytes = msg.as_bytes();
    unsafe { host_log(bytes.as_ptr(), bytes.len() as i32); }
}

// ─── PE ───

fn describe_pe(bytes: &[u8]) -> Value {
    let pe = match goblin::pe::PE::parse(bytes) {
        Ok(pe) => pe,
        Err(e) => return json!({ "error": format!("PE parse: {}", e) }),
    };

    let sections: Vec<Value> = pe.sections.iter().map(|s| {
        let name = s.name().unwrap_or("?").to_string();
        json!({
            "name": name,
            "size": s.size_of_raw_data,
            "virtual_address": format!("{:#x}", s.virtual_address),
        })
    }).collect();

    let imports: Vec<String> = pe.imports.iter().map(|entry| {
        format!("{}:{}", entry.dll.trim_end_matches('\0'), entry.name.trim_end_matches('\0'))
    }).collect();

    let exports: Vec<String> = pe.exports.iter()
        .filter_map(|entry| entry.name.map(|n| n.to_string()))
        .collect();

    let arch = match pe.header.coff_header.machine {
        0x014c => "x86", 0x8664 => "x86_64", 0x01c4 => "arm", 0xaa64 => "arm64", _ => "unknown",
    };

    json!({
        "format": "pe", "architecture": arch,
        "entry_point": format!("{:#x}", pe.entry),
        "sections": sections, "imports": imports, "exports": exports,
    })
}

// ─── ELF ───

fn describe_elf(bytes: &[u8]) -> Value {
    let elf = match goblin::elf::Elf::parse(bytes) {
        Ok(elf) => elf,
        Err(e) => return json!({ "error": format!("ELF parse: {}", e) }),
    };

    let sections: Vec<Value> = elf.section_headers.iter().filter_map(|s| {
        let name = elf.shdr_strtab.get_at(s.sh_name).unwrap_or("?");
        if name.is_empty() { return None; }
        Some(json!({ "name": name.to_string(), "size": s.sh_size,
            "virtual_address": format!("{:#x}", s.sh_addr) }))
    }).collect();

    let imports: Vec<String> = elf.dynsyms.iter()
        .filter(|sym| sym.is_import())
        .map(|sym| elf.dynstrtab.get_at(sym.st_name).unwrap_or("?").to_string())
        .collect();

    let exports: Vec<String> = elf.dynsyms.iter()
        .filter(|sym| !sym.is_import() && sym.st_shndx != 0)
        .map(|sym| elf.dynstrtab.get_at(sym.st_name).unwrap_or("?").to_string())
        .collect();

    let arch = match elf.header.e_machine {
        0x03 => "x86", 0x3e => "x86_64", 0x28 => "arm", 0xb7 => "aarch64", _ => "unknown",
    };

    json!({ "format": "elf", "architecture": arch,
        "entry_point": format!("{:#x}", elf.header.e_entry),
        "sections": sections, "imports": imports, "exports": exports })
}

// ─── Mach-O ───

fn describe_mach(bytes: &[u8]) -> Value {
    let mach = match goblin::mach::Mach::parse(bytes) {
        Ok(goblin::mach::Mach::Binary(macho)) => macho,
        Ok(goblin::mach::Mach::Fat(multi)) => {
            // Take first arch from fat binary
            match multi.into_iter().next() {
                Some(Ok(goblin::mach::SingleArch::MachO(macho))) => macho,
                Some(Err(e)) => return json!({ "error": format!("Fat binary parse: {}", e) }),
                _ => return json!({ "error": "empty fat binary" }),
            }
        }
        Err(e) => return json!({ "error": format!("Mach-O parse: {}", e) }),
    };

    let arch = match mach.header.magic {
        0xfeedface | 0xcefaedfe => "x86",
        0xfeedfacf | 0xcffaedfe => "x86_64",
        _ => "unknown",
    };

    let mut sections = Vec::new();
    for seg in mach.segments.iter() {
        let seg_name = seg.name().unwrap_or("?").to_string();
        for s in seg.into_iter() {
            if let Ok((section, _data)) = s {
                let sname = section.name().unwrap_or("?");
                sections.push(json!({ "name": format!("{}/{}", seg_name, sname),
                    "size": section.size, "virtual_address": format!("{:#x}", section.addr) }));
            }
        }
    }

    let mut imports = Vec::new();
    let mut exports = Vec::new();
    for sym_result in mach.symbols() {
        if let Ok((full_name, nlist)) = sym_result {
            if nlist.is_global() {
                if nlist.is_undefined() { imports.push(full_name.to_string()); }
                else { exports.push(full_name.to_string()); }
            }
        }
    }

    json!({ "format": "mach", "architecture": arch,
        "entry_point": format!("{:#x}", mach.entry),
        "sections": sections, "imports": imports, "exports": exports })
}

// ─── Exports ───

#[derive(serde::Deserialize)]
struct PathInput {
    path: String,
}

#[export_name = "goblin.parse"]
pub extern "C" fn goblin_parse(input_ptr: i32, input_len: i32) -> i64 {
    let input: PathInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return error(format!("invalid input: {}", e)),
    };
    let obj_bytes = match read_file(&input.path) {
        Ok(b) => b,
        Err(e) => return error(e),
    };

    let output = match goblin::Object::parse(&obj_bytes) {
        Ok(obj) => match obj {
            goblin::Object::PE(_pe) => describe_pe(&obj_bytes),
            goblin::Object::Elf(_elf) => describe_elf(&obj_bytes),
            goblin::Object::Mach(_mach) => describe_mach(&obj_bytes),
            _ => json!({ "error": "unknown format" }),
        }
        Err(e) => json!({ "error": format!("goblin parse: {}", e) }),
    };

    log_msg(&format!("goblin.parse file={}", input.path));
    write_output(&output)
}

#[export_name = "goblin.symbols"]
pub extern "C" fn goblin_symbols(input_ptr: i32, input_len: i32) -> i64 {
    let input: PathInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return error(format!("invalid input: {}", e)),
    };
    let bytes = match read_file(&input.path) {
        Ok(b) => b,
        Err(e) => return error(e),
    };

    let output = match goblin::Object::parse(&bytes) {
        Ok(goblin::Object::PE(_pe)) => match goblin::pe::PE::parse(&bytes) {
            Ok(pe) => {
                let imports: Vec<String> = pe.imports.iter()
                    .map(|e| format!("{}:{}", e.dll.trim_end_matches('\0'), e.name.trim_end_matches('\0')))
                    .collect();
                let exports: Vec<String> = pe.exports.iter()
                    .filter_map(|e| e.name.map(|n| n.to_string()))
                    .collect();
                json!({ "imports": imports, "exports": exports })
            }
            Err(e) => json!({ "error": format!("PE symbols: {}", e) }),
        },
        Ok(goblin::Object::Elf(_elf)) => match goblin::elf::Elf::parse(&bytes) {
            Ok(elf) => {
                let imports: Vec<String> = elf.dynsyms.iter()
                    .filter(|s| s.is_import())
                    .map(|sym| elf.dynstrtab.get_at(sym.st_name).unwrap_or("?").to_string())
                    .collect();
                let exports: Vec<String> = elf.dynsyms.iter()
                    .filter(|s| !s.is_import() && s.st_shndx != 0)
                    .map(|sym| elf.dynstrtab.get_at(sym.st_name).unwrap_or("?").to_string())
                    .collect();
                json!({ "imports": imports, "exports": exports })
            }
            Err(e) => json!({ "error": format!("ELF symbols: {}", e) }),
        },
        Ok(goblin::Object::Mach(_mach)) => match goblin::mach::Mach::parse(&bytes) {
            Ok(goblin::mach::Mach::Binary(macho)) => {
                let mut imports = Vec::new();
                let mut exports = Vec::new();
                for sym_result in macho.symbols() {
                    if let Ok((full_name, nlist)) = sym_result {
                        if nlist.is_global() {
                            if nlist.is_undefined() { imports.push(full_name.to_string()); }
                            else { exports.push(full_name.to_string()); }
                        }
                    }
                }
                json!({ "imports": imports, "exports": exports })
            }
            Ok(goblin::mach::Mach::Fat(_multi)) => json!({ "imports": [], "exports": [], "info": "fat binary - use goblin.parse for per-arch details" }),
            Err(e) => json!({ "error": format!("Mach symbols: {}", e) }),
        },
        _ => json!({ "imports": [], "exports": [] }),
    };

    log_msg(&format!("goblin.symbols file={}", input.path));
    write_output(&output)
}

#[export_name = "goblin.libraries"]
pub extern "C" fn goblin_libraries(input_ptr: i32, input_len: i32) -> i64 {
    let input: PathInput = match serde_json::from_slice(&read_input(input_ptr, input_len)) {
        Ok(v) => v,
        Err(e) => return error(format!("invalid input: {}", e)),
    };
    let bytes = match read_file(&input.path) {
        Ok(b) => b,
        Err(e) => return error(e),
    };

    let libraries: Vec<String> = match goblin::Object::parse(&bytes) {
        Ok(goblin::Object::PE(_pe)) => goblin::pe::PE::parse(&bytes)
            .map(|pe| pe.libraries.iter().map(|&s| s.to_string()).collect())
            .unwrap_or_default(),
        Ok(goblin::Object::Elf(_elf)) => {
            let mut libs = std::collections::HashSet::new();
            if let Ok(elf) = goblin::elf::Elf::parse(&bytes) {
                for sym in &elf.dynsyms {
                    if sym.is_import() {
                        if let Some(name) = elf.dynstrtab.get_at(sym.st_name) {
                            if let Some(lib) = name.split('.').next() {
                                libs.insert(lib.to_string());
                            }
                        }
                    }
                }
            }
            libs.into_iter().collect()
        }
        Ok(goblin::Object::Mach(_mach)) => match goblin::mach::Mach::parse(&bytes) {
            Ok(goblin::mach::Mach::Binary(macho)) => macho.libs.iter().map(|&s| s.to_string()).collect(),
            Ok(goblin::mach::Mach::Fat(_multi)) => Vec::new(),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    };

    log_msg(&format!("goblin.libraries count={}", libraries.len()));
    write_output(&json!({ "libraries": libraries }))
}
