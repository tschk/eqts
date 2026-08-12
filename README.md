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

## Quick start

Add eqts to a Rust library that emits both a Rust library and a dynamic library:

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
eqts = "0.2"
```

Install the generator and build every adapter:

```bash
cargo install cargo-eqts
cargo eqts build --target all --release
```

The generated package exposes explicit `./node-napi`, `./node-koffi`, `./bun`, `./deno`, `./wasm`, and `./wasm-browser` entry points. Use a single target while developing, for example `cargo eqts build --target bun --release`.

## Rust API

- `#[eqts::export]` exports synchronous free functions.
- `#[eqts::async_export(T)]` exports cancellable asynchronous functions.
- `#[eqts::stream(T)]` and `#[eqts::iterator(T)]` export demand-driven async iterables.
- `#[eqts::callback(T)]` delivers Rust events to a JavaScript callback.
- `#[eqts::object(T)]` and `#[eqts::trait_export(T)]` create disposable stateful proxies.
- `#[eqts::methods]` exports public synchronous and immutable asynchronous methods.
- `eqts::Record` and `eqts::Enum` derive the shared TypeScript value model.

Each crate calls `eqts::setup!()` once. Unsupported signatures fail at compile time instead of degrading to raw pointers or `any`.

## Runtime targets

| Target | Transport | Initialization |
| --- | --- | --- |
| `node-napi` | napi-rs | synchronous |
| `node-koffi` | Koffi over the shared C ABI | synchronous |
| `bun` | `bun:ffi` over the shared C ABI | synchronous |
| `deno` | `Deno.dlopen` over the shared C ABI | synchronous; requires FFI permission |
| `wasm` | wasm-bindgen for Node-compatible runtimes | synchronous |
| `wasm-browser` | wasm-bindgen web target | `await initialize()` |

Consumer bridge crates expose the requested Cargo features: `node-napi`, `node-koffi`, `bun`, `deno`, and `wasm`. Node-API is the default. Native FFI targets share a panic-safe status-and-output-pointer ABI; Node-API uses napi-rs and Wasm uses wasm-bindgen.

Native owned values use a versioned JSON ABI with explicit buffer ownership. Rust `Result` errors become `EqtsError`; 64-bit integers become TypeScript `bigint`; bytes become `Uint8Array`. Generated TypeScript function names use camel case while Rust symbols remain snake case.

Reactive exports use web-standard TypeScript contracts. Async functions and async methods return cancellable promises through an optional `AbortSignal`; streams and iterators are demand-driven `AsyncIterable` handles with one outstanding `next()` call; callbacks are dispatched on the JavaScript thread with one-item backpressure. Stateful handles expose idempotent `dispose()` and `Symbol.dispose`, reject use after disposal, and register a finalizer as a safety net. `#[eqts::methods]` supplies typed sync and async methods for object and structural-trait proxies. Arbitrary Rust `dyn Trait` values and mutable async methods are intentionally rejected because they cannot satisfy the shared ownership contract.

Browser Wasm is available through `--target wasm-browser` and exposes an async `initialize()` function. `--target wasm` remains the synchronous Node-compatible build. `--target all` emits both plus a consolidated conditional-exports manifest.

## Compatibility

eqts 0.2 requires Rust 1.88 or newer. The generated declarations are verified with TypeScript 7. Node-API uses Node 20.17 or newer through the pinned napi-rs toolchain. Native packages are designed for prebuilt artifacts and do not require install-time downloads or a consumer Rust compiler.

## Project links

- [Crate documentation](https://docs.rs/eqts)
- [eqts on crates.io](https://crates.io/crates/eqts)
- [cargo-eqts on crates.io](https://crates.io/crates/cargo-eqts)
- [GitHub releases](https://github.com/tschk/eqts/releases)

See [PLAN.md](PLAN.md) for the complete transport roadmap and acceptance criteria.
