# eqts implementation plan

## Goal

Build one annotated Rust API into the same TypeScript API over a user-selected transport: Node-API, Node through Koffi, Bun FFI, Deno FFI, or WebAssembly.

The project is worthwhile only while it preserves this distinction from existing bindgen projects:

```text
one Rust surface
→ user-selected transport
→ same TypeScript import
→ one Cargo command
```

## Status

The fixed-width scalar slice is complete. One annotated function builds and executes through Node-API, Node with Koffi, Bun FFI, Deno FFI, and Node-compatible Wasm on macOS arm64. Metadata is deterministic and versioned; the native ABI catches panics and returns status codes. Owned values, browser Wasm initialization, consolidated publishing, and the cross-platform matrix remain roadmap work.

## Public surface

The Rust package exposes `eqts::setup!()`, `#[eqts::export]`, `eqts::Record`, and `eqts::Enum`. The first release supports functions, numeric scalars, booleans, strings, bytes, records, enums, `Option`, `Result`, and vectors. Unsupported signatures fail during compilation rather than becoming pointers or TypeScript `any`.

Cargo features select available transports:

- `node-napi`, enabled by default
- `node-koffi`
- `bun`
- `deno`
- `wasm`

`cargo eqts build --target <target>` emits one selected artifact. `--target all` emits every enabled target. The root Node import uses Node-API; explicit package subpaths select Koffi, Bun, Deno, or Wasm.

## Architecture

The workspace contains three crates: public API `eqts`, proc macros `eqts-macros`, and `cargo-eqts`. Proc macros validate signatures, export backend wrappers, and register transport-neutral metadata. `cargo-eqts` reads that metadata and generates one canonical declaration model plus small runtime loaders.

Node-API uses napi-rs. Koffi, Bun, and Deno share a stable C ABI dynamic library. Wasm uses wasm-bindgen. Equilibrium integration follows the standalone release and consumes its existing C headers; Inauguration is not a dependency.

## Delivery order

1. Prove scalar function parity through Koffi, Bun, and Deno.
2. Add canonical declarations and owned value types.
3. Add Node-API as the default packaged target.
4. Add Wasm with separate runtime verification.
5. Add objects, async functions, callbacks, traits, streams, and iterators only after identical behavior exists on every transport.

## Acceptance

All enabled targets must expose the same generated TypeScript names and normalized results. Builds must fail clearly for disabled targets, unsupported Rust signatures, missing libraries, wrong architectures, invalid UTF-8, overflow, Rust errors, and caught panics. Published native packages must use prebuilt artifacts without install-time downloads or consumer compilers.

Required gates are Rust formatting, Clippy with every feature and target, workspace tests, doctests, documentation, and Bun-driven formatting, linting, type-checking, tests, and builds. Runtime claims require execution on each advertised OS, architecture, libc, and JavaScript runtime.
