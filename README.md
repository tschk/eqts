# eqts

One Rust API, imported from TypeScript through a selected native runtime.

Current implementation generates adapters for Node-API, Node with Koffi, Bun FFI, Deno FFI, Node-compatible Wasm, and browser Wasm. The shared API supports fixed-width scalars, strings, bytes, vectors, options, results, named records, unit enums, async functions, callbacks, streams, iterators, and stateful object or structural-trait proxies.

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

Reactive exports use web-standard TypeScript contracts. Async functions and async methods return cancellable promises through an optional `AbortSignal`; streams and iterators are demand-driven `AsyncIterable` handles with one outstanding `next()` call; callbacks are dispatched on the JavaScript thread with one-item backpressure. Stateful handles expose idempotent `dispose()` and `Symbol.dispose`, reject use after disposal, and register a finalizer as a safety net. `#[eqts::methods]` supplies typed sync and async methods for object and structural-trait proxies. Arbitrary Rust `dyn Trait` values and mutable async methods are intentionally rejected because they cannot satisfy the shared ownership contract.

Browser Wasm is available through `--target wasm-browser` and exposes an async `initialize()` function. `--target wasm` remains the synchronous Node-compatible build. `--target all` emits both plus a consolidated conditional-exports manifest.

See [PLAN.md](PLAN.md) for the complete transport roadmap and acceptance criteria.
