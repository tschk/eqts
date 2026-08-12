use std::ffi::{CStr, c_char};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use cargo_metadata::{CrateType, Metadata, MetadataCommand, Package};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;

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

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Target {
    NodeKoffi,
    Bun,
    Deno,
}

#[derive(Debug, Deserialize)]
struct Function {
    name: String,
    parameters: Vec<Parameter>,
    result: Scalar,
}

#[derive(Debug, Deserialize)]
struct Parameter {
    name: String,
    scalar: Scalar,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Scalar {
    Void,
    U8,
    U16,
    U32,
    I8,
    I16,
    I32,
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
    let metadata = MetadataCommand::new().exec()?;
    let package = library_package(&metadata)?;
    cargo_build(package, release)?;
    let library = library_path(&metadata, package, release)?;
    let functions = load_metadata(&library)?;
    let target_dir = out_dir.join(target_name(target));
    fs::create_dir_all(&target_dir)?;
    fs::copy(
        &library,
        target_dir.join(library.file_name().context("library has no filename")?),
    )?;
    fs::write(
        target_dir.join("index.ts"),
        render_loader(target, &library, &functions),
    )?;
    fs::write(
        target_dir.join("index.d.ts"),
        render_declarations(&functions),
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
    Ok(metadata
        .target_directory
        .join(profile)
        .join(filename)
        .into_std_path_buf())
}

fn load_metadata(library_path: &Path) -> Result<Vec<Function>> {
    type MetadataFn = unsafe extern "C" fn() -> *const c_char;
    unsafe {
        let library = libloading::Library::new(library_path)
            .with_context(|| format!("failed to load {}", library_path.display()))?;
        let metadata: libloading::Symbol<MetadataFn> = library
            .get(b"eqts_metadata_json")
            .context("eqts::setup!() metadata symbol is missing")?;
        let pointer = metadata();
        if pointer.is_null() {
            bail!("eqts metadata symbol returned a null pointer");
        }
        let json = CStr::from_ptr(pointer)
            .to_str()
            .context("eqts metadata is not UTF-8")?;
        serde_json::from_str(json).context("eqts metadata is invalid JSON")
    }
}

fn target_name(target: Target) -> &'static str {
    match target {
        Target::NodeKoffi => "node-koffi",
        Target::Bun => "bun",
        Target::Deno => "deno",
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

fn render_loader(target: Target, library: &Path, functions: &[Function]) -> String {
    let path = format!(
        "./{}",
        library
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("library")
    );
    match target {
        Target::NodeKoffi => render_koffi(&path, functions),
        Target::Bun => render_bun(&path, functions),
        Target::Deno => render_deno(&path, functions),
    }
}

fn render_koffi(path: &str, functions: &[Function]) -> String {
    let mut output = format!(
        "import koffi from \"koffi\";\n\nconst library = koffi.load(new URL(\"{path}\", import.meta.url).pathname);\n\n"
    );
    for function in functions {
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| c_type(parameter.scalar))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "export const {} = library.func(\"eqts_{}\", \"{}\", [{}]);",
            function.name,
            function.name,
            c_type(function.result),
            parameters
                .split(", ")
                .filter(|value| !value.is_empty())
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .expect("writing to a string cannot fail");
    }
    output
}

fn render_bun(path: &str, functions: &[Function]) -> String {
    let mut output = "import { dlopen, FFIType } from \"bun:ffi\";\n\n".to_string();
    writeln!(
        output,
        "const {{ symbols }} = dlopen(new URL(\"{path}\", import.meta.url).pathname, {{"
    )
    .expect("writing to a string cannot fail");
    for function in functions {
        let args = function
            .parameters
            .iter()
            .map(|parameter| bun_type(parameter.scalar))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "  eqts_{}: {{ args: [{}], returns: {} }},",
            function.name,
            args,
            bun_type(function.result)
        )
        .expect("writing to a string cannot fail");
    }
    output.push_str("});\n\n");
    for function in functions {
        writeln!(
            output,
            "export const {} = symbols.eqts_{};",
            function.name, function.name
        )
        .expect("writing to a string cannot fail");
    }
    output
}

fn render_deno(path: &str, functions: &[Function]) -> String {
    let mut output = format!(
        "const {{ symbols }} = Deno.dlopen(new URL(\"{path}\", import.meta.url).pathname, {{\n"
    );
    for function in functions {
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| format!("\"{}\"", deno_type(parameter.scalar)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "  eqts_{}: {{ parameters: [{}], result: \"{}\" }},",
            function.name,
            parameters,
            deno_type(function.result)
        )
        .expect("writing to a string cannot fail");
    }
    output.push_str("});\n\n");
    for function in functions {
        writeln!(
            output,
            "export const {} = symbols.eqts_{};",
            function.name, function.name
        )
        .expect("writing to a string cannot fail");
    }
    output
}

fn ts_type(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Void => "void",
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
        Scalar::U8 => "uint8_t",
        Scalar::U16 => "uint16_t",
        Scalar::U32 => "uint32_t",
        Scalar::I8 => "int8_t",
        Scalar::I16 => "int16_t",
        Scalar::I32 => "int32_t",
        Scalar::F32 => "float",
        Scalar::F64 => "double",
    }
}

fn bun_type(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Void => "FFIType.void",
        Scalar::U8 => "FFIType.u8",
        Scalar::U16 => "FFIType.u16",
        Scalar::U32 => "FFIType.u32",
        Scalar::I8 => "FFIType.i8",
        Scalar::I16 => "FFIType.i16",
        Scalar::I32 => "FFIType.i32",
        Scalar::F32 => "FFIType.f32",
        Scalar::F64 => "FFIType.f64",
    }
}

fn deno_type(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Void => "void",
        Scalar::U8 => "u8",
        Scalar::U16 => "u16",
        Scalar::U32 => "u32",
        Scalar::I8 => "i8",
        Scalar::I16 => "i16",
        Scalar::I32 => "i32",
        Scalar::F32 => "f32",
        Scalar::F64 => "f64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_function() -> Function {
        Function {
            name: "add".to_string(),
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
        assert!(render_bun("./libmath.dylib", &[add_function()]).contains("eqts_add"));
    }

    #[test]
    fn deno_loader_uses_exported_symbol() {
        assert!(render_deno("./libmath.dylib", &[add_function()]).contains("eqts_add"));
    }

    #[test]
    fn koffi_loader_uses_exported_symbol() {
        assert!(render_koffi("./libmath.dylib", &[add_function()]).contains("eqts_add"));
    }
}
