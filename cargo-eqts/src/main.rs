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
    All,
}

#[derive(Clone, Debug, Deserialize)]
struct Function {
    module: String,
    name: String,
    symbol: String,
    parameters: Vec<Parameter>,
    result: Scalar,
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
    scalar: Scalar,
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
            Target::Wasm => build_wasm(package, release, out_dir)?,
            Target::NodeKoffi | Target::Bun | Target::Deno | Target::All => {}
        }
    }
    Ok(())
}

fn selected_targets(target: Target) -> Vec<Target> {
    match target {
        Target::NodeKoffi | Target::Bun | Target::Deno | Target::NodeNapi | Target::Wasm => {
            vec![target]
        }
        Target::All => vec![
            Target::NodeKoffi,
            Target::Bun,
            Target::Deno,
            Target::NodeNapi,
            Target::Wasm,
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

fn build_wasm(package: &Package, release: bool, out_dir: &Path) -> Result<()> {
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
    let output = directory.join(out_dir).join(target_name(Target::Wasm));
    fs::create_dir_all(&output)?;
    let mut bindgen = Command::new("wasm-bindgen");
    bindgen
        .current_dir(&directory)
        .arg(wasm)
        .args(["--out-dir"])
        .arg(output)
        .args(["--out-name", "index", "--target", "nodejs"]);
    run_backend(bindgen, "wasm-bindgen")
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
    if document.schema_version != 1 {
        bail!(
            "unsupported eqts metadata schema version {}; expected 1",
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
            if matches!(parameter.scalar, Scalar::Void) {
                bail!(
                    "parameter {} in function {} cannot have type void",
                    parameter.name,
                    function.name
                );
            }
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
        Target::All => "all",
    }
}

fn render_declarations(functions: &[Function]) -> String {
    let mut output = String::new();
    for function in functions {
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| format!("{}: {}", parameter.name, ts_type(parameter.scalar)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "export declare function {}({parameters}): {};",
            function.name,
            ts_type(function.result)
        )
        .expect("writing to a string cannot fail");
    }
    output
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
        Target::NodeNapi | Target::Wasm | Target::All => {
            bail!("target {} has no loader renderer", target_name(target))
        }
    }
}

fn render_koffi(path: &str, functions: &[Function]) -> String {
    let mut output = format!(
        "import koffi from \"koffi\";\nimport {{ fileURLToPath }} from \"node:url\";\n\nconst library = koffi.load(fileURLToPath(new URL(\"{path}\", import.meta.url)));\n\nfunction checkStatus(status, name) {{\n  if (status !== 0) throw new Error(`eqts call ${{name}} failed with ABI status ${{status}}`);\n}}\n\n"
    );
    for function in functions {
        let mut abi_parameters = function
            .parameters
            .iter()
            .map(|parameter| format!("\"{}\"", c_type(parameter.scalar)))
            .collect::<Vec<_>>();
        if !matches!(function.result, Scalar::Void) {
            abi_parameters.push(format!(
                "koffi.out(koffi.pointer(\"{}\"))",
                c_type(function.result)
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
    let mut output = "import { dlopen, FFIType, ptr } from \"bun:ffi\";\nimport { fileURLToPath } from \"node:url\";\n\n".to_string();
    writeln!(
        output,
        "const {{ symbols }} = dlopen(fileURLToPath(new URL(\"{path}\", import.meta.url)), {{"
    )
    .expect("writing to a string cannot fail");
    for function in functions {
        let mut args = function
            .parameters
            .iter()
            .map(|parameter| bun_type(parameter.scalar))
            .collect::<Vec<_>>();
        if !matches!(function.result, Scalar::Void) {
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
    output.push_str("});\n\nfunction checkStatus(status, name) {\n  if (status !== 0) throw new Error(`eqts call ${name} failed with ABI status ${status}`);\n}\n\n");
    for function in functions {
        render_wrapper(&mut output, function, Runtime::Bun);
    }
    output
}

fn render_deno(path: &str, functions: &[Function]) -> String {
    let mut output =
        format!("const {{ symbols }} = Deno.dlopen(new URL(\"{path}\", import.meta.url), {{\n");
    for function in functions {
        let mut parameters = function
            .parameters
            .iter()
            .map(|parameter| format!("\"{}\"", deno_type(parameter.scalar)))
            .collect::<Vec<_>>();
        if !matches!(function.result, Scalar::Void) {
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
    output.push_str("});\n\nfunction checkStatus(status, name) {\n  if (status !== 0) throw new Error(`eqts call ${name} failed with ABI status ${status}`);\n}\n\n");
    for function in functions {
        render_wrapper(&mut output, function, Runtime::Deno);
    }
    output
}

#[derive(Clone, Copy)]
enum Runtime {
    Koffi,
    Bun,
    Deno,
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
        .map(|parameter| js_input(&parameter.name, parameter.scalar))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(output, "export function {}({parameters}) {{", function.name)
        .expect("writing to a string cannot fail");
    if matches!(function.result, Scalar::Void) {
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
            output_storage(runtime, function.result)
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
            js_output("output[0]", function.result)
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

fn ts_type(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Void => "void",
        Scalar::Bool => "boolean",
        Scalar::U64 | Scalar::I64 => "bigint",
        Scalar::U8
        | Scalar::U16
        | Scalar::U32
        | Scalar::I8
        | Scalar::I16
        | Scalar::I32
        | Scalar::F32
        | Scalar::F64 => "number",
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
            parameters: vec![
                Parameter {
                    name: "a".to_string(),
                    scalar: Scalar::U32,
                },
                Parameter {
                    name: "b".to_string(),
                    scalar: Scalar::U32,
                },
            ],
            result: Scalar::U32,
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
        function.parameters[0].scalar = Scalar::Void;
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
                Target::Wasm
            ]
        );
    }

    #[test]
    fn individual_compiled_targets_are_selected() {
        assert_eq!(selected_targets(Target::NodeNapi), [Target::NodeNapi]);
        assert_eq!(selected_targets(Target::Wasm), [Target::Wasm]);
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
        let error = parse_metadata(br#"{"schema_version":2,"functions":[]}"#)
            .expect_err("unknown schema must fail");
        assert!(error.to_string().contains("schema version 2"));
    }

    #[test]
    fn bool_and_bigint_wrappers_convert_values() {
        let function = Function {
            module: "fixture".to_string(),
            name: "toggle".to_string(),
            symbol: "custom_toggle".to_string(),
            parameters: vec![Parameter {
                name: "enabled".to_string(),
                scalar: Scalar::Bool,
            }],
            result: Scalar::I64,
        };
        let loader = render_bun("./libmath.dylib", &[function]);
        assert!(loader.contains("custom_toggle"));
        assert!(loader.contains("Number(enabled), ptr(output)"));
        assert!(loader.contains("new BigInt64Array(1)"));
    }
}
