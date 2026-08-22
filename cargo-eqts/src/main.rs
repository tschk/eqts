use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use cargo_metadata::{CrateType, Metadata, MetadataCommand, Package};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use serde_json::json;

#[derive(Parser)]
#[command(name = "cargo", bin_name = "cargo")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Eqts {
        #[command(subcommand)]
        command: EqtsCommand,
    },
}

#[derive(Subcommand)]
enum EqtsCommand {
    Build {
        #[arg(long, value_enum)]
        target: Target,
        #[arg(long)]
        release: bool,
        #[arg(long, default_value = "dist")]
        out_dir: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Target {
    NodeKoffi,
    Bun,
    Deno,
    NodeNapi,
    Wasm,
    WasmBrowser,
    All,
}

#[derive(Clone, Debug)]
struct Function {
    module: String,
    name: String,
    symbol: String,
    abi: FunctionAbi,
    kind: ExportKind,
    parameters: Vec<Parameter>,
    result: Type,
    methods: Vec<Method>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionWire {
    module: String,
    name: String,
    symbol: String,
    #[serde(default)]
    abi: FunctionAbi,
    #[serde(default)]
    kind: Option<KindWire>,
    #[serde(default)]
    value: Option<Type>,
    #[serde(default)]
    item: Option<Type>,
    #[serde(default)]
    callback_parameter: Option<String>,
    parameters: Vec<Parameter>,
    result: Type,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum KindWire {
    Name(String),
    Descriptor(ExportKind),
}

impl<'de> Deserialize<'de> for Function {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = FunctionWire::deserialize(deserializer)?;
        if let Some(KindWire::Descriptor(kind)) = wire.kind {
            return Ok(Self {
                module: wire.module,
                name: wire.name,
                symbol: wire.symbol,
                abi: wire.abi,
                kind,
                parameters: wire.parameters,
                result: wire.result,
                methods: Vec::new(),
            });
        }
        let kind_name = match wire.kind {
            Some(KindWire::Name(kind)) => kind,
            None => "function".to_string(),
            Some(KindWire::Descriptor(_)) => unreachable!("descriptor returned above"),
        };
        let kind = match kind_name.as_str() {
            "function" => ExportKind::Function,
            "object" => ExportKind::Object {
                value: wire
                    .value
                    .ok_or_else(|| serde::de::Error::missing_field("value"))?,
            },
            "trait" => ExportKind::Trait {
                value: wire
                    .value
                    .ok_or_else(|| serde::de::Error::missing_field("value"))?,
            },
            "async" => ExportKind::Async {
                value: wire
                    .value
                    .ok_or_else(|| serde::de::Error::missing_field("value"))?,
            },
            "callback" => ExportKind::Callback {
                value: wire
                    .value
                    .ok_or_else(|| serde::de::Error::missing_field("value"))?,
                callback_parameter: wire
                    .callback_parameter
                    .ok_or_else(|| serde::de::Error::missing_field("callback_parameter"))?,
            },
            "stream" => ExportKind::Stream {
                item: wire
                    .item
                    .ok_or_else(|| serde::de::Error::missing_field("item"))?,
            },
            "iterator" => ExportKind::Iterator {
                item: wire
                    .item
                    .ok_or_else(|| serde::de::Error::missing_field("item"))?,
            },
            kind => {
                return Err(serde::de::Error::custom(format!(
                    "unknown export kind {kind}"
                )));
            }
        };
        Ok(Self {
            module: wire.module,
            name: wire.name,
            symbol: wire.symbol,
            abi: wire.abi,
            kind,
            parameters: wire.parameters,
            result: wire.result,
            methods: Vec::new(),
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum ExportKind {
    #[default]
    Function,
    Object {
        value: Type,
    },
    Trait {
        value: Type,
    },
    Async {
        value: Type,
    },
    Callback {
        value: Type,
        callback_parameter: String,
    },
    Stream {
        item: Type,
    },
    Iterator {
        item: Type,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetadataDocument {
    schema_version: u32,
    #[serde(default)]
    capabilities: std::collections::BTreeMap<String, bool>,
    functions: Vec<Function>,
    #[serde(default)]
    method_sets: Vec<MethodSet>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MethodSet {
    rust_type: String,
    methods: Vec<Method>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Method {
    name: String,
    parameters: Vec<Parameter>,
    result: Type,
    mutable: bool,
    asynchronous: bool,
}

#[repr(C)]
struct MetadataSlice {
    ptr: *const u8,
    len: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameter {
    name: String,
    #[serde(alias = "scalar")]
    ty: Type,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum FunctionAbi {
    #[default]
    Scalar,
    Json,
}

#[derive(Clone, Debug)]
enum Type {
    Scalar(Scalar),
    Owned(OwnedType),
}

impl<'de> Deserialize<'de> for Type {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let kind = match &value {
            serde_json::Value::String(kind) => kind.as_str(),
            serde_json::Value::Object(object) => object
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| serde::de::Error::custom("eqts type requires a string kind"))?,
            _ => {
                return Err(serde::de::Error::custom(
                    "eqts type must be a string or object",
                ));
            }
        };
        if kind == "scalar" {
            if value.as_object().is_some_and(|object| object.len() != 2) {
                return Err(serde::de::Error::custom(
                    "scalar type contains unsupported metadata fields",
                ));
            }
            let scalar = value
                .get("scalar")
                .cloned()
                .ok_or_else(|| serde::de::Error::custom("scalar type requires scalar"))?;
            return serde_json::from_value(scalar)
                .map(Self::Scalar)
                .map_err(serde::de::Error::custom);
        }
        if let Ok(scalar) = serde_json::from_value(serde_json::Value::String(kind.to_string())) {
            return Ok(Self::Scalar(scalar));
        }
        serde_json::from_value(value)
            .map(Self::Owned)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
enum OwnedType {
    String,
    Bytes,
    Vec { value: Box<Type> },
    Option { value: Box<Type> },
    Result { ok: Box<Type>, error: Box<Type> },
    Record { name: String, fields: Vec<Field> },
    Enum { name: String, variants: Vec<String> },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Field {
    name: String,
    ty: Type,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Scalar {
    Void,
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Eqts {
            command:
                EqtsCommand::Build {
                    target,
                    release,
                    out_dir,
                },
        } => build(target, release, &out_dir),
    }
}

fn build(target: Target, release: bool, out_dir: &Path) -> Result<()> {
    let build_all = target == Target::All;
    let targets = selected_targets(target);
    let metadata = MetadataCommand::new().exec()?;
    let package = library_package(&metadata)?;
    cargo_build(package, release)?;
    let library = library_path(&metadata, package, release)?;
    let functions = normalized_metadata(load_metadata(&library)?)?;
    let native_targets = targets
        .iter()
        .copied()
        .filter(|target| matches!(target, Target::NodeKoffi | Target::Bun | Target::Deno))
        .collect::<Vec<_>>();
    if !native_targets.is_empty() {
        for target in native_targets {
            generate_target(target, out_dir, &library, &functions)?;
        }
    }
    for target in targets {
        match target {
            Target::NodeNapi => build_node_napi(package, release, out_dir, &functions)?,
            Target::Wasm | Target::WasmBrowser => {
                build_wasm(package, release, out_dir, target, &functions)?;
            }
            Target::NodeKoffi | Target::Bun | Target::Deno | Target::All => {}
        }
    }
    if build_all {
        fs::create_dir_all(out_dir)?;
        fs::write(
            out_dir.join("package.json"),
            render_root_package_manifest()?,
        )?;
    }
    Ok(())
}

fn selected_targets(target: Target) -> Vec<Target> {
    match target {
        Target::NodeKoffi
        | Target::Bun
        | Target::Deno
        | Target::NodeNapi
        | Target::Wasm
        | Target::WasmBrowser => {
            vec![target]
        }
        Target::All => vec![
            Target::NodeKoffi,
            Target::Bun,
            Target::Deno,
            Target::NodeNapi,
            Target::Wasm,
            Target::WasmBrowser,
        ],
    }
}

fn package_directory(package: &Package) -> Result<PathBuf> {
    package
        .manifest_path
        .parent()
        .map(|path| path.to_path_buf().into_std_path_buf())
        .context("package manifest has no parent directory")
}

fn build_node_napi(
    package: &Package,
    release: bool,
    out_dir: &Path,
    functions: &[Function],
) -> Result<()> {
    let directory = package_directory(package)?;
    let output = directory.join(out_dir).join(target_name(Target::NodeNapi));
    if output.exists() {
        fs::remove_dir_all(&output)
            .with_context(|| format!("failed to clean {}", output.display()))?;
    }
    fs::create_dir_all(&output)?;
    let mut command = Command::new("bun");
    command.current_dir(&directory).args([
        "x",
        "--no-install",
        "napi",
        "build",
        "--platform",
        "--output-dir",
    ]);
    command.arg(&output).args([
        "--js",
        "index.js",
        "--dts",
        "index.d.ts",
        "--esm",
        "--features",
        "node-napi",
    ]);
    if release {
        command.arg("--release");
    }
    run_backend(command, "napi-rs")?;
    fs::rename(output.join("index.js"), output.join("bindings.js"))?;
    if output.join("index.d.ts").exists() {
        fs::rename(output.join("index.d.ts"), output.join("bindings.d.ts"))?;
    }
    fs::write(
        output.join("index.js"),
        render_bridge_loader(BridgeRuntime::NodeNapi, functions),
    )?;
    fs::write(output.join("index.d.ts"), render_declarations(functions))?;
    fs::write(
        output.join("package.json"),
        render_package_manifest(Target::NodeNapi)?,
    )?;
    Ok(())
}

fn build_wasm(
    package: &Package,
    release: bool,
    out_dir: &Path,
    target: Target,
    functions: &[Function],
) -> Result<()> {
    let directory = package_directory(package)?;
    let mut cargo = Command::new("cargo");
    cargo.current_dir(&directory).args([
        "build",
        "--locked",
        "--target",
        "wasm32-unknown-unknown",
        "--no-default-features",
        "--features",
        "wasm",
    ]);
    if release {
        cargo.arg("--release");
    }
    run_backend(cargo, "WebAssembly Cargo")?;
    let profile = if release { "release" } else { "debug" };
    let wasm = directory
        .join("target/wasm32-unknown-unknown")
        .join(profile)
        .join(format!("{}.wasm", package.name.replace('-', "_")));
    let output = directory.join(out_dir).join(target_name(target));
    if output.exists() {
        fs::remove_dir_all(&output)
            .with_context(|| format!("failed to clean {}", output.display()))?;
    }
    fs::create_dir_all(&output)?;
    let mut bindgen = Command::new("wasm-bindgen");
    let (name, bindgen_target) = if target == Target::WasmBrowser {
        ("bindings", "web")
    } else {
        ("bindings", "nodejs")
    };
    bindgen
        .current_dir(&directory)
        .arg(wasm)
        .args(["--out-dir"])
        .arg(&output)
        .args(["--out-name", name, "--target", bindgen_target]);
    run_backend(bindgen, "wasm-bindgen")?;
    if target == Target::Wasm {
        fs::rename(output.join("bindings.js"), output.join("bindings.cjs"))?;
    }
    fs::write(
        output.join("index.js"),
        render_bridge_loader(
            if target == Target::WasmBrowser {
                BridgeRuntime::WasmBrowser
            } else {
                BridgeRuntime::WasmNode
            },
            functions,
        ),
    )?;
    let mut declarations = render_declarations(functions);
    if target == Target::WasmBrowser {
        declarations.push_str(
            "export declare function initialize(input?: import(\"./bindings.js\").InitInput | Promise<import(\"./bindings.js\").InitInput>): Promise<import(\"./bindings.js\").InitOutput>;\n",
        );
    }
    fs::write(output.join("index.d.ts"), declarations)?;
    fs::write(
        output.join("package.json"),
        render_package_manifest(target)?,
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
enum BridgeRuntime {
    NodeNapi,
    WasmNode,
    WasmBrowser,
}

fn render_bridge_loader(runtime: BridgeRuntime, functions: &[Function]) -> String {
    let mut output = match runtime {
        BridgeRuntime::NodeNapi => {
            "import * as bindings from \"./bindings.js\";\n\n".to_string()
        }
        BridgeRuntime::WasmNode => {
            "import bindings from \"./bindings.cjs\";\n\n".to_string()
        }
        BridgeRuntime::WasmBrowser => "import init, * as bindings from \"./bindings.js\";\n\nlet ready;\n\nexport function initialize(input = new URL(\"./bindings_bg.wasm\", import.meta.url)) {\n  if (!ready) ready = Promise.resolve().then(() => init({ module_or_path: input }));\n  return ready;\n}\n\n".to_string(),
    };
    output.push_str("function __eqtsNormalize(value) {\n  if (value instanceof Map) return Object.fromEntries(Array.from(value, ([key, entry]) => [key, __eqtsNormalize(entry)]));\n  if (Array.isArray(value)) return value.map(__eqtsNormalize);\n  return value;\n}\n\n");
    if functions
        .iter()
        .any(|function| !matches!(function.kind, ExportKind::Function))
    {
        output.push_str(reactive_runtime());
    }
    if functions.iter().any(|function| {
        matches!(function.result, Type::Owned(OwnedType::Result { .. }))
            || !matches!(function.kind, ExportKind::Function)
    }) {
        output.push_str("export class EqtsError extends Error {\n  constructor(code, value) {\n    super(typeof value === \"string\" ? value : `eqts error: ${code}`);\n    this.name = \"EqtsError\";\n    this.code = code;\n    this.value = value;\n  }\n}\n\n");
    }
    for function in functions {
        if matches!(function.kind, ExportKind::Function) {
            render_bridge_function(&mut output, function);
        } else {
            render_reactive_function(&mut output, function);
        }
    }
    output
}

fn reactive_runtime() -> &'static str {
    "const __eqtsFinalizer = new FinalizationRegistry((handle) => bindings.eqtsHandleDispose(handle));\n\nfunction __eqtsHandle(handle, decode) {\n  let disposed = false;\n  let outstanding = false;\n  const resource = {\n    get disposed() { return disposed; },\n    dispose() {\n      if (disposed) return;\n      disposed = true;\n      __eqtsFinalizer.unregister(resource);\n      bindings.eqtsHandleDispose(handle);\n    },\n    async next() {\n      if (disposed) throw new EqtsError(\"USE_AFTER_DISPOSE\", \"reactive handle is disposed\");\n      if (outstanding) throw new EqtsError(\"CONCURRENT_NEXT\", \"only one next() call may be outstanding\");\n      outstanding = true;\n      try {\n        for (;;) {\n          const poll = bindings.eqtsReactivePoll(handle);\n          if (poll.status === 10) { await new Promise((resolve) => setTimeout(resolve, 0)); continue; }\n          if (poll.status === 11 || poll.status === 13) return { value: decode(__eqtsNormalize(poll.value)), done: false };\n          if (poll.status === 12) { resource.dispose(); return { value: undefined, done: true }; }\n          throw new EqtsError(\"REACTIVE_POLL\", poll);\n        }\n      } catch (error) { resource.dispose(); throw error; } finally { outstanding = false; }\n    },\n    async return() { bindings.eqtsReactiveCancel(handle); resource.dispose(); return { value: undefined, done: true }; },\n    [Symbol.asyncIterator]() { return resource; },\n    [Symbol.dispose]() { resource.dispose(); },\n  };\n  __eqtsFinalizer.register(resource, handle, resource);\n  return resource;\n}\n\nfunction __eqtsCallback(handle, callback, decode) {\n  let disposed = false;\n  const resource = { get disposed() { return disposed; }, dispose() { if (disposed) return; disposed = true; __eqtsFinalizer.unregister(resource); bindings.eqtsReactiveCancel(handle); bindings.eqtsHandleDispose(handle); }, [Symbol.dispose]() { resource.dispose(); } };\n  __eqtsFinalizer.register(resource, handle, resource);\n  void (async () => {\n    try {\n      while (!disposed) { const poll = bindings.eqtsReactivePoll(handle); if (poll.status === 10) { await new Promise((resolve) => setTimeout(resolve, 0)); continue; } if (poll.status === 13) { await callback(decode(__eqtsNormalize(poll.value))); continue; } if (poll.status === 12) { resource.dispose(); return; } throw new EqtsError(\"REACTIVE_POLL\", poll); }\n    } catch (error) { resource.dispose(); queueMicrotask(() => { throw error; }); }\n  })();\n  return resource;\n}\n\nasync function __eqtsAwait(handle, signal, decode) {\n  if (signal?.aborted) { bindings.eqtsReactiveCancel(handle); bindings.eqtsHandleDispose(handle); throw signal.reason ?? new DOMException(\"Aborted\", \"AbortError\"); }\n  const abort = () => bindings.eqtsReactiveCancel(handle);\n  signal?.addEventListener(\"abort\", abort, { once: true });\n  const iterator = __eqtsHandle(handle, decode);\n  try {\n    const result = await iterator.next();\n    if (signal?.aborted) throw signal.reason ?? new DOMException(\"Aborted\", \"AbortError\");\n    return result.value;\n  } finally { iterator.dispose(); signal?.removeEventListener(\"abort\", abort); }\n}\n\n"
}

fn render_reactive_function(output: &mut String, function: &Function) {
    let name = typescript_name(&function.name);
    let mut parameters = function
        .parameters
        .iter()
        .map(|parameter| typescript_name(&parameter.name))
        .collect::<Vec<_>>();
    let arguments = parameters.join(", ");
    let value = match &function.kind {
        ExportKind::Object { value }
        | ExportKind::Trait { value }
        | ExportKind::Async { value }
        | ExportKind::Callback { value, .. } => value,
        ExportKind::Stream { item } | ExportKind::Iterator { item } => item,
        ExportKind::Function => unreachable!("reactive renderer received function"),
    };
    let decode = format!("(value) => {}", wire_decode("value", value));
    if matches!(function.kind, ExportKind::Async { .. }) {
        parameters.push("options = {}".to_string());
    }
    if let ExportKind::Callback {
        callback_parameter, ..
    } = &function.kind
    {
        parameters.push(typescript_name(callback_parameter));
    }
    let callback = if let ExportKind::Callback {
        callback_parameter, ..
    } = &function.kind
    {
        Some(typescript_name(callback_parameter))
    } else {
        None
    };
    writeln!(
        output,
        "export function {name}({}) {{",
        parameters.join(", ")
    )
    .expect("writing to a string cannot fail");
    writeln!(output, "  const handle = bindings.{name}({arguments});")
        .expect("writing to a string cannot fail");
    if matches!(
        function.kind,
        ExportKind::Object { .. } | ExportKind::Trait { .. }
    ) {
        output.push_str("  const resource = __eqtsHandle(handle, __eqtsNormalize);\n");
        render_object_methods(output, function);
        output.push_str("  return resource;\n");
    } else if let Some(callback) = callback {
        writeln!(
            output,
            "  return __eqtsCallback(handle, {callback}, {decode});"
        )
        .expect("writing to a string cannot fail");
    } else if matches!(function.kind, ExportKind::Async { .. }) {
        writeln!(
            output,
            "  return __eqtsAwait(handle, options.signal, {decode});"
        )
        .expect("writing to a string cannot fail");
    } else {
        writeln!(output, "  return __eqtsHandle(handle, {decode});")
            .expect("writing to a string cannot fail");
    }
    output.push_str("}\n");
}

fn render_native_reactive_constructor(output: &mut String, function: &Function, runtime: Runtime) {
    let name = typescript_name(&function.name);
    let mut parameters = function
        .parameters
        .iter()
        .map(|parameter| typescript_name(&parameter.name))
        .collect::<Vec<_>>();
    let arguments = function
        .parameters
        .iter()
        .map(|parameter| wire_encode(&typescript_name(&parameter.name), &parameter.ty))
        .collect::<Vec<_>>()
        .join(", ");
    let value = match &function.kind {
        ExportKind::Object { value }
        | ExportKind::Trait { value }
        | ExportKind::Async { value }
        | ExportKind::Callback { value, .. } => value,
        ExportKind::Stream { item } | ExportKind::Iterator { item } => item,
        ExportKind::Function => unreachable!("reactive renderer received function"),
    };
    let decode = format!("(value) => {}", wire_decode("value", value));
    if matches!(function.kind, ExportKind::Async { .. }) {
        parameters.push("options = {}".to_string());
    }
    let callback = if let ExportKind::Callback {
        callback_parameter, ..
    } = &function.kind
    {
        parameters.push(typescript_name(callback_parameter));
        Some(typescript_name(callback_parameter))
    } else {
        None
    };
    writeln!(
        output,
        "export function {name}({}) {{",
        parameters.join(", ")
    )
    .expect("writing to a string cannot fail");
    let storage = output_storage(runtime, Scalar::U64);
    writeln!(
        output,
        "  const __eqtsInput = new TextEncoder().encode(JSON.stringify([{arguments}]));"
    )
    .expect("writing to a string cannot fail");
    writeln!(output, "  const __eqtsHandleOutput = {storage};")
        .expect("writing to a string cannot fail");
    let output_arg = if matches!(runtime, Runtime::Bun) {
        "ptr(__eqtsHandleOutput)"
    } else {
        "__eqtsHandleOutput"
    };
    let input_arg = if matches!(runtime, Runtime::Bun) {
        "ptr(__eqtsInput)"
    } else {
        "__eqtsInput"
    };
    let input_len = if matches!(runtime, Runtime::Koffi) {
        "__eqtsInput.byteLength"
    } else {
        "BigInt(__eqtsInput.byteLength)"
    };
    writeln!(
        output,
        "  const __eqtsStatus = {}({input_arg}, {input_len}, {output_arg});",
        runtime_call(runtime, function)
    )
    .expect("writing to a string cannot fail");
    writeln!(
        output,
        "  checkStatus(__eqtsStatus, \"{}\");",
        function.name
    )
    .expect("writing to a string cannot fail");
    output.push_str("  const handle = __eqtsHandleOutput[0];\n");
    if matches!(
        function.kind,
        ExportKind::Object { .. } | ExportKind::Trait { .. }
    ) {
        output.push_str("  const resource = __eqtsHandle(handle, __eqtsNormalize);\n");
        render_object_methods(output, function);
        output.push_str("  return resource;\n");
    } else if let Some(callback) = callback {
        writeln!(
            output,
            "  return __eqtsCallback(handle, {callback}, {decode});"
        )
        .expect("writing to a string cannot fail");
    } else if matches!(function.kind, ExportKind::Async { .. }) {
        writeln!(
            output,
            "  return __eqtsAwait(handle, options.signal, {decode});"
        )
        .expect("writing to a string cannot fail");
    } else {
        writeln!(output, "  return __eqtsHandle(handle, {decode});")
            .expect("writing to a string cannot fail");
    }
    output.push_str("}\n");
}

fn render_object_methods(output: &mut String, function: &Function) {
    for method in &function.methods {
        let name = typescript_name(&method.name);
        let parameters = method
            .parameters
            .iter()
            .map(|parameter| typescript_name(&parameter.name))
            .collect::<Vec<_>>();
        let arguments = method
            .parameters
            .iter()
            .map(|parameter| wire_encode(&typescript_name(&parameter.name), &parameter.ty))
            .collect::<Vec<_>>()
            .join(", ");
        let mut rendered_parameters = parameters.clone();
        if method.asynchronous {
            rendered_parameters.push("options = {}".to_string());
        }
        writeln!(
            output,
            "  resource.{name} = ({}) => {{",
            rendered_parameters.join(", ")
        )
        .expect("writing string");
        output.push_str("    if (resource.disposed) throw new EqtsError(\"USE_AFTER_DISPOSE\", \"reactive handle is disposed\");\n");
        let method_name = serde_json::to_string(&method.name).expect("method name JSON");
        if method.asynchronous {
            writeln!(output, "    const __eqtsAsyncHandle = bindings.eqtsHandleInvokeAsync(handle, {method_name}, [{arguments}]);").expect("writing string");
            writeln!(
                output,
                "    return __eqtsAwait(__eqtsAsyncHandle, options.signal, (value) => {});",
                wire_decode("value", &method.result)
            )
            .expect("writing string");
        } else {
            output.push_str("    let __eqtsResult;\n    try {\n");
            writeln!(output, "      __eqtsResult = __eqtsNormalize(bindings.eqtsHandleInvoke(handle, {method_name}, [{arguments}]));").expect("writing string");
            output.push_str("    } catch (__eqtsCaught) {\n      let __eqtsDetail = __eqtsCaught?.message ?? String(__eqtsCaught);\n      try { __eqtsDetail = JSON.parse(__eqtsDetail); } catch {}\n      throw new EqtsError(\"RUST_ERROR\", __eqtsDetail);\n    }\n");
        }
        if !method.asynchronous && matches!(method.result, Type::Scalar(Scalar::Void)) {
            output.push_str("    return;\n");
        } else if !method.asynchronous {
            writeln!(
                output,
                "    return {};",
                wire_decode("__eqtsResult", &method.result)
            )
            .expect("writing string");
        }
        output.push_str("  };\n");
    }
}

fn render_bridge_function(output: &mut String, function: &Function) {
    let name = typescript_name(&function.name);
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| typescript_name(&parameter.name))
        .collect::<Vec<_>>()
        .join(", ");
    let arguments = function
        .parameters
        .iter()
        .map(|parameter| {
            let name = typescript_name(&parameter.name);
            if function.abi == FunctionAbi::Json {
                wire_encode(&name, &parameter.ty)
            } else {
                name
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(output, "export function {name}({parameters}) {{")
        .expect("writing to a string cannot fail");
    if let Type::Owned(OwnedType::Result { ok, .. }) = &function.result {
        output.push_str("  try {\n");
        writeln!(
            output,
            "    const __eqtsResult = __eqtsNormalize(bindings.{name}({arguments}));"
        )
        .expect("writing to a string cannot fail");
        writeln!(output, "    return {};", wire_decode("__eqtsResult", ok))
            .expect("writing to a string cannot fail");
        output.push_str("  } catch (__eqtsCaught) {\n    let __eqtsDetail = __eqtsCaught?.message ?? String(__eqtsCaught);\n    try { __eqtsDetail = JSON.parse(__eqtsDetail); } catch {}\n    throw new EqtsError(\"RUST_ERROR\", __eqtsDetail);\n  }\n");
    } else if matches!(function.result, Type::Scalar(Scalar::Void)) {
        writeln!(output, "  bindings.{name}({arguments});")
            .expect("writing to a string cannot fail");
    } else {
        let expression = format!("bindings.{name}({arguments})");
        if function.abi == FunctionAbi::Json {
            writeln!(
                output,
                "  const __eqtsResult = __eqtsNormalize({expression});"
            )
            .expect("writing to a string cannot fail");
            writeln!(
                output,
                "  return {};",
                wire_decode("__eqtsResult", &function.result)
            )
            .expect("writing to a string cannot fail");
        } else {
            writeln!(output, "  return {expression};").expect("writing to a string cannot fail");
        }
    }
    output.push_str("}\n");
}

fn run_backend(mut command: Command, name: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to run {name}"))?;
    if !status.success() {
        bail!("{name} build failed");
    }
    Ok(())
}

fn generate_target(
    target: Target,
    out_dir: &Path,
    library: &Path,
    functions: &[Function],
) -> Result<()> {
    let target_dir = out_dir.join(target_name(target));
    if target_dir.exists() {
        fs::remove_dir_all(&target_dir)
            .with_context(|| format!("failed to clean {}", target_dir.display()))?;
    }
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("failed to create {}", target_dir.display()))?;
    let filename = library.file_name().context("library has no filename")?;
    fs::copy(library, target_dir.join(filename)).with_context(|| {
        format!(
            "failed to copy {} into {}",
            library.display(),
            target_dir.display()
        )
    })?;
    fs::write(
        target_dir.join("index.js"),
        render_loader(target, filename, functions)?,
    )?;
    fs::write(
        target_dir.join("index.d.ts"),
        render_declarations(functions),
    )?;
    fs::write(
        target_dir.join("package.json"),
        render_package_manifest(target)?,
    )?;
    println!("generated {}", target_dir.display());
    Ok(())
}

fn library_package(metadata: &Metadata) -> Result<&Package> {
    let package = metadata
        .root_package()
        .or_else(|| {
            metadata.packages.iter().find(|package| {
                package
                    .targets
                    .iter()
                    .any(|target| target.crate_types.contains(&CrateType::CDyLib))
            })
        })
        .context("run cargo eqts from a package containing a cdylib target")?;
    if package
        .targets
        .iter()
        .all(|target| !target.crate_types.contains(&CrateType::CDyLib))
    {
        bail!("package {} has no cdylib target", package.name);
    }
    Ok(package)
}

fn cargo_build(package: &Package, release: bool) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args([
        "build",
        "-p",
        package.name.as_str(),
        "--locked",
        "--no-default-features",
    ]);
    if release {
        command.arg("--release");
    }
    let status = command.status().context("failed to run cargo build")?;
    if !status.success() {
        bail!("cargo build failed");
    }
    Ok(())
}

fn library_path(metadata: &Metadata, package: &Package, release: bool) -> Result<PathBuf> {
    let library_name = package
        .targets
        .iter()
        .find(|target| target.crate_types.contains(&CrateType::CDyLib))
        .map(|target| target.name.replace('-', "_"))
        .context("cdylib target disappeared")?;
    let profile = if release { "release" } else { "debug" };
    let filename = if cfg!(target_os = "windows") {
        format!("{library_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{library_name}.dylib")
    } else {
        format!("lib{library_name}.so")
    };
    let path = metadata
        .target_directory
        .join(profile)
        .join(filename)
        .into_std_path_buf();
    path.canonicalize()
        .with_context(|| format!("built library was not found at {}", path.display()))
}

fn load_metadata(library_path: &Path) -> Result<Vec<Function>> {
    type MetadataFn = unsafe extern "C" fn() -> MetadataSlice;
    unsafe {
        let library = libloading::Library::new(library_path)
            .with_context(|| format!("failed to load {}", library_path.display()))?;
        let metadata: libloading::Symbol<MetadataFn> = library
            .get(b"eqts_metadata_v1")
            .context("eqts::setup!() metadata symbol is missing")?;
        let metadata = metadata();
        if metadata.ptr.is_null() {
            bail!("eqts metadata symbol returned a null pointer");
        }
        if metadata.len > 16 * 1024 * 1024 {
            bail!("eqts metadata exceeds the 16 MiB size limit");
        }
        let bytes = std::slice::from_raw_parts(metadata.ptr, metadata.len);
        parse_metadata(bytes)
    }
}

fn parse_metadata(bytes: &[u8]) -> Result<Vec<Function>> {
    let raw: serde_json::Value =
        serde_json::from_slice(bytes).context("eqts metadata is invalid JSON")?;
    if raw
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        == Some(3)
    {
        let reactive_capabilities = raw
            .get("capabilities")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|capabilities| {
                capabilities
                    .iter()
                    .any(|(name, value)| name != "owned_values" && value.as_bool() == Some(true))
            });
        let missing_kind = raw
            .get("functions")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|functions| {
                !functions.is_empty()
                    && functions
                        .iter()
                        .any(|function| function.get("kind").is_none())
            });
        if reactive_capabilities && missing_kind {
            bail!(
                "eqts metadata schema version 3 declares reactive capabilities without per-export descriptors; cannot distinguish ordinary u64 values from reactive handles or generate parity-safe loaders"
            );
        }
    }
    let mut document: MetadataDocument =
        serde_json::from_value(raw).context("eqts metadata is invalid JSON")?;
    if !matches!(document.schema_version, 1..=3) {
        bail!(
            "unsupported eqts metadata schema version {}; expected 1, 2, or 3",
            document.schema_version
        );
    }
    if document.schema_version < 3 {
        validate_capabilities(document.capabilities)?;
    }
    for function in &mut document.functions {
        let (ExportKind::Object { value } | ExportKind::Trait { value }) = &function.kind else {
            continue;
        };
        let Type::Owned(OwnedType::Record {
            name: type_name, ..
        }) = value
        else {
            bail!(
                "object or trait export {} must describe a named record type",
                function.name
            );
        };
        let method_set = document
            .method_sets
            .iter()
            .find(|set| set.rust_type.rsplit("::").next() == Some(type_name.as_str()))
            .with_context(|| {
                format!(
                    "object or trait export {} has no method set for {type_name}",
                    function.name
                )
            })?;
        function.methods.clone_from(&method_set.methods);
    }
    Ok(document.functions)
}

fn validate_capabilities(capabilities: std::collections::BTreeMap<String, bool>) -> Result<()> {
    let known = [
        "owned_values",
        "objects",
        "async_functions",
        "callbacks",
        "traits",
        "streams",
        "iterators",
    ];
    let unknown = capabilities
        .keys()
        .filter(|name| !known.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        bail!(
            "metadata declares unknown eqts capabilities: {}",
            unknown.join(", ")
        );
    }
    let unsupported = capabilities
        .into_iter()
        .filter_map(|(name, enabled)| (enabled && name != "owned_values").then_some(name))
        .collect::<Vec<_>>();
    if !unsupported.is_empty() {
        bail!(
            "metadata requires unsupported eqts capabilities: {}",
            unsupported.join(", ")
        );
    }
    Ok(())
}

fn normalized_metadata(mut functions: Vec<Function>) -> Result<Vec<Function>> {
    for function in &functions {
        if function.module.is_empty() {
            bail!("exported function {} has an empty module", function.name);
        }
        validate_identifier(&function.name)
            .with_context(|| format!("invalid exported function name {:?}", function.name))?;
        validate_symbol(&function.symbol)
            .with_context(|| format!("invalid ABI symbol for function {}", function.name))?;
        for parameter in &function.parameters {
            validate_identifier(&parameter.name).with_context(|| {
                format!(
                    "invalid parameter name {:?} in function {}",
                    parameter.name, function.name
                )
            })?;
            validate_type(&parameter.ty)?;
            if matches!(parameter.ty, Type::Scalar(Scalar::Void)) {
                bail!(
                    "parameter {} in function {} cannot have type void",
                    parameter.name,
                    function.name
                );
            }
        }
        validate_type(&function.result)?;
        match &function.kind {
            ExportKind::Function => {}
            ExportKind::Object { value } | ExportKind::Trait { value } => {
                validate_reactive_handle(function)?;
                validate_type(value)?;
                if function.methods.is_empty() {
                    bail!("object or trait export {} has no methods", function.name);
                }
                for method in &function.methods {
                    validate_identifier(&method.name)?;
                    for parameter in &method.parameters {
                        validate_identifier(&parameter.name)?;
                        validate_type(&parameter.ty)?;
                    }
                    validate_type(&method.result)?;
                    let _ = method.mutable;
                }
            }
            ExportKind::Async { value } => {
                validate_reactive_handle(function)?;
                validate_type(value)?;
            }
            ExportKind::Callback {
                value,
                callback_parameter,
            } => {
                validate_reactive_handle(function)?;
                validate_type(value)?;
                validate_identifier(callback_parameter)?;
                if function
                    .parameters
                    .iter()
                    .any(|parameter| parameter.name == *callback_parameter)
                {
                    bail!(
                        "callback parameter {callback_parameter} must be virtual and omitted from the raw factory ABI"
                    );
                }
            }
            ExportKind::Stream { item } | ExportKind::Iterator { item } => {
                validate_reactive_handle(function)?;
                validate_type(item)?;
            }
        }
        if function.abi == FunctionAbi::Scalar
            && (function
                .parameters
                .iter()
                .any(|parameter| !matches!(parameter.ty, Type::Scalar(_)))
                || !matches!(function.result, Type::Scalar(_)))
        {
            bail!(
                "function {} uses owned types with scalar ABI",
                function.name
            );
        }
    }
    functions.sort_by(|left, right| {
        (&left.module, &left.name, &left.symbol).cmp(&(&right.module, &right.name, &right.symbol))
    });
    for pair in functions.windows(2) {
        if pair[0].name == pair[1].name {
            bail!("duplicate exported function name {:?}", pair[0].name);
        }
    }
    Ok(functions)
}

fn validate_reactive_handle(function: &Function) -> Result<()> {
    if function.abi != FunctionAbi::Json || !matches!(function.result, Type::Scalar(Scalar::U64)) {
        bail!(
            "reactive export {} must use JSON constructor ABI and return a u64 handle",
            function.name
        );
    }
    Ok(())
}

fn validate_type(ty: &Type) -> Result<()> {
    match ty {
        Type::Scalar(_) | Type::Owned(OwnedType::String | OwnedType::Bytes) => Ok(()),
        Type::Owned(OwnedType::Vec { value } | OwnedType::Option { value }) => validate_type(value),
        Type::Owned(OwnedType::Result { ok, error }) => {
            validate_type(ok)?;
            validate_type(error)
        }
        Type::Owned(OwnedType::Record { name, fields }) => {
            validate_identifier(name)?;
            for field in fields {
                validate_identifier(&field.name)?;
                validate_type(&field.ty)?;
            }
            Ok(())
        }
        Type::Owned(OwnedType::Enum { name, variants }) => {
            validate_identifier(name)?;
            if variants.is_empty() {
                bail!("enum {name} must contain at least one variant");
            }
            Ok(())
        }
    }
}

fn validate_symbol(value: &str) -> Result<()> {
    let mut chars = value.chars();
    let first = chars.next().context("symbol cannot be empty")?;
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        bail!("symbol must be an ASCII C identifier");
    }
    Ok(())
}

fn validate_identifier(value: &str) -> Result<()> {
    let mut chars = value.chars();
    let first = chars.next().context("identifier cannot be empty")?;
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic())
        || !chars.all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
    {
        bail!("identifier must be an ASCII JavaScript identifier");
    }
    if matches!(
        value,
        "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "let"
            | "new"
            | "null"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    ) {
        bail!("identifier is a reserved JavaScript word");
    }
    Ok(())
}

fn target_name(target: Target) -> &'static str {
    match target {
        Target::NodeKoffi => "node-koffi",
        Target::Bun => "bun",
        Target::Deno => "deno",
        Target::NodeNapi => "node-napi",
        Target::Wasm => "wasm",
        Target::WasmBrowser => "wasm-browser",
        Target::All => "all",
    }
}

fn typescript_name(name: &str) -> String {
    let mut parts = name.split('_');
    let mut output = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        let mut characters = part.chars();
        if let Some(first) = characters.next() {
            output.extend(first.to_uppercase());
            output.extend(characters);
        }
    }
    output
}

#[expect(clippy::too_many_lines)]
fn render_declarations(functions: &[Function]) -> String {
    let mut output = String::new();
    let mut definitions = std::collections::BTreeMap::new();
    for function in functions {
        for parameter in &function.parameters {
            collect_definitions(&parameter.ty, &mut definitions);
        }
        collect_definitions(&function.result, &mut definitions);
        match &function.kind {
            ExportKind::Object { value }
            | ExportKind::Trait { value }
            | ExportKind::Async { value }
            | ExportKind::Callback { value, .. } => collect_definitions(value, &mut definitions),
            ExportKind::Stream { item } | ExportKind::Iterator { item } => {
                collect_definitions(item, &mut definitions);
            }
            ExportKind::Function => {}
        }
    }
    for definition in definitions.values() {
        output.push_str(definition);
        output.push('\n');
    }
    if functions.iter().any(|function| {
        function.abi == FunctionAbi::Json || !matches!(function.kind, ExportKind::Function)
    }) {
        output.push_str("export declare class EqtsError extends Error {\n  constructor(code: string, value: unknown);\n  readonly code: string;\n  readonly value: unknown;\n}\n\n");
    }
    if functions
        .iter()
        .any(|function| !matches!(function.kind, ExportKind::Function))
    {
        output.push_str("export interface EqtsHandle<T> extends AsyncIterable<T>, AsyncIterator<T>, Disposable {\n  readonly disposed: boolean;\n  dispose(): void;\n}\n\nexport interface EqtsCallbackHandle extends Disposable {\n  readonly disposed: boolean;\n  dispose(): void;\n}\n\n");
    }
    for function in functions {
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| format!("{}: {}", parameter.name, ts_type(&parameter.ty)))
            .collect::<Vec<_>>()
            .join(", ");
        let parameters = if let ExportKind::Callback {
            value,
            callback_parameter,
        } = &function.kind
        {
            let callback = format!(
                "{}: (value: {}) => void | Promise<void>",
                typescript_name(callback_parameter),
                ts_type(value)
            );
            if parameters.is_empty() {
                callback
            } else {
                format!("{parameters}, {callback}")
            }
        } else {
            parameters
        };
        let return_type = match &function.kind {
            ExportKind::Function => ts_return_type(&function.result),
            ExportKind::Async { value } => format!("Promise<{}>", ts_type(value)),
            ExportKind::Object { value } | ExportKind::Trait { value } => {
                let methods = function
                    .methods
                    .iter()
                    .map(|method| {
                        let mut parameters = method
                            .parameters
                            .iter()
                            .map(|parameter| {
                                format!(
                                    "{}: {}",
                                    typescript_name(&parameter.name),
                                    ts_type(&parameter.ty)
                                )
                            })
                            .collect::<Vec<_>>();
                        if method.asynchronous {
                            parameters.push("options?: { signal?: AbortSignal }".to_string());
                        }
                        let result = if method.asynchronous {
                            format!("Promise<{}>", ts_type(&method.result))
                        } else {
                            ts_return_type(&method.result)
                        };
                        format!(
                            "{}({}): {result}",
                            typescript_name(&method.name),
                            parameters.join(", ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("EqtsHandle<{}> & {{ {methods} }}", ts_type(value))
            }
            ExportKind::Callback { .. } => "EqtsCallbackHandle".to_string(),
            ExportKind::Stream { item } | ExportKind::Iterator { item } => {
                format!("EqtsHandle<{}>", ts_type(item))
            }
        };
        let parameters = if matches!(function.kind, ExportKind::Async { .. }) {
            if parameters.is_empty() {
                "options?: { signal?: AbortSignal }".to_string()
            } else {
                format!("{parameters}, options?: {{ signal?: AbortSignal }}")
            }
        } else {
            parameters
        };
        writeln!(
            output,
            "export declare function {}({parameters}): {return_type};",
            typescript_name(&function.name),
        )
        .expect("writing to a string cannot fail");
    }
    output
}

fn collect_definitions(ty: &Type, definitions: &mut std::collections::BTreeMap<String, String>) {
    match ty {
        Type::Owned(OwnedType::Vec { value } | OwnedType::Option { value }) => {
            collect_definitions(value, definitions);
        }
        Type::Owned(OwnedType::Result { ok, error }) => {
            collect_definitions(ok, definitions);
            collect_definitions(error, definitions);
        }
        Type::Owned(OwnedType::Record { name, fields }) => {
            for field in fields {
                collect_definitions(&field.ty, definitions);
            }
            let body = fields
                .iter()
                .map(|field| format!("  {}: {};", field.name, ts_type(&field.ty)))
                .collect::<Vec<_>>()
                .join("\n");
            definitions.insert(
                name.clone(),
                format!("export interface {name} {{\n{body}\n}}\n"),
            );
        }
        Type::Owned(OwnedType::Enum { name, variants }) => {
            let variants = variants
                .iter()
                .map(|variant| {
                    serde_json::to_string(variant).expect("string serialization cannot fail")
                })
                .collect::<Vec<_>>()
                .join(" | ");
            definitions.insert(name.clone(), format!("export type {name} = {variants};\n"));
        }
        Type::Scalar(_) | Type::Owned(OwnedType::String | OwnedType::Bytes) => {}
    }
}

fn render_loader(
    target: Target,
    filename: &std::ffi::OsStr,
    functions: &[Function],
) -> Result<String> {
    let filename = filename
        .to_str()
        .context("library filename is not valid UTF-8")?;
    let path = format!("./{filename}");
    match target {
        Target::NodeKoffi => Ok(render_koffi(&path, functions)),
        Target::Bun => Ok(render_bun(&path, functions)),
        Target::Deno => Ok(render_deno(&path, functions)),
        Target::NodeNapi | Target::Wasm | Target::WasmBrowser | Target::All => {
            bail!("target {} has no loader renderer", target_name(target))
        }
    }
}

fn render_koffi(path: &str, functions: &[Function]) -> String {
    let reactive = functions
        .iter()
        .any(|function| !matches!(function.kind, ExportKind::Function));
    let owned_buffer = if functions
        .iter()
        .any(|function| function.abi == FunctionAbi::Json)
        || reactive
    {
        "const OwnedBuffer = koffi.struct({ ptr: \"void *\", len: \"size_t\", capacity: \"size_t\" });\nconst __eqtsBufferFree = library.func(\"eqts_buffer_free_v1\", \"void\", [\"void *\", \"size_t\", \"size_t\"]);\n"
    } else {
        ""
    };
    let mut output = format!(
        "import koffi from \"koffi\";\nimport {{ fileURLToPath }} from \"node:url\";\n\nconst library = koffi.load(fileURLToPath(new URL(\"{path}\", import.meta.url)));\n{owned_buffer}\n{}\n",
        js_helpers()
    );
    if reactive {
        output.push_str("const __eqtsInvokeAsync = library.func(\"eqts_handle_invoke_async_v1\", \"int32_t\", [\"uint64_t\", \"uint8_t *\", \"size_t\", koffi.out(koffi.pointer(\"uint64_t\"))]);\n");
        output.push_str("const __eqtsDispose = library.func(\"eqts_handle_dispose_v1\", \"int32_t\", [\"uint64_t\"]);\nconst __eqtsCancel = library.func(\"eqts_reactive_cancel_v1\", \"int32_t\", [\"uint64_t\"]);\nconst __eqtsPoll = library.func(\"eqts_reactive_poll_v1\", \"int32_t\", [\"uint64_t\", koffi.out(koffi.pointer(OwnedBuffer))]);\nconst __eqtsInvoke = library.func(\"eqts_handle_invoke_v1\", \"int32_t\", [\"uint64_t\", \"uint8_t *\", \"size_t\", koffi.out(koffi.pointer(OwnedBuffer))]);\nconst bindings = {\n  eqtsHandleDispose(handle) { __eqtsDispose(handle); },\n  eqtsHandleInvoke(handle, method, __eqtsArguments) { const input = new TextEncoder().encode(JSON.stringify({ method, arguments: __eqtsArguments })); const output = {}; const status = __eqtsInvoke(handle, input, input.byteLength, output); let text = \"\"; try { if (output.ptr) text = koffi.decode(output.ptr, \"char\", Number(output.len)); } finally { if (output.ptr) __eqtsBufferFree(output.ptr, output.len, output.capacity); } checkStatus(status, method, text); return JSON.parse(text); },\n  eqtsReactiveCancel(handle) { const status = __eqtsCancel(handle); if (status === 14) throw new EqtsError(\"UNKNOWN_HANDLE\", handle); },\n  eqtsReactivePoll(handle) { const output = {}; const status = __eqtsPoll(handle, output); let value = null; try { if (output.ptr) value = JSON.parse(koffi.decode(output.ptr, \"char\", Number(output.len))); } finally { if (output.ptr) __eqtsBufferFree(output.ptr, output.len, output.capacity); } if (status === 14) throw new EqtsError(\"UNKNOWN_HANDLE\", handle); return { status, value }; },\n};\n\n");
        output.push_str("bindings.eqtsHandleInvokeAsync = (handle, method, __eqtsArguments) => { const input = new TextEncoder().encode(JSON.stringify({ method, arguments: __eqtsArguments })); const output = [null]; const status = __eqtsInvokeAsync(handle, input, input.byteLength, output); checkStatus(status, method); return output[0]; };\n\n");
        output.push_str(reactive_runtime());
    }
    for function in functions {
        if !matches!(function.kind, ExportKind::Function) {
            writeln!(output, "const __eqts_{} = library.func(\"{}\", \"int32_t\", [\"uint8_t *\", \"size_t\", koffi.out(koffi.pointer(\"uint64_t\"))]);", function.name, function.symbol).expect("writing to string cannot fail");
            render_native_reactive_constructor(&mut output, function, Runtime::Koffi);
            continue;
        } else if function.abi == FunctionAbi::Json {
            writeln!(output, "const __eqts_{} = library.func(\"{}\", \"int32_t\", [\"uint8_t *\", \"size_t\", koffi.out(koffi.pointer(OwnedBuffer))]);", function.name, function.symbol).expect("writing to a string cannot fail");
            render_json_wrapper(&mut output, function, Runtime::Koffi);
            continue;
        }
        let mut abi_parameters = function
            .parameters
            .iter()
            .map(|parameter| format!("\"{}\"", c_type(scalar_type(&parameter.ty))))
            .collect::<Vec<_>>();
        if !matches!(function.result, Type::Scalar(Scalar::Void)) {
            abi_parameters.push(format!(
                "koffi.out(koffi.pointer(\"{}\"))",
                c_type(scalar_type(&function.result))
            ));
        }
        writeln!(
            output,
            "const __eqts_{} = library.func(\"{}\", \"int32_t\", [{}]);",
            function.name,
            function.symbol,
            abi_parameters.join(", ")
        )
        .expect("writing to a string cannot fail");
        render_wrapper(&mut output, function, Runtime::Koffi);
    }
    output
}

fn render_bun(path: &str, functions: &[Function]) -> String {
    let reactive = functions
        .iter()
        .any(|function| !matches!(function.kind, ExportKind::Function));
    let mut output = "import { dlopen, FFIType, ptr, toArrayBuffer } from \"bun:ffi\";\nimport { fileURLToPath } from \"node:url\";\n\n".to_string();
    writeln!(
        output,
        "const {{ symbols }} = dlopen(fileURLToPath(new URL(\"{path}\", import.meta.url)), {{"
    )
    .expect("writing to a string cannot fail");
    for function in functions {
        if !matches!(function.kind, ExportKind::Function) {
            writeln!(
                output,
                "  {}: {{ args: [FFIType.ptr, FFIType.u64, FFIType.ptr], returns: FFIType.i32 }},",
                function.symbol
            )
            .expect("writing to string cannot fail");
            continue;
        }
        if function.abi == FunctionAbi::Json {
            writeln!(
                output,
                "  {}: {{ args: [FFIType.ptr, FFIType.u64, FFIType.ptr], returns: FFIType.i32 }},",
                function.symbol
            )
            .expect("writing to a string cannot fail");
            continue;
        }
        let mut args = function
            .parameters
            .iter()
            .map(|parameter| bun_type(scalar_type(&parameter.ty)))
            .collect::<Vec<_>>();
        if !matches!(function.result, Type::Scalar(Scalar::Void)) {
            args.push("FFIType.ptr");
        }
        writeln!(
            output,
            "  {}: {{ args: [{}], returns: FFIType.i32 }},",
            function.symbol,
            args.join(", ")
        )
        .expect("writing to a string cannot fail");
    }
    if functions
        .iter()
        .any(|function| function.abi == FunctionAbi::Json)
        || reactive
    {
        output.push_str("  eqts_buffer_free_v1: { args: [FFIType.u64, FFIType.u64, FFIType.u64], returns: FFIType.void },\n");
    }
    if reactive {
        output.push_str("  eqts_handle_dispose_v1: { args: [FFIType.u64], returns: FFIType.i32 },\n  eqts_handle_invoke_v1: { args: [FFIType.u64, FFIType.ptr, FFIType.u64, FFIType.ptr], returns: FFIType.i32 },\n  eqts_handle_invoke_async_v1: { args: [FFIType.u64, FFIType.ptr, FFIType.u64, FFIType.ptr], returns: FFIType.i32 },\n  eqts_reactive_cancel_v1: { args: [FFIType.u64], returns: FFIType.i32 },\n  eqts_reactive_poll_v1: { args: [FFIType.u64, FFIType.ptr], returns: FFIType.i32 },\n");
    }
    output.push_str("});\n\n");
    output.push_str(js_helpers());
    output.push('\n');
    if reactive {
        output.push_str("const bindings = {\n  eqtsHandleDispose(handle) { symbols.eqts_handle_dispose_v1(handle); },\n  eqtsReactiveCancel(handle) { const status = symbols.eqts_reactive_cancel_v1(handle); if (status === 14) throw new EqtsError(\"UNKNOWN_HANDLE\", handle); },\n  eqtsReactivePoll(handle) { const output = new BigUint64Array(3); const status = symbols.eqts_reactive_poll_v1(handle, ptr(output)); let value = null; try { if (output[0] !== 0n) value = JSON.parse(new TextDecoder().decode(new Uint8Array(toArrayBuffer(Number(output[0]), 0, Number(output[1]))).slice())); } finally { if (output[0] !== 0n) symbols.eqts_buffer_free_v1(output[0], output[1], output[2]); } if (status === 14) throw new EqtsError(\"UNKNOWN_HANDLE\", handle); return { status, value }; },\n};\n\n");
        output.push_str("bindings.eqtsHandleInvoke = (handle, method, __eqtsArguments) => { const input = new TextEncoder().encode(JSON.stringify({ method, arguments: __eqtsArguments })); const output = new BigUint64Array(3); const status = symbols.eqts_handle_invoke_v1(handle, ptr(input), BigInt(input.byteLength), ptr(output)); let text = \"\"; try { if (output[0] !== 0n) text = new TextDecoder().decode(new Uint8Array(toArrayBuffer(Number(output[0]), 0, Number(output[1]))).slice()); } finally { if (output[0] !== 0n) symbols.eqts_buffer_free_v1(output[0], output[1], output[2]); } checkStatus(status, method, text); return JSON.parse(text); };\n\n");
        output.push_str("bindings.eqtsHandleInvokeAsync = (handle, method, __eqtsArguments) => { const input = new TextEncoder().encode(JSON.stringify({ method, arguments: __eqtsArguments })); const output = new BigUint64Array(1); const status = symbols.eqts_handle_invoke_async_v1(handle, ptr(input), BigInt(input.byteLength), ptr(output)); checkStatus(status, method); return output[0]; };\n\n");
        output.push_str(reactive_runtime());
    }
    for function in functions {
        if !matches!(function.kind, ExportKind::Function) {
            render_native_reactive_constructor(&mut output, function, Runtime::Bun);
        } else if function.abi == FunctionAbi::Json {
            render_json_wrapper(&mut output, function, Runtime::Bun);
        } else {
            render_wrapper(&mut output, function, Runtime::Bun);
        }
    }
    output
}

fn render_deno(path: &str, functions: &[Function]) -> String {
    let reactive = functions
        .iter()
        .any(|function| !matches!(function.kind, ExportKind::Function));
    let mut output =
        format!("const {{ symbols }} = Deno.dlopen(new URL(\"{path}\", import.meta.url), {{\n");
    for function in functions {
        if !matches!(function.kind, ExportKind::Function) {
            writeln!(
                output,
                "  {}: {{ parameters: [\"buffer\", \"usize\", \"buffer\"], result: \"i32\" }},",
                function.symbol
            )
            .expect("writing to string cannot fail");
            continue;
        }
        if function.abi == FunctionAbi::Json {
            writeln!(
                output,
                "  {}: {{ parameters: [\"buffer\", \"usize\", \"buffer\"], result: \"i32\" }},",
                function.symbol
            )
            .expect("writing to a string cannot fail");
            continue;
        }
        let mut parameters = function
            .parameters
            .iter()
            .map(|parameter| format!("\"{}\"", deno_type(scalar_type(&parameter.ty))))
            .collect::<Vec<_>>();
        if !matches!(function.result, Type::Scalar(Scalar::Void)) {
            parameters.push("\"buffer\"".to_string());
        }
        writeln!(
            output,
            "  {}: {{ parameters: [{}], result: \"i32\" }},",
            function.symbol,
            parameters.join(", ")
        )
        .expect("writing to a string cannot fail");
    }
    if functions
        .iter()
        .any(|function| function.abi == FunctionAbi::Json)
        || reactive
    {
        output.push_str("  eqts_buffer_free_v1: { parameters: [\"pointer\", \"usize\", \"usize\"], result: \"void\" },\n");
    }
    if reactive {
        output.push_str("  eqts_handle_dispose_v1: { parameters: [\"u64\"], result: \"i32\" },\n  eqts_handle_invoke_v1: { parameters: [\"u64\", \"buffer\", \"usize\", \"buffer\"], result: \"i32\" },\n  eqts_handle_invoke_async_v1: { parameters: [\"u64\", \"buffer\", \"usize\", \"buffer\"], result: \"i32\" },\n  eqts_reactive_cancel_v1: { parameters: [\"u64\"], result: \"i32\" },\n  eqts_reactive_poll_v1: { parameters: [\"u64\", \"buffer\"], result: \"i32\" },\n");
    }
    output.push_str("});\n\n");
    output.push_str(js_helpers());
    output.push('\n');
    if reactive {
        output.push_str("const bindings = {\n  eqtsHandleDispose(handle) { symbols.eqts_handle_dispose_v1(handle); },\n  eqtsReactiveCancel(handle) { const status = symbols.eqts_reactive_cancel_v1(handle); if (status === 14) throw new EqtsError(\"UNKNOWN_HANDLE\", handle); },\n  eqtsReactivePoll(handle) { const output = new BigUint64Array(3); const status = symbols.eqts_reactive_poll_v1(handle, output); const pointer = output[0] === 0n ? null : Deno.UnsafePointer.create(output[0]); let value = null; try { if (pointer) value = JSON.parse(new TextDecoder().decode(new Uint8Array(new Deno.UnsafePointerView(pointer).getArrayBuffer(Number(output[1]))).slice())); } finally { if (pointer) symbols.eqts_buffer_free_v1(pointer, output[1], output[2]); } if (status === 14) throw new EqtsError(\"UNKNOWN_HANDLE\", handle); return { status, value }; },\n};\n\n");
        output.push_str("bindings.eqtsHandleInvoke = (handle, method, __eqtsArguments) => { const input = new TextEncoder().encode(JSON.stringify({ method, arguments: __eqtsArguments })); const output = new BigUint64Array(3); const status = symbols.eqts_handle_invoke_v1(handle, input, BigInt(input.byteLength), output); const pointer = output[0] === 0n ? null : Deno.UnsafePointer.create(output[0]); let text = \"\"; try { if (pointer) text = new TextDecoder().decode(new Uint8Array(new Deno.UnsafePointerView(pointer).getArrayBuffer(Number(output[1]))).slice()); } finally { if (pointer) symbols.eqts_buffer_free_v1(pointer, output[1], output[2]); } checkStatus(status, method, text); return JSON.parse(text); };\n\n");
        output.push_str("bindings.eqtsHandleInvokeAsync = (handle, method, __eqtsArguments) => { const input = new TextEncoder().encode(JSON.stringify({ method, arguments: __eqtsArguments })); const output = new BigUint64Array(1); const status = symbols.eqts_handle_invoke_async_v1(handle, input, BigInt(input.byteLength), output); checkStatus(status, method); return output[0]; };\n\n");
        output.push_str(reactive_runtime());
    }
    for function in functions {
        if !matches!(function.kind, ExportKind::Function) {
            render_native_reactive_constructor(&mut output, function, Runtime::Deno);
        } else if function.abi == FunctionAbi::Json {
            render_json_wrapper(&mut output, function, Runtime::Deno);
        } else {
            render_wrapper(&mut output, function, Runtime::Deno);
        }
    }
    output
}

#[derive(Clone, Copy)]
enum Runtime {
    Koffi,
    Bun,
    Deno,
}

fn js_helpers() -> &'static str {
    "export class EqtsError extends Error {\n  constructor(code, value) {\n    super(typeof value === \"string\" ? value : `eqts error: ${code}`);\n    this.name = \"EqtsError\";\n    this.code = code;\n    this.value = value;\n  }\n}\n\nfunction __eqtsNormalize(value) {\n  if (value instanceof Map) return Object.fromEntries(Array.from(value, ([key, entry]) => [key, __eqtsNormalize(entry)]));\n  if (Array.isArray(value)) return value.map(__eqtsNormalize);\n  return value;\n}\n\nfunction checkStatus(status, name, detail) {\n  if (status === 0) return;\n  const code = { 1: \"RUST_PANIC\", 2: \"NULL_OUTPUT\", 3: \"INVALID_INPUT\", 4: \"ENCODE_FAILURE\" }[status] ?? \"ABI_ERROR\";\n  throw new EqtsError(code, detail || `eqts call ${name} failed with ABI status ${status}`);\n}\n"
}

fn render_json_wrapper(output: &mut String, function: &Function, runtime: Runtime) {
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let inputs = function
        .parameters
        .iter()
        .map(|parameter| wire_encode(&parameter.name, &parameter.ty))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        "export function {}({parameters}) {{",
        typescript_name(&function.name)
    )
    .expect("writing to a string cannot fail");
    writeln!(
        output,
        "  const __eqtsInput = new TextEncoder().encode(JSON.stringify([{inputs}]));"
    )
    .expect("writing to a string cannot fail");
    match runtime {
        Runtime::Koffi => {
            output.push_str("  const __eqtsOutput = {};\n");
            writeln!(
                output,
                "  const __eqtsStatus = __eqts_{}(__eqtsInput, __eqtsInput.byteLength, __eqtsOutput);",
                function.name
            )
            .expect("writing to a string cannot fail");
            output.push_str("  let __eqtsText = \"\";\n  try {\n    if (__eqtsOutput.ptr) __eqtsText = koffi.decode(__eqtsOutput.ptr, \"char\", Number(__eqtsOutput.len));\n  } finally {\n    if (__eqtsOutput.ptr) __eqtsBufferFree(__eqtsOutput.ptr, __eqtsOutput.len, __eqtsOutput.capacity);\n  }\n");
        }
        Runtime::Bun => {
            output.push_str("  const __eqtsOutput = new BigUint64Array(3);\n");
            writeln!(
                output,
                "  const __eqtsStatus = symbols.{}(ptr(__eqtsInput), BigInt(__eqtsInput.byteLength), ptr(__eqtsOutput));",
                function.symbol
            )
            .expect("writing to a string cannot fail");
            output.push_str("  let __eqtsText = \"\";\n  try {\n    if (__eqtsOutput[0] !== 0n) {\n      const __eqtsBytes = new Uint8Array(toArrayBuffer(Number(__eqtsOutput[0]), 0, Number(__eqtsOutput[1]))).slice();\n      __eqtsText = new TextDecoder().decode(__eqtsBytes);\n    }\n  } finally {\n    if (__eqtsOutput[0] !== 0n) symbols.eqts_buffer_free_v1(__eqtsOutput[0], __eqtsOutput[1], __eqtsOutput[2]);\n  }\n");
        }
        Runtime::Deno => {
            output.push_str("  const __eqtsOutput = new BigUint64Array(3);\n");
            writeln!(
                output,
                "  const __eqtsStatus = symbols.{}(__eqtsInput, BigInt(__eqtsInput.byteLength), __eqtsOutput);",
                function.symbol
            )
            .expect("writing to a string cannot fail");
            output.push_str("  const __eqtsPointer = __eqtsOutput[0] === 0n ? null : Deno.UnsafePointer.create(__eqtsOutput[0]);\n  let __eqtsText = \"\";\n  try {\n    if (__eqtsPointer) {\n      const __eqtsBytes = new Uint8Array(new Deno.UnsafePointerView(__eqtsPointer).getArrayBuffer(Number(__eqtsOutput[1]))).slice();\n      __eqtsText = new TextDecoder().decode(__eqtsBytes);\n    }\n  } finally {\n    if (__eqtsPointer) symbols.eqts_buffer_free_v1(__eqtsPointer, __eqtsOutput[1], __eqtsOutput[2]);\n  }\n");
        }
    }
    writeln!(
        output,
        "  checkStatus(__eqtsStatus, \"{}\", __eqtsText);",
        function.name
    )
    .expect("writing to a string cannot fail");
    if matches!(function.result, Type::Scalar(Scalar::Void)) {
        output.push_str("  return;\n");
    } else {
        output.push_str("  const __eqtsDecoded = JSON.parse(__eqtsText);\n");
        match &function.result {
            Type::Owned(OwnedType::Result { ok, error }) => {
                writeln!(
                    output,
                    "  if (Object.hasOwn(__eqtsDecoded, \"error\")) throw new EqtsError(\"RUST_ERROR\", {});",
                    wire_decode("__eqtsDecoded.error", error)
                )
                .expect("writing to a string cannot fail");
                writeln!(output, "  return {};", wire_decode("__eqtsDecoded.ok", ok))
                    .expect("writing to a string cannot fail");
            }
            ty => {
                writeln!(output, "  return {};", wire_decode("__eqtsDecoded", ty))
                    .expect("writing to a string cannot fail");
            }
        }
    }
    output.push_str("}\n");
}

fn wire_encode(expression: &str, ty: &Type) -> String {
    match ty {
        Type::Scalar(Scalar::U64 | Scalar::I64) => format!("{expression}.toString()"),
        Type::Owned(OwnedType::Bytes) => format!("Array.from({expression})"),
        Type::Owned(OwnedType::Vec { value }) => {
            format!(
                "{expression}.map((value) => {})",
                wire_encode("value", value)
            )
        }
        Type::Owned(OwnedType::Option { value }) => format!(
            "{expression} == null ? null : {}",
            wire_encode(expression, value)
        ),
        Type::Owned(OwnedType::Record { fields, .. }) => {
            let fields = fields
                .iter()
                .map(|field| {
                    format!(
                        "{}: {}",
                        field.name,
                        wire_encode(&format!("{expression}.{}", field.name), &field.ty)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {fields} }}")
        }
        Type::Owned(OwnedType::Result { .. }) => expression.to_string(),
        Type::Scalar(_) | Type::Owned(OwnedType::String | OwnedType::Enum { .. }) => {
            expression.to_string()
        }
    }
}

fn wire_decode(expression: &str, ty: &Type) -> String {
    match ty {
        Type::Scalar(Scalar::U64 | Scalar::I64) => format!("BigInt({expression})"),
        Type::Owned(OwnedType::Bytes) => format!("Uint8Array.from({expression})"),
        Type::Owned(OwnedType::Vec { value }) => {
            format!(
                "{expression}.map((value) => {})",
                wire_decode("value", value)
            )
        }
        Type::Owned(OwnedType::Option { value }) => format!(
            "{expression} == null ? null : {}",
            wire_decode(expression, value)
        ),
        Type::Owned(OwnedType::Record { fields, .. }) => {
            let fields = fields
                .iter()
                .map(|field| {
                    format!(
                        "{}: {}",
                        field.name,
                        wire_decode(&format!("{expression}.{}", field.name), &field.ty)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {fields} }}")
        }
        Type::Owned(OwnedType::Result { .. }) => expression.to_string(),
        Type::Scalar(_) | Type::Owned(OwnedType::String | OwnedType::Enum { .. }) => {
            expression.to_string()
        }
    }
}

fn render_wrapper(output: &mut String, function: &Function, runtime: Runtime) {
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let arguments = function
        .parameters
        .iter()
        .map(|parameter| js_input(&parameter.name, scalar_type(&parameter.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        "export function {}({parameters}) {{",
        typescript_name(&function.name)
    )
    .expect("writing to a string cannot fail");
    if matches!(function.result, Type::Scalar(Scalar::Void)) {
        writeln!(
            output,
            "  const status = {}({arguments});",
            runtime_call(runtime, function)
        )
        .expect("writing to a string cannot fail");
        writeln!(output, "  checkStatus(status, \"{}\");", function.name)
            .expect("writing to a string cannot fail");
    } else {
        writeln!(
            output,
            "  const output = {};",
            output_storage(runtime, scalar_type(&function.result))
        )
        .expect("writing to a string cannot fail");
        let output_argument = match runtime {
            Runtime::Koffi | Runtime::Deno => "output",
            Runtime::Bun => "ptr(output)",
        };
        let separator = if arguments.is_empty() { "" } else { ", " };
        writeln!(
            output,
            "  const status = {}({arguments}{separator}{output_argument});",
            runtime_call(runtime, function)
        )
        .expect("writing to a string cannot fail");
        writeln!(output, "  checkStatus(status, \"{}\");", function.name)
            .expect("writing to a string cannot fail");
        writeln!(
            output,
            "  return {};",
            js_output("output[0]", scalar_type(&function.result))
        )
        .expect("writing to a string cannot fail");
    }
    output.push_str("}\n");
}

fn runtime_call(runtime: Runtime, function: &Function) -> String {
    match runtime {
        Runtime::Koffi => format!("__eqts_{}", function.name),
        Runtime::Bun | Runtime::Deno => format!("symbols.{}", function.symbol),
    }
}

fn output_storage(runtime: Runtime, scalar: Scalar) -> String {
    match runtime {
        Runtime::Koffi => "[null]".to_string(),
        Runtime::Bun | Runtime::Deno => format!("new {}(1)", typed_array(scalar)),
    }
}

fn js_input(name: &str, scalar: Scalar) -> String {
    if matches!(scalar, Scalar::Bool) {
        format!("Number({name})")
    } else {
        name.to_string()
    }
}

fn js_output(expression: &str, scalar: Scalar) -> String {
    if matches!(scalar, Scalar::Bool) {
        format!("Boolean({expression})")
    } else {
        expression.to_string()
    }
}

fn typed_array(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Void | Scalar::Bool | Scalar::U8 => "Uint8Array",
        Scalar::U16 => "Uint16Array",
        Scalar::U32 => "Uint32Array",
        Scalar::U64 => "BigUint64Array",
        Scalar::I8 => "Int8Array",
        Scalar::I16 => "Int16Array",
        Scalar::I32 => "Int32Array",
        Scalar::I64 => "BigInt64Array",
        Scalar::F32 => "Float32Array",
        Scalar::F64 => "Float64Array",
    }
}

fn render_package_manifest(target: Target) -> Result<String> {
    let mut manifest = json!({
        "private": true,
        "type": "module",
        "types": "./index.d.ts",
        "exports": {
            ".": {
                "types": "./index.d.ts",
                "import": "./index.js",
                "default": "./index.js"
            }
        }
    });
    if target == Target::NodeKoffi {
        manifest["dependencies"] = json!({ "koffi": ">=2 <3" });
    }
    let mut output = serde_json::to_string_pretty(&manifest)?;
    output.push('\n');
    Ok(output)
}

fn render_root_package_manifest() -> Result<String> {
    let manifest = json!({
        "private": true,
        "exports": {
            ".": {
                "types": "./node-napi/index.d.ts",
                "node": "./node-napi/index.js",
                "default": "./wasm-browser/index.js"
            },
            "./node-koffi": {
                "types": "./node-koffi/index.d.ts",
                "default": "./node-koffi/index.js"
            },
            "./bun": {
                "types": "./bun/index.d.ts",
                "default": "./bun/index.js"
            },
            "./deno": {
                "types": "./deno/index.d.ts",
                "default": "./deno/index.js"
            },
            "./wasm": {
                "types": "./wasm/index.d.ts",
                "default": "./wasm/index.js"
            },
            "./wasm-browser": {
                "types": "./wasm-browser/index.d.ts",
                "default": "./wasm-browser/index.js"
            }
        },
        "dependencies": { "koffi": ">=2 <3" }
    });
    let mut output = serde_json::to_string_pretty(&manifest)?;
    output.push('\n');
    Ok(output)
}

fn scalar_type(ty: &Type) -> Scalar {
    match ty {
        Type::Scalar(scalar) => *scalar,
        Type::Owned(_) => unreachable!("owned type reached scalar code generation"),
    }
}

fn ts_return_type(ty: &Type) -> String {
    match ty {
        Type::Owned(OwnedType::Result { ok, .. }) => ts_type(ok),
        _ => ts_type(ty),
    }
}

fn ts_type(ty: &Type) -> String {
    match ty {
        Type::Scalar(Scalar::Void) => "void".to_string(),
        Type::Scalar(Scalar::Bool) => "boolean".to_string(),
        Type::Scalar(Scalar::U64 | Scalar::I64) => "bigint".to_string(),
        Type::Scalar(
            Scalar::U8
            | Scalar::U16
            | Scalar::U32
            | Scalar::I8
            | Scalar::I16
            | Scalar::I32
            | Scalar::F32
            | Scalar::F64,
        ) => "number".to_string(),
        Type::Owned(OwnedType::String) => "string".to_string(),
        Type::Owned(OwnedType::Bytes) => "Uint8Array".to_string(),
        Type::Owned(OwnedType::Vec { value }) => format!("Array<{}>", ts_type(value)),
        Type::Owned(OwnedType::Option { value }) => format!("{} | null", ts_type(value)),
        Type::Owned(OwnedType::Result { ok, .. }) => ts_type(ok),
        Type::Owned(OwnedType::Record { name, .. } | OwnedType::Enum { name, .. }) => name.clone(),
    }
}

fn c_type(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Void => "void",
        Scalar::Bool | Scalar::U8 => "uint8_t",
        Scalar::U16 => "uint16_t",
        Scalar::U32 => "uint32_t",
        Scalar::U64 => "uint64_t",
        Scalar::I8 => "int8_t",
        Scalar::I16 => "int16_t",
        Scalar::I32 => "int32_t",
        Scalar::I64 => "int64_t",
        Scalar::F32 => "float",
        Scalar::F64 => "double",
    }
}

fn bun_type(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Void => "FFIType.void",
        Scalar::Bool | Scalar::U8 => "FFIType.u8",
        Scalar::U16 => "FFIType.u16",
        Scalar::U32 => "FFIType.u32",
        Scalar::U64 => "FFIType.u64",
        Scalar::I8 => "FFIType.i8",
        Scalar::I16 => "FFIType.i16",
        Scalar::I32 => "FFIType.i32",
        Scalar::I64 => "FFIType.i64",
        Scalar::F32 => "FFIType.f32",
        Scalar::F64 => "FFIType.f64",
    }
}

fn deno_type(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Void => "void",
        Scalar::Bool | Scalar::U8 => "u8",
        Scalar::U16 => "u16",
        Scalar::U32 => "u32",
        Scalar::U64 => "u64",
        Scalar::I8 => "i8",
        Scalar::I16 => "i16",
        Scalar::I32 => "i32",
        Scalar::I64 => "i64",
        Scalar::F32 => "f32",
        Scalar::F64 => "f64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn add_function() -> Function {
        Function {
            module: "fixture".to_string(),
            name: "add".to_string(),
            symbol: "eqts_add".to_string(),
            abi: FunctionAbi::Scalar,
            kind: ExportKind::Function,
            parameters: vec![
                Parameter {
                    name: "a".to_string(),
                    ty: Type::Scalar(Scalar::U32),
                },
                Parameter {
                    name: "b".to_string(),
                    ty: Type::Scalar(Scalar::U32),
                },
            ],
            result: Type::Scalar(Scalar::U32),
            methods: Vec::new(),
        }
    }

    #[test]
    fn declarations_preserve_function_shape() {
        assert_eq!(
            render_declarations(&[add_function()]),
            "export declare function add(a: number, b: number): number;\n"
        );
    }

    #[test]
    fn bun_loader_uses_exported_symbol() {
        let loader = render_bun("./libmath.dylib", &[add_function()]);
        assert!(loader.contains("fileURLToPath(new URL(\"./libmath.dylib\""));
        assert!(loader.contains(
            "eqts_add: { args: [FFIType.u32, FFIType.u32, FFIType.ptr], returns: FFIType.i32 }"
        ));
        assert!(loader.contains("new Uint32Array(1)"));
        assert!(loader.contains("symbols.eqts_add(a, b, ptr(output))"));
        assert!(loader.contains("checkStatus(status, \"add\")"));
    }

    #[test]
    fn deno_loader_uses_exported_symbol() {
        let loader = render_deno("./libmath.dylib", &[add_function()]);
        assert!(loader.contains("Deno.dlopen(new URL(\"./libmath.dylib\""));
        assert!(!loader.contains(".pathname"));
        assert!(
            loader.contains(
                "eqts_add: { parameters: [\"u32\", \"u32\", \"buffer\"], result: \"i32\" }"
            )
        );
        assert!(loader.contains("symbols.eqts_add(a, b, output)"));
    }

    #[test]
    fn koffi_loader_uses_exported_symbol() {
        let loader = render_koffi("./libmath.dylib", &[add_function()]);
        assert!(loader.contains("fileURLToPath(new URL(\"./libmath.dylib\""));
        assert!(loader.contains("koffi.out(koffi.pointer(\"uint32_t\"))"));
        assert!(loader.contains("__eqts_add(a, b, output)"));
    }

    #[test]
    fn metadata_is_sorted_for_deterministic_generation() {
        let mut subtract = add_function();
        subtract.name = "subtract".to_string();
        subtract.symbol = "eqts_subtract".to_string();
        let functions = normalized_metadata(vec![subtract, add_function()])
            .expect("valid metadata should normalize");
        let names = functions
            .iter()
            .map(|function| function.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["add", "subtract"]);
    }

    #[test]
    fn duplicate_exports_are_rejected() {
        let error = normalized_metadata(vec![add_function(), add_function()])
            .expect_err("duplicate exports must fail");
        assert!(error.to_string().contains("duplicate exported function"));
    }

    #[test]
    fn invalid_identifiers_are_rejected() {
        for identifier in ["", "two words", "9lives", "default", "let"] {
            assert!(validate_identifier(identifier).is_err(), "{identifier:?}");
        }
    }

    #[test]
    fn void_parameters_are_rejected() {
        let mut function = add_function();
        function.parameters[0].ty = Type::Scalar(Scalar::Void);
        let error = normalized_metadata(vec![function]).expect_err("void parameters must fail");
        assert!(error.to_string().contains("cannot have type void"));
    }

    #[test]
    fn all_selects_every_target() {
        assert_eq!(
            selected_targets(Target::All),
            [
                Target::NodeKoffi,
                Target::Bun,
                Target::Deno,
                Target::NodeNapi,
                Target::Wasm,
                Target::WasmBrowser
            ]
        );
    }

    #[test]
    fn individual_compiled_targets_are_selected() {
        assert_eq!(selected_targets(Target::NodeKoffi), [Target::NodeKoffi]);
        assert_eq!(selected_targets(Target::Bun), [Target::Bun]);
        assert_eq!(selected_targets(Target::Deno), [Target::Deno]);
        assert_eq!(selected_targets(Target::NodeNapi), [Target::NodeNapi]);
        assert_eq!(selected_targets(Target::Wasm), [Target::Wasm]);
        assert_eq!(selected_targets(Target::WasmBrowser), [Target::WasmBrowser]);
    }

    #[test]
    fn browser_wasm_loader_initializes_once() {
        let loader = render_bridge_loader(BridgeRuntime::WasmBrowser, &[add_function()]);
        assert!(loader.contains(
            "if (!ready) ready = Promise.resolve().then(() => init({ module_or_path: input }))"
        ));
        assert!(loader.contains("return ready"));
        assert!(loader.contains("new URL(\"./bindings_bg.wasm\", import.meta.url)"));
        assert!(loader.contains("export function add(a, b)"));
        assert!(!loader.contains("export * from \"./bindings.js\""));
    }

    #[test]
    fn browser_wasm_initialize_is_concurrency_safe() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        fs::write(
            temporary.path().join("bindings.js"),
            "let calls = 0; export default async function init({ module_or_path }) { calls++; await module_or_path.text(); return calls; } export function add() {} export { calls };",
        )
        .expect("write bindings");
        fs::write(
            temporary.path().join("index.js"),
            render_bridge_loader(BridgeRuntime::WasmBrowser, &[add_function()]),
        )
        .expect("write loader");
        fs::write(
            temporary.path().join("test.js"),
            "import { initialize } from './index.js'; import { calls } from './bindings.js'; const response = new Response('wasm'); const [a,b] = await Promise.all([initialize(response), initialize(response)]); if (a !== b || calls !== 1) throw new Error(`init race: ${a} ${b} ${calls}`);",
        )
        .expect("write test");
        let status = Command::new("bun")
            .arg("test.js")
            .current_dir(temporary.path())
            .status()
            .expect("Bun must be installed for generated JavaScript tests");
        assert!(status.success());
    }

    #[test]
    fn native_generation_removes_stale_files() {
        let temporary = tempfile::tempdir().expect("temporary directory should exist");
        let library = temporary.path().join(if cfg!(target_os = "windows") {
            "fixture.dll"
        } else if cfg!(target_os = "macos") {
            "libfixture.dylib"
        } else {
            "libfixture.so"
        });
        fs::write(&library, b"fixture").expect("fixture library should be writable");
        let stale = temporary.path().join("bun/index.ts");
        fs::create_dir_all(stale.parent().expect("stale file should have parent"))
            .expect("stale directory should be writable");
        fs::write(&stale, b"stale").expect("stale file should be writable");
        generate_target(Target::Bun, temporary.path(), &library, &[add_function()])
            .expect("generation should succeed");
        assert!(!stale.exists());
    }

    #[test]
    fn package_manifest_has_types_and_runtime_dependency() {
        let manifest: serde_json::Value = serde_json::from_str(
            &render_package_manifest(Target::NodeKoffi).expect("manifest should serialize"),
        )
        .expect("manifest should be JSON");
        assert_eq!(manifest["exports"]["."]["types"], "./index.d.ts");
        assert_eq!(manifest["dependencies"]["koffi"], ">=2 <3");
    }

    #[test]
    fn metadata_schema_and_new_scalars_are_supported() {
        let functions = parse_metadata(
            br#"{"schema_version":1,"functions":[{"module":"fixture","name":"enabled","symbol":"eqts_enabled","parameters":[{"name":"value","scalar":"bool"},{"name":"count","scalar":"u64"}],"result":"i64"}]}"#,
        )
        .expect("versioned metadata should parse");
        assert_eq!(
            render_declarations(&functions),
            "export declare function enabled(value: boolean, count: bigint): bigint;\n"
        );
    }

    #[test]
    fn unsupported_metadata_schema_is_rejected() {
        let error = parse_metadata(br#"{"schema_version":4,"functions":[]}"#)
            .expect_err("unknown schema must fail");
        assert!(error.to_string().contains("schema version 4"));
    }

    #[test]
    fn reactive_schema_without_export_descriptors_is_rejected() {
        let metadata = br#"{"schema_version":3,"capabilities":{"owned_values":true,"objects":true,"async_functions":true,"callbacks":true,"traits":true,"streams":true,"iterators":true},"functions":[{"module":"fixture","name":"resource","symbol":"eqts_resource","abi":"scalar","parameters":[],"result":{"kind":"scalar","scalar":"u64"}}]}"#;
        let error = parse_metadata(metadata).expect_err("ambiguous reactive handle must fail");
        assert!(error.to_string().contains("without per-export descriptors"));
        assert!(error.to_string().contains("ordinary u64 values"));
    }

    #[test]
    fn schema_v3_explicit_function_exports_parse_with_default_capabilities() {
        let functions = parse_metadata(
            br#"{"schema_version":3,"capabilities":{"owned_values":true,"objects":true,"async_functions":true,"callbacks":true,"traits":true,"streams":true,"iterators":true},"functions":[{"module":"example","name":"add","symbol":"eqts_add","abi":"scalar","kind":{"kind":"function"},"parameters":[{"name":"left","ty":{"kind":"scalar","scalar":"u32"}},{"name":"right","ty":{"kind":"scalar","scalar":"u32"}}],"result":{"kind":"scalar","scalar":"u32"}}],"method_sets":[]}"#,
        )
        .expect("ordinary schema v3 functions must parse when kind is explicit");
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name, "add");
        assert!(matches!(functions[0].kind, ExportKind::Function));
    }

    #[test]
    fn schema_v3_empty_inventory_parses_with_default_capabilities() {
        parse_metadata(
            br#"{"schema_version":3,"capabilities":{"owned_values":true,"objects":true,"async_functions":true,"callbacks":true,"traits":true,"streams":true,"iterators":true},"functions":[],"method_sets":[]}"#,
        )
        .expect("empty schema v3 metadata must parse");
    }

    #[test]
    fn unknown_export_kind_is_rejected() {
        let error = parse_metadata(
            br#"{"schema_version":3,"capabilities":{"owned_values":true},"functions":[{"module":"fixture","name":"work","symbol":"eqts_work","abi":"json","kind":"teleport","parameters":[],"result":{"kind":"scalar","scalar":"u64"}}]}"#,
        )
        .expect_err("unknown export kind must fail");
        assert!(format!("{error:#}").contains("unknown export kind teleport"));
    }

    #[test]
    fn empty_module_is_rejected() {
        let mut function = add_function();
        function.module.clear();
        let error = normalized_metadata(vec![function]).expect_err("empty module must fail");
        assert!(error.to_string().contains("empty module"));
    }

    #[test]
    fn invalid_symbols_are_rejected() {
        for symbol in ["", "1add", "eqts-add"] {
            let mut function = add_function();
            function.symbol = symbol.to_string();
            assert!(normalized_metadata(vec![function]).is_err(), "{symbol:?}");
        }
    }

    #[test]
    fn empty_enum_is_rejected() {
        let mut function = add_function();
        function.abi = FunctionAbi::Json;
        function.result = Type::Owned(OwnedType::Enum {
            name: "Empty".to_string(),
            variants: Vec::new(),
        });
        let error = normalized_metadata(vec![function]).expect_err("empty enum must fail");
        assert!(
            error
                .to_string()
                .contains("must contain at least one variant")
        );
    }

    #[test]
    fn typescript_names_camel_case_snake_identifiers() {
        assert_eq!(typescript_name("add"), "add");
        assert_eq!(typescript_name("add_u64"), "addU64");
        assert_eq!(typescript_name("on_event_name"), "onEventName");
        assert_eq!(typescript_name("foo__bar"), "fooBar");
        assert_eq!(typescript_name("_leading"), "Leading");
    }

    #[test]
    fn cli_build_parses_target_and_defaults() {
        let cli = Cli::try_parse_from(["cargo", "eqts", "build", "--target", "bun"])
            .expect("valid CLI must parse");
        match cli.command {
            Commands::Eqts {
                command:
                    EqtsCommand::Build {
                        target,
                        release,
                        out_dir,
                    },
            } => {
                assert_eq!(target, Target::Bun);
                assert!(!release);
                assert_eq!(out_dir, PathBuf::from("dist"));
            }
        }
    }

    #[test]
    fn cli_build_requires_a_target() {
        let Err(error) = Cli::try_parse_from(["cargo", "eqts", "build"]) else {
            panic!("target is required");
        };
        assert!(error.to_string().contains("required"));
    }

    #[test]
    fn cli_build_rejects_unknown_targets() {
        let Err(error) = Cli::try_parse_from(["cargo", "eqts", "build", "--target", "jvm"]) else {
            panic!("unknown target must fail");
        };
        assert!(error.to_string().contains("invalid value"));
    }

    fn reactive_function(kind: ExportKind) -> Function {
        Function {
            module: "fixture".to_string(),
            name: "events".to_string(),
            symbol: "eqts_events".to_string(),
            abi: FunctionAbi::Json,
            kind,
            parameters: Vec::new(),
            result: Type::Scalar(Scalar::U64),
            methods: Vec::new(),
        }
    }

    #[test]
    fn v3_reactive_descriptors_generate_canonical_declarations() {
        let functions = parse_metadata(
            br#"{"schema_version":3,"capabilities":{"owned_values":true,"objects":true,"async_functions":true,"callbacks":true,"traits":true,"streams":true,"iterators":true},"functions":[{"module":"fixture","name":"events","symbol":"eqts_events","abi":"json","kind":"stream","parameters":[],"result":{"kind":"scalar","scalar":"u64"},"item":{"kind":"string"}}]}"#,
        )
        .expect("described reactive metadata should parse");
        let declarations = render_declarations(&functions);
        assert!(declarations.contains("interface EqtsHandle<T>"));
        assert!(declarations.contains("events(): EqtsHandle<string>"));
    }

    #[test]
    fn v3_nested_reactive_descriptor_shape_parses() {
        let functions = parse_metadata(
            br#"{"schema_version":3,"capabilities":{"streams":true},"functions":[{"module":"fixture","name":"events","symbol":"eqts_events","abi":"json","kind":{"kind":"stream","item":{"kind":"string"}},"parameters":[],"result":{"kind":"scalar","scalar":"u64"}}]}"#,
        )
        .expect("landed nested descriptor should parse");
        assert!(matches!(functions[0].kind, ExportKind::Stream { .. }));
    }

    #[test]
    fn native_reactive_constructors_use_json_arguments_and_handle_output() {
        let mut function = reactive_function(ExportKind::Stream {
            item: Type::Owned(OwnedType::String),
        });
        function.parameters = vec![Parameter {
            name: "start_at".to_string(),
            ty: Type::Scalar(Scalar::U64),
        }];

        let koffi = render_koffi("./fixture.dylib", std::slice::from_ref(&function));
        assert!(
            koffi.contains("[\"uint8_t *\", \"size_t\", koffi.out(koffi.pointer(\"uint64_t\"))]")
        );
        assert!(koffi.contains("JSON.stringify([startAt.toString()])"));
        assert!(koffi.contains("(__eqtsInput, __eqtsInput.byteLength, __eqtsHandleOutput)"));

        let bun = render_bun("./fixture.dylib", std::slice::from_ref(&function));
        assert!(bun.contains("args: [FFIType.ptr, FFIType.u64, FFIType.ptr]"));
        assert!(bun.contains(
            "(ptr(__eqtsInput), BigInt(__eqtsInput.byteLength), ptr(__eqtsHandleOutput))"
        ));

        let deno = render_deno("./fixture.dylib", &[function]);
        assert!(deno.contains("parameters: [\"buffer\", \"usize\", \"buffer\"]"));
        assert!(deno.contains("(__eqtsInput, BigInt(__eqtsInput.byteLength), __eqtsHandleOutput)"));
    }

    #[test]
    fn reactive_runtime_enforces_lifecycle_backpressure_and_js_dispatch() {
        let loader = render_bridge_loader(
            BridgeRuntime::NodeNapi,
            &[
                reactive_function(ExportKind::Stream {
                    item: Type::Owned(OwnedType::String),
                }),
                reactive_function(ExportKind::Callback {
                    value: Type::Owned(OwnedType::String),
                    callback_parameter: "on_event".to_string(),
                }),
            ],
        );
        assert!(loader.contains("new FinalizationRegistry"));
        assert!(loader.contains("if (disposed) return"));
        assert!(loader.contains("USE_AFTER_DISPOSE"));
        assert!(loader.contains("CONCURRENT_NEXT"));
        assert!(loader.contains("poll.status === 13"));
        assert!(!loader.contains("new Worker"));
    }

    #[test]
    fn reactive_async_uses_abort_signal_and_cancels() {
        let function = reactive_function(ExportKind::Async {
            value: Type::Owned(OwnedType::String),
        });
        let declarations = render_declarations(std::slice::from_ref(&function));
        assert!(declarations.contains("options?: { signal?: AbortSignal }"));
        let loader = render_bridge_loader(BridgeRuntime::WasmBrowser, &[function]);
        assert!(loader.contains("signal?.aborted"));
        assert!(loader.contains("bindings.eqtsReactiveCancel(handle)"));
        assert!(loader.contains("removeEventListener"));
    }

    #[test]
    fn reactive_async_disposes_handle_when_polling_fails() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let function = reactive_function(ExportKind::Async {
            value: Type::Owned(OwnedType::String),
        });
        fs::write(
            temporary.path().join("bindings.js"),
            "export let disposals = 0; export function events() { return 7n; } export function eqtsReactivePoll() { return { status: 99, value: null }; } export function eqtsReactiveCancel() {} export function eqtsHandleDispose() { disposals++; }",
        )
        .expect("write bindings");
        fs::write(
            temporary.path().join("index.js"),
            render_bridge_loader(BridgeRuntime::NodeNapi, &[function]),
        )
        .expect("write loader");
        fs::write(
            temporary.path().join("test.js"),
            "import { events } from './index.js'; import { disposals } from './bindings.js'; let rejected = false; try { await events(); } catch (error) { rejected = error.code === 'REACTIVE_POLL'; } if (!rejected || disposals !== 1) throw new Error(`cleanup failure: ${rejected} ${disposals}`);",
        )
        .expect("write test");
        let status = Command::new("bun")
            .arg("test.js")
            .current_dir(temporary.path())
            .status()
            .expect("Bun must be installed for generated JavaScript tests");
        assert!(status.success());
    }

    #[test]
    fn reactive_stream_disposes_handle_when_polling_fails() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let function = reactive_function(ExportKind::Stream {
            item: Type::Owned(OwnedType::String),
        });
        fs::write(
            temporary.path().join("bindings.js"),
            "export let disposals = 0; export function events() { return 7n; } export function eqtsReactivePoll() { return { status: 99, value: null }; } export function eqtsReactiveCancel() {} export function eqtsHandleDispose() { disposals++; }",
        )
        .expect("write bindings");
        fs::write(
            temporary.path().join("index.js"),
            render_bridge_loader(BridgeRuntime::NodeNapi, &[function]),
        )
        .expect("write loader");
        fs::write(
            temporary.path().join("test.js"),
            "import { events } from './index.js'; import { disposals } from './bindings.js'; const stream = events(); let rejected = false; try { await stream.next(); } catch (error) { rejected = error.code === 'REACTIVE_POLL'; } if (!rejected || !stream.disposed || disposals !== 1) throw new Error(`cleanup failure: ${rejected} ${stream.disposed} ${disposals}`);",
        )
        .expect("write test");
        let status = Command::new("bun")
            .arg("test.js")
            .current_dir(temporary.path())
            .status()
            .expect("Bun must be installed for generated JavaScript tests");
        assert!(status.success());
    }

    #[test]
    fn callback_dispatch_is_js_threaded_and_errors_cancel_resource() {
        let function = reactive_function(ExportKind::Callback {
            value: Type::Owned(OwnedType::String),
            callback_parameter: "on_event".to_string(),
        });
        let declarations = render_declarations(std::slice::from_ref(&function));
        assert!(declarations.contains("onEvent: (value: string) => void | Promise<void>"));
        let loader = render_bridge_loader(BridgeRuntime::NodeNapi, &[function]);
        assert!(loader.contains("await callback(decode(__eqtsNormalize(poll.value)))"));
        assert!(!loader.contains("for await (const event of resource)"));
        assert!(loader.contains("bindings.eqtsReactiveCancel(handle)"));
        assert!(loader.contains("queueMicrotask(() => { throw error; })"));
    }

    #[test]
    fn object_and_trait_exports_fail_without_method_descriptors() {
        for kind in [
            ExportKind::Object {
                value: Type::Owned(OwnedType::String),
            },
            ExportKind::Trait {
                value: Type::Owned(OwnedType::String),
            },
        ] {
            let error = normalized_metadata(vec![reactive_function(kind)])
                .expect_err("methodless object or trait must fail");
            assert!(error.to_string().contains("has no methods"));
        }
    }

    #[test]
    fn object_methods_generate_typed_disposable_invocation() {
        let mut functions = parse_metadata(br#"{"schema_version":3,"capabilities":{"objects":true},"method_sets":[{"rust_type":"Counter","methods":[{"name":"increment","parameters":[{"name":"by","ty":{"kind":"scalar","scalar":"u32"}}],"result":{"kind":"scalar","scalar":"u32"},"mutable":true,"asynchronous":false}]}],"functions":[{"module":"fixture","name":"counter","symbol":"eqts_counter","abi":"json","kind":{"kind":"object","value":{"kind":"record","name":"Counter","fields":[]}},"parameters":[],"result":{"kind":"scalar","scalar":"u64"}}]}"#).expect("object metadata parses");
        functions[0].methods.push(Method {
            name: "fetch".to_string(),
            parameters: Vec::new(),
            result: Type::Owned(OwnedType::String),
            mutable: false,
            asynchronous: true,
        });
        let declarations = render_declarations(&functions);
        assert!(declarations.contains("increment(by: number): number"));
        assert!(
            declarations.contains("fetch(options?: { signal?: AbortSignal }): Promise<string>")
        );
        let bridge = render_bridge_loader(BridgeRuntime::NodeNapi, &functions);
        assert!(bridge.contains("bindings.eqtsHandleInvoke(handle, \"increment\", [by])"));
        assert!(bridge.contains("if (resource.disposed)"));
        assert!(bridge.contains("bindings.eqtsHandleInvokeAsync(handle, \"fetch\", [])"));
        assert!(bridge.contains("options.signal"));
        let native = render_bun("./fixture.dylib", &functions);
        assert!(native.contains("eqts_handle_invoke_v1"));
        assert!(native.contains("eqts_handle_invoke_async_v1"));
        assert!(native.contains("JSON.stringify({ method, arguments: __eqtsArguments })"));
        assert!(!native.contains("method, arguments)"));
    }

    #[test]
    fn unsupported_future_capabilities_are_rejected_strictly() {
        for metadata in [
            br#"{"schema_version":2,"functions":[{"module":"fixture","name":"work","symbol":"eqts_work","abi":"json","async":true,"parameters":[],"result":{"kind":"string"}}]}"#.as_slice(),
            br#"{"schema_version":2,"functions":[{"module":"fixture","name":"work","symbol":"eqts_work","abi":"json","parameters":[{"name":"callback","ty":{"kind":"callback"}}],"result":{"kind":"string"}}]}"#.as_slice(),
            br#"{"schema_version":2,"functions":[{"module":"fixture","name":"work","symbol":"eqts_work","abi":"json","parameters":[],"result":{"kind":"object","name":"Worker"}}]}"#.as_slice(),
            br#"{"schema_version":2,"functions":[{"module":"fixture","name":"work","symbol":"eqts_work","abi":"json","parameters":[],"result":{"kind":"stream","value":{"kind":"string"}}}]}"#.as_slice(),
            br#"{"schema_version":2,"functions":[{"module":"fixture","name":"work","symbol":"eqts_work","abi":"json","parameters":[],"result":{"kind":"iterator","value":{"kind":"string"}}}]}"#.as_slice(),
            br#"{"schema_version":2,"functions":[{"module":"fixture","name":"work","symbol":"eqts_work","abi":"json","parameters":[],"result":{"kind":"trait","name":"Worker"}}]}"#.as_slice(),
        ] {
            assert!(parse_metadata(metadata).is_err());
        }
    }

    #[test]
    fn scalar_type_rejects_unrecognized_capability_fields() {
        let metadata = br#"{"schema_version":2,"functions":[{"module":"fixture","name":"work","symbol":"eqts_work","abi":"scalar","parameters":[],"result":{"kind":"scalar","scalar":"u32","stream":true}}]}"#;
        assert!(parse_metadata(metadata).is_err());
    }

    #[test]
    fn declared_capabilities_allow_owned_values_only() {
        let metadata = br#"{"schema_version":2,"capabilities":{"owned_values":true,"objects":false,"async_functions":false,"callbacks":false,"traits":false,"streams":false,"iterators":false},"functions":[]}"#;
        assert!(parse_metadata(metadata).is_ok());
    }

    #[test]
    fn declared_unsupported_capability_fails_before_codegen() {
        for capability in [
            "objects",
            "async_functions",
            "callbacks",
            "traits",
            "streams",
            "iterators",
        ] {
            let metadata = format!(
                r#"{{"schema_version":2,"capabilities":{{"owned_values":true,"{capability}":true}},"functions":[]}}"#
            );
            let error = parse_metadata(metadata.as_bytes())
                .expect_err("unsupported capability must stop generation");
            assert!(error.to_string().contains("unsupported eqts capabilities"));
        }
    }

    #[test]
    fn bool_and_bigint_wrappers_convert_values() {
        let function = Function {
            module: "fixture".to_string(),
            name: "toggle".to_string(),
            symbol: "custom_toggle".to_string(),
            abi: FunctionAbi::Scalar,
            kind: ExportKind::Function,
            parameters: vec![Parameter {
                name: "enabled".to_string(),
                ty: Type::Scalar(Scalar::Bool),
            }],
            result: Type::Scalar(Scalar::I64),
            methods: Vec::new(),
        };
        let loader = render_bun("./libmath.dylib", &[function]);
        assert!(loader.contains("custom_toggle"));
        assert!(loader.contains("Number(enabled), ptr(output)"));
        assert!(loader.contains("new BigInt64Array(1)"));
    }

    fn owned_function() -> Function {
        let person = Type::Owned(OwnedType::Record {
            name: "Person".to_string(),
            fields: vec![
                Field {
                    name: "name".to_string(),
                    ty: Type::Owned(OwnedType::String),
                },
                Field {
                    name: "id".to_string(),
                    ty: Type::Scalar(Scalar::U64),
                },
            ],
        });
        Function {
            module: "fixture".to_string(),
            name: "round_trip".to_string(),
            symbol: "eqts_round_trip".to_string(),
            abi: FunctionAbi::Json,
            kind: ExportKind::Function,
            parameters: vec![Parameter {
                name: "person".to_string(),
                ty: person.clone(),
            }],
            result: Type::Owned(OwnedType::Result {
                ok: Box::new(person),
                error: Box::new(Type::Owned(OwnedType::String)),
            }),
            methods: Vec::new(),
        }
    }

    #[test]
    fn version_two_owned_schema_generates_types() {
        let functions = parse_metadata(
            br#"{"schema_version":2,"functions":[{"module":"fixture","name":"load","symbol":"eqts_load","abi":"json","parameters":[{"name":"names","ty":{"kind":"vec","value":{"kind":"string"}}}],"result":{"kind":"option","value":{"kind":"bytes"}}}]}"#,
        )
        .expect("owned metadata should parse");
        assert_eq!(functions[0].abi, FunctionAbi::Json);
        assert!(
            render_declarations(&functions)
                .contains("load(names: Array<string>): Uint8Array | null")
        );
    }

    #[test]
    fn version_two_tagged_scalar_schema_parses() {
        let functions = parse_metadata(
            br#"{"schema_version":2,"functions":[{"module":"fixture","name":"add","symbol":"eqts_add","abi":"scalar","parameters":[{"name":"a","ty":{"kind":"scalar","scalar":"u32"}}],"result":{"kind":"scalar","scalar":"u64"}}]}"#,
        )
        .expect("canonical tagged scalar metadata should parse");
        assert_eq!(
            render_declarations(&functions),
            "export declare function add(a: number): bigint;\n"
        );
    }

    #[test]
    fn owned_declarations_include_records_and_throwing_result() {
        let declarations = render_declarations(&[owned_function()]);
        assert!(declarations.contains("export interface Person"));
        assert!(declarations.contains("id: bigint;"));
        assert!(declarations.contains("export declare class EqtsError"));
        assert!(declarations.contains("roundTrip(person: Person): Person"));
    }

    #[test]
    fn json_loaders_use_owned_buffer_and_free_it() {
        let function = owned_function();
        let koffi = render_koffi("./libfixture.dylib", std::slice::from_ref(&function));
        assert!(koffi.contains("koffi.out(koffi.pointer(OwnedBuffer))"));
        assert!(koffi.contains(
            "__eqtsBufferFree(__eqtsOutput.ptr, __eqtsOutput.len, __eqtsOutput.capacity)"
        ));
        assert!(koffi.contains("person.id.toString()"));
        assert!(koffi.contains("BigInt(__eqtsDecoded.ok.id)"));

        let bun = render_bun("./libfixture.dylib", std::slice::from_ref(&function));
        assert!(bun.contains("new BigUint64Array(3)"));
        assert!(bun.contains("toArrayBuffer(Number(__eqtsOutput[0]), 0, Number(__eqtsOutput[1]))"));
        assert!(bun.contains(
            "symbols.eqts_buffer_free_v1(__eqtsOutput[0], __eqtsOutput[1], __eqtsOutput[2])"
        ));

        let deno = render_deno("./libfixture.dylib", &[function]);
        assert!(deno.contains("Deno.UnsafePointer.create(__eqtsOutput[0])"));
        assert!(deno.contains(
            "symbols.eqts_buffer_free_v1(__eqtsPointer, __eqtsOutput[1], __eqtsOutput[2])"
        ));
    }

    #[test]
    fn json_loader_internal_names_do_not_collide_with_parameters() {
        let function = Function {
            module: "fixture".to_string(),
            name: "collision".to_string(),
            symbol: "eqts_collision".to_string(),
            abi: FunctionAbi::Json,
            kind: ExportKind::Function,
            parameters: [
                "input", "output", "status", "text", "decoded", "bytes", "pointer",
            ]
            .into_iter()
            .map(|name| Parameter {
                name: name.to_string(),
                ty: Type::Owned(OwnedType::String),
            })
            .collect(),
            result: Type::Owned(OwnedType::String),
            methods: Vec::new(),
        };
        for loader in [
            render_koffi("./libfixture.dylib", std::slice::from_ref(&function)),
            render_bun("./libfixture.dylib", std::slice::from_ref(&function)),
            render_deno("./libfixture.dylib", std::slice::from_ref(&function)),
        ] {
            assert!(loader.contains("const __eqtsInput"));
            assert!(loader.contains("const __eqtsOutput"));
            assert!(!loader.contains("const input ="));
            assert!(!loader.contains("const output ="));
        }
    }

    #[test]
    fn napi_and_wasm_bridges_normalize_owned_values() {
        let function = owned_function();
        let napi = render_bridge_loader(BridgeRuntime::NodeNapi, std::slice::from_ref(&function));
        assert!(napi.contains("import * as bindings from \"./bindings.js\""));
        assert!(napi.contains("person.id.toString()"));
        assert!(napi.contains("BigInt(__eqtsResult.id)"));
        assert!(napi.contains("throw new EqtsError(\"RUST_ERROR\", __eqtsDetail)"));
        assert!(napi.contains("JSON.parse(__eqtsDetail)"));
        assert!(napi.contains("Object.fromEntries"));

        let wasm = render_bridge_loader(BridgeRuntime::WasmNode, &[function]);
        assert!(wasm.contains("import bindings from \"./bindings.cjs\""));
        assert!(wasm.contains("export class EqtsError"));
    }

    #[test]
    fn bridge_converts_bytes_and_options_to_canonical_values() {
        let function = Function {
            module: "fixture".to_string(),
            name: "maybe_bytes".to_string(),
            symbol: "eqts_maybe_bytes".to_string(),
            abi: FunctionAbi::Json,
            kind: ExportKind::Function,
            parameters: vec![Parameter {
                name: "value".to_string(),
                ty: Type::Owned(OwnedType::Bytes),
            }],
            result: Type::Owned(OwnedType::Option {
                value: Box::new(Type::Owned(OwnedType::Bytes)),
            }),
            methods: Vec::new(),
        };
        let loader = render_bridge_loader(BridgeRuntime::WasmBrowser, &[function]);
        assert!(loader.contains("Array.from(value)"));
        assert!(loader.contains("Uint8Array.from"));
        assert!(loader.contains("== null ? null"));
    }

    #[test]
    fn scalar_loaders_do_not_require_buffer_free_symbol() {
        let loader = render_bun("./libfixture.dylib", &[add_function()]);
        assert!(!loader.contains("eqts_buffer_free_v1:"));
    }

    #[test]
    fn root_manifest_exposes_every_transport() {
        let manifest: serde_json::Value =
            serde_json::from_str(&render_root_package_manifest().expect("manifest should render"))
                .expect("manifest should parse");
        for path in [".", "./node-koffi", "./bun", "./deno", "./wasm"] {
            assert!(manifest["exports"].get(path).is_some(), "{path}");
        }
    }
}
