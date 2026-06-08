//! Tiny dev helper: read a `.wat` file, write the compiled `.wasm`.
//! Usage: `cargo run --release --example wat_compile -- <input.wat> <output.wasm>`

fn main() {
    let mut args = std::env::args().skip(1);
    let inp = args
        .next()
        .expect("usage: wat_compile <input.wat> <output.wasm>");
    let out = args
        .next()
        .expect("usage: wat_compile <input.wat> <output.wasm>");
    let wat_src = std::fs::read_to_string(&inp).expect("read wat");
    let bytes = wat::parse_str(&wat_src).expect("compile wat");
    std::fs::write(&out, &bytes).expect("write wasm");
    eprintln!("compiled {} bytes -> {}", bytes.len(), out);
}
