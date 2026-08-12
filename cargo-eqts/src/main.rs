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

#[derive(Clone, Debug, Deserialize)]
struct Function {
    module: String,
    name: String,
    symbol: String,
    #[serde(default)]
    abi: FunctionAbi,
    parameters: Vec<Parameter>,
    result: Type,
}

#[derive(Debug, Deserialize)]
struct MetadataDocument {
    schema_version: u32,
    functions: Vec<Function>,
}

#[repr(C)]
struct MetadataSlice {
    ptr: *const u8,
    len: usize,
}

#[derive(Clone, Debug, Deserialize)]
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
    let native_targets = targets
        .iter()
        .copied()
        .filter(|target| matches!(target, Target::NodeKoffi | Target::Bun | Target::Deno))
        .collect::<Vec<_>>();
    if !native_targets.is_empty() {
        cargo_build(package, release)?;
        let library = library_path(&metadata, package, release)?;
        let functions = normalized_metadata(load_metadata(&library)?)?;
        for target in native_targets {
            generate_target(target, out_dir, &library, &functions)?;
        }
    }
    for target in targets {
        match target {
            Target::NodeNapi => build_node_napi(package, release, out_dir)?,
            Target::Wasm | Target::WasmBrowser => build_wasm(package, release, out_dir, target)?,
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

fn build_node_napi(package: &Package, release: bool, out_dir: &Path) -> Result<()> {
    let directory = package_directory(package)?;
    let output = directory.join(out_dir).join(target_name(Target::NodeNapi));
    fs::create_dir_all(&output)?;
    let mut command = Command::new("bun");
    command
        .current_dir(&directory)
        .args(["x", "napi", "build", "--platform", "--output-dir"]);
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
    run_backend(command, "napi-rs")
}

fn build_wasm(package: &Package, release: bool, out_dir: &Path, target: Target) -> Result<()> {
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
    fs::create_dir_all(&output)?;
    let mut bindgen = Command::new("wasm-bindgen");
    let (name, bindgen_target) = if target == Target::WasmBrowser {
        ("bindings", "web")
    } else {
        ("index", "nodejs")
    };
    bindgen
        .current_dir(&directory)
        .arg(wasm)
        .args(["--out-dir"])
        .arg(&output)
        .args(["--out-name", name, "--target", bindgen_target]);
    run_backend(bindgen, "wasm-bindgen")?;
    if target == Target::WasmBrowser {
        fs::write(output.join("index.js"), render_browser_wasm_loader())?;
        fs::write(
            output.join("index.d.ts"),
            render_browser_wasm_declarations(),
        )?;
        fs::write(
            output.join("package.json"),
            render_package_manifest(Target::WasmBrowser)?,
        )?;
    }
    Ok(())
}

fn render_browser_wasm_loader() -> &'static str {
    "import init, * as bindings from \"./bindings.js\";\n\nlet ready;\n\nexport function initialize(input = new URL(\"./bindings_bg.wasm\", import.meta.url)) {\n  return ready ??= init({ module_or_path: input });\n}\n\nexport { bindings };\nexport * from \"./bindings.js\";\n"
}

fn render_browser_wasm_declarations() -> &'static str {
    "export function initialize(input?: import(\"./bindings.js\").InitInput | Promise<import(\"./bindings.js\").InitInput>): Promise<import(\"./bindings.js\").InitOutput>;\nexport * as bindings from \"./bindings.js\";\nexport * from \"./bindings.js\";\n"
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
    command.args(["build", "-p", package.name.as_str(), "--locked"]);
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
    let document: MetadataDocument =
        serde_json::from_slice(bytes).context("eqts metadata is invalid JSON")?;
    if !matches!(document.schema_version, 1 | 2) {
        bail!(
            "unsupported eqts metadata schema version {}; expected 1 or 2",
            document.schema_version
        );
    }
    Ok(document.functions)
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

fn render_declarations(functions: &[Function]) -> String {
    let mut output = String::new();
    let mut definitions = std::collections::BTreeMap::new();
    for function in functions {
        for parameter in &function.parameters {
            collect_definitions(&parameter.ty, &mut definitions);
        }
        collect_definitions(&function.result, &mut definitions);
    }
    for definition in definitions.values() {
        output.push_str(definition);
        output.push('\n');
    }
    if functions
        .iter()
        .any(|function| function.abi == FunctionAbi::Json)
    {
        output.push_str("export declare class EqtsError extends Error {\n  constructor(code: string, value: unknown);\n  readonly code: string;\n  readonly value: unknown;\n}\n\n");
    }
    for function in functions {
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| format!("{}: {}", parameter.name, ts_type(&parameter.ty)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "export declare function {}({parameters}): {};",
            typescript_name(&function.name),
            ts_return_type(&function.result)
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
    let owned_buffer = if functions
        .iter()
        .any(|function| function.abi == FunctionAbi::Json)
    {
        "const OwnedBuffer = koffi.struct({ ptr: \"void *\", len: \"size_t\", capacity: \"size_t\" });\nconst __eqtsBufferFree = library.func(\"eqts_buffer_free_v1\", \"void\", [\"void *\", \"size_t\", \"size_t\"]);\n"
    } else {
        ""
    };
    let mut output = format!(
        "import koffi from \"koffi\";\nimport {{ fileURLToPath }} from \"node:url\";\n\nconst library = koffi.load(fileURLToPath(new URL(\"{path}\", import.meta.url)));\n{owned_buffer}\n{}\n",
        js_helpers()
    );
    for function in functions {
        if function.abi == FunctionAbi::Json {
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
    let mut output = "import { dlopen, FFIType, ptr, toArrayBuffer } from \"bun:ffi\";\nimport { fileURLToPath } from \"node:url\";\n\n".to_string();
    writeln!(
        output,
        "const {{ symbols }} = dlopen(fileURLToPath(new URL(\"{path}\", import.meta.url)), {{"
    )
    .expect("writing to a string cannot fail");
    for function in functions {
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
    {
        output.push_str("  eqts_buffer_free_v1: { args: [FFIType.u64, FFIType.u64, FFIType.u64], returns: FFIType.void },\n");
    }
    output.push_str("});\n\n");
    output.push_str(js_helpers());
    output.push('\n');
    for function in functions {
        if function.abi == FunctionAbi::Json {
            render_json_wrapper(&mut output, function, Runtime::Bun);
        } else {
            render_wrapper(&mut output, function, Runtime::Bun);
        }
    }
    output
}

fn render_deno(path: &str, functions: &[Function]) -> String {
    let mut output =
        format!("const {{ symbols }} = Deno.dlopen(new URL(\"{path}\", import.meta.url), {{\n");
    for function in functions {
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
    {
        output.push_str("  eqts_buffer_free_v1: { parameters: [\"pointer\", \"usize\", \"usize\"], result: \"void\" },\n");
    }
    output.push_str("});\n\n");
    output.push_str(js_helpers());
    output.push('\n');
    for function in functions {
        if function.abi == FunctionAbi::Json {
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
    "export class EqtsError extends Error {\n  constructor(code, value) {\n    super(typeof value === \"string\" ? value : `eqts error: ${code}`);\n    this.name = \"EqtsError\";\n    this.code = code;\n    this.value = value;\n  }\n}\n\nfunction checkStatus(status, name, detail) {\n  if (status === 0) return;\n  const code = { 1: \"RUST_PANIC\", 2: \"NULL_OUTPUT\", 3: \"INVALID_INPUT\", 4: \"ENCODE_FAILURE\" }[status] ?? \"ABI_ERROR\";\n  throw new EqtsError(code, detail || `eqts call ${name} failed with ABI status ${status}`);\n}\n"
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
        "  const input = new TextEncoder().encode(JSON.stringify([{inputs}]));"
    )
    .expect("writing to a string cannot fail");
    match runtime {
        Runtime::Koffi => {
            output.push_str("  const output = {};\n");
            writeln!(
                output,
                "  const status = __eqts_{}(input, input.byteLength, output);",
                function.name
            )
            .expect("writing to a string cannot fail");
            output.push_str("  let text = \"\";\n  try {\n    if (output.ptr) text = koffi.decode(output.ptr, \"char\", Number(output.len));\n  } finally {\n    if (output.ptr) __eqtsBufferFree(output.ptr, output.len, output.capacity);\n  }\n");
        }
        Runtime::Bun => {
            output.push_str("  const output = new BigUint64Array(3);\n");
            writeln!(
                output,
                "  const status = symbols.{}(ptr(input), BigInt(input.byteLength), ptr(output));",
                function.symbol
            )
            .expect("writing to a string cannot fail");
            output.push_str("  let text = \"\";\n  try {\n    if (output[0] !== 0n) {\n      const bytes = new Uint8Array(toArrayBuffer(Number(output[0]), 0, Number(output[1]))).slice();\n      text = new TextDecoder().decode(bytes);\n    }\n  } finally {\n    if (output[0] !== 0n) symbols.eqts_buffer_free_v1(output[0], output[1], output[2]);\n  }\n");
        }
        Runtime::Deno => {
            output.push_str("  const output = new BigUint64Array(3);\n");
            writeln!(
                output,
                "  const status = symbols.{}(input, BigInt(input.byteLength), output);",
                function.symbol
            )
            .expect("writing to a string cannot fail");
            output.push_str("  const pointer = output[0] === 0n ? null : Deno.UnsafePointer.create(output[0]);\n  let text = \"\";\n  try {\n    if (pointer) {\n      const bytes = new Uint8Array(new Deno.UnsafePointerView(pointer).getArrayBuffer(Number(output[1]))).slice();\n      text = new TextDecoder().decode(bytes);\n    }\n  } finally {\n    if (pointer) symbols.eqts_buffer_free_v1(pointer, output[1], output[2]);\n  }\n");
        }
    }
    writeln!(
        output,
        "  checkStatus(status, \"{}\", text);",
        function.name
    )
    .expect("writing to a string cannot fail");
    if matches!(function.result, Type::Scalar(Scalar::Void)) {
        output.push_str("  return;\n");
    } else {
        output.push_str("  const decoded = JSON.parse(text);\n");
        match &function.result {
            Type::Owned(OwnedType::Result { ok, error }) => {
                writeln!(
                    output,
                    "  if (Object.hasOwn(decoded, \"error\")) throw new EqtsError(\"RUST_ERROR\", {});",
                    wire_decode("decoded.error", error)
                )
                .expect("writing to a string cannot fail");
                writeln!(output, "  return {};", wire_decode("decoded.ok", ok))
                    .expect("writing to a string cannot fail");
            }
            ty => {
                writeln!(output, "  return {};", wire_decode("decoded", ty))
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

    fn add_function() -> Function {
        Function {
            module: "fixture".to_string(),
            name: "add".to_string(),
            symbol: "eqts_add".to_string(),
            abi: FunctionAbi::Scalar,
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
        for identifier in ["", "two words", "9lives", "default"] {
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
        assert_eq!(selected_targets(Target::NodeNapi), [Target::NodeNapi]);
        assert_eq!(selected_targets(Target::Wasm), [Target::Wasm]);
        assert_eq!(selected_targets(Target::WasmBrowser), [Target::WasmBrowser]);
    }

    #[test]
    fn browser_wasm_loader_initializes_once_and_reexports_bindings() {
        let loader = render_browser_wasm_loader();
        assert!(loader.contains("ready ??= init({ module_or_path: input })"));
        assert!(loader.contains("new URL(\"./bindings_bg.wasm\", import.meta.url)"));
        assert!(loader.contains("export * from \"./bindings.js\""));
        assert!(
            render_browser_wasm_declarations()
                .contains("Promise<import(\"./bindings.js\").InitOutput>")
        );
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
        let error = parse_metadata(br#"{"schema_version":3,"functions":[]}"#)
            .expect_err("unknown schema must fail");
        assert!(error.to_string().contains("schema version 3"));
    }

    #[test]
    fn bool_and_bigint_wrappers_convert_values() {
        let function = Function {
            module: "fixture".to_string(),
            name: "toggle".to_string(),
            symbol: "custom_toggle".to_string(),
            abi: FunctionAbi::Scalar,
            parameters: vec![Parameter {
                name: "enabled".to_string(),
                ty: Type::Scalar(Scalar::Bool),
            }],
            result: Type::Scalar(Scalar::I64),
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
            parameters: vec![Parameter {
                name: "person".to_string(),
                ty: person.clone(),
            }],
            result: Type::Owned(OwnedType::Result {
                ok: Box::new(person),
                error: Box::new(Type::Owned(OwnedType::String)),
            }),
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
        assert!(koffi.contains("__eqtsBufferFree(output.ptr, output.len, output.capacity)"));
        assert!(koffi.contains("person.id.toString()"));
        assert!(koffi.contains("BigInt(decoded.ok.id)"));

        let bun = render_bun("./libfixture.dylib", std::slice::from_ref(&function));
        assert!(bun.contains("new BigUint64Array(3)"));
        assert!(bun.contains("toArrayBuffer(Number(output[0]), 0, Number(output[1]))"));
        assert!(bun.contains("symbols.eqts_buffer_free_v1(output[0], output[1], output[2])"));

        let deno = render_deno("./libfixture.dylib", &[function]);
        assert!(deno.contains("Deno.UnsafePointer.create(output[0])"));
        assert!(deno.contains("symbols.eqts_buffer_free_v1(pointer, output[1], output[2])"));
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
