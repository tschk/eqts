# eqts

One Rust API, imported from TypeScript through a selected native runtime.

Current implementation generates scalar adapters for Node with Koffi, Bun FFI, and Deno FFI. The example executes successfully in Bun and Deno on macOS arm64; Koffi execution and the wider platform matrix remain unverified.

```rust
eqts::setup!();

#[eqts::export]
pub fn add(a: u32, b: u32) -> u32 {
    a + b
}
```

```bash
cargo install --path cargo-eqts
cargo eqts build --target bun
```

See [PLAN.md](PLAN.md) for the complete transport roadmap and acceptance criteria.
