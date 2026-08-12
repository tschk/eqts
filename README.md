# eqts

One Rust API, imported from TypeScript through a selected native runtime.

Current implementation generates adapters for Node-API, Node with Koffi, Bun FFI, Deno FFI, Node-compatible Wasm, and browser Wasm. The shared API supports fixed-width scalars, strings, bytes, vectors, options, results, named records, and unit enums.

```rust
eqts::setup!();

#[eqts::export]
pub fn add(a: u32, b: u32) -> u32 {
    a + b
}
```

```bash
cargo install cargo-eqts
cargo eqts build --target all --release
```

Consumer bridge crates expose the requested Cargo features: `node-napi`, `node-koffi`, `bun`, `deno`, and `wasm`. Node-API is the default. Native FFI targets share a panic-safe status-and-output-pointer ABI; Node-API uses napi-rs and Wasm uses wasm-bindgen.

Native owned values use a versioned JSON ABI with explicit buffer ownership. Rust `Result` errors become `EqtsError`; 64-bit integers become TypeScript `bigint`; bytes become `Uint8Array`. Generated TypeScript function names use camel case while Rust symbols remain snake case.

Browser Wasm is available through `--target wasm-browser` and exposes an async `initialize()` function. `--target wasm` remains the synchronous Node-compatible build. `--target all` emits both plus a consolidated conditional-exports manifest.

See [PLAN.md](PLAN.md) for the complete transport roadmap and acceptance criteria.
