//! Deterministic scalar-only Core Wasm bindings for a Wasmtime 45 host.
//!
//! This is deliberately separate from the Wasmer generators. It never imports a
//! support runtime and does not describe values with FatPtr, MessagePack, or an
//! async protocol.

use crate::{
    functions::{Function, FunctionList},
    generators::WasmtimeCoreWasmError,
    primitives::Primitive,
    types::{Type, TypeIdent, TypeMap},
};
use std::{fs, path::Path};

const ABI_MODULE: &str = "kernal-api:v1";
const ABI_VERSION: u8 = 1;
const GUEST_IMPORTS_FILE: &str = "guest_imports.rs";
const HOST_LINKER_FILE: &str = "wasmtime45_host_linker.rs";
const MANIFEST_FILE: &str = "kernal-api-v1.abi";

struct RenderedBindings {
    guest_imports: String,
    host_linker: String,
    manifest: String,
}

pub(crate) fn generate_bindings(
    import_functions: FunctionList,
    export_functions: FunctionList,
    types: TypeMap,
    path: &str,
) -> Result<(), WasmtimeCoreWasmError> {
    let rendered = render_bindings(&import_functions, &export_functions, &types)?;
    let output = Path::new(path);

    fs::create_dir_all(output).map_err(|source| WasmtimeCoreWasmError::Io {
        path: output.display().to_string(),
        source,
    })?;
    write_file(output.join(GUEST_IMPORTS_FILE), rendered.guest_imports)?;
    write_file(output.join(HOST_LINKER_FILE), rendered.host_linker)?;
    write_file(output.join(MANIFEST_FILE), rendered.manifest)?;

    Ok(())
}

fn write_file(path: impl AsRef<Path>, contents: String) -> Result<(), WasmtimeCoreWasmError> {
    let path = path.as_ref();
    fs::write(path, contents).map_err(|source| WasmtimeCoreWasmError::Io {
        path: path.display().to_string(),
        source,
    })
}

fn render_bindings(
    import_functions: &FunctionList,
    export_functions: &FunctionList,
    types: &TypeMap,
) -> Result<RenderedBindings, WasmtimeCoreWasmError> {
    validate_functions(import_functions, "import")?;
    validate_functions(export_functions, "export")?;
    validate_type_definitions(types)?;

    Ok(RenderedBindings {
        guest_imports: render_guest_imports(import_functions),
        host_linker: render_host_linker(import_functions),
        manifest: render_manifest(import_functions, export_functions),
    })
}

fn validate_functions(
    functions: &FunctionList,
    direction: &'static str,
) -> Result<(), WasmtimeCoreWasmError> {
    for function in functions {
        if function.is_async {
            return Err(WasmtimeCoreWasmError::AsyncFunction {
                direction,
                function: function.name.clone(),
            });
        }

        for argument in &function.args {
            lower_value_type(
                &argument.ty,
                direction,
                &function.name,
                format!("argument `{}`", argument.name),
            )?;
        }

        if let Some(return_type) = &function.return_type {
            lower_return_type(return_type, direction, &function.name)?;
        }
    }

    Ok(())
}

fn validate_type_definitions(types: &TypeMap) -> Result<(), WasmtimeCoreWasmError> {
    for (ident, ty) in types {
        match ty {
            Type::Primitive(primitive) if lower_primitive(*primitive).is_some() => {}
            Type::Unit => {}
            _ => {
                return Err(WasmtimeCoreWasmError::UnsupportedTypeDefinition {
                    name: ident.to_string(),
                    ty: ty.name(),
                });
            }
        }
    }

    Ok(())
}

fn lower_value_type(
    ty: &TypeIdent,
    direction: &'static str,
    function: &str,
    position: String,
) -> Result<&'static str, WasmtimeCoreWasmError> {
    if ty.is_array() || !ty.generic_args.is_empty() || ty.name == "()" {
        return Err(unsupported_value(direction, function, position, ty));
    }

    ty.as_primitive()
        .and_then(lower_primitive)
        .ok_or_else(|| unsupported_value(direction, function, position, ty))
}

fn lower_return_type(
    ty: &TypeIdent,
    direction: &'static str,
    function: &str,
) -> Result<Option<&'static str>, WasmtimeCoreWasmError> {
    if ty.name == "()" && !ty.is_array() && ty.generic_args.is_empty() {
        return Ok(None);
    }

    lower_value_type(ty, direction, function, "return".to_owned()).map(Some)
}

fn unsupported_value(
    direction: &'static str,
    function: &str,
    position: String,
    ty: &TypeIdent,
) -> WasmtimeCoreWasmError {
    WasmtimeCoreWasmError::UnsupportedValue {
        direction,
        function: function.to_owned(),
        position,
        ty: ty.to_string(),
    }
}

fn lower_primitive(primitive: Primitive) -> Option<&'static str> {
    match primitive {
        Primitive::Bool
        | Primitive::I8
        | Primitive::I16
        | Primitive::I32
        | Primitive::U8
        | Primitive::U16
        | Primitive::U32 => Some("i32"),
        Primitive::I64 | Primitive::U64 => Some("i64"),
        Primitive::F32 => Some("f32"),
        Primitive::F64 => Some("f64"),
    }
}

fn render_guest_imports(import_functions: &FunctionList) -> String {
    let declarations = import_functions
        .iter()
        .map(render_guest_import)
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        "// Generated by fp-bindgen: scalar Core Wasm ABI v{ABI_VERSION}.\n\
         // Module namespace: `{ABI_MODULE}`.\n\
         // Rust bool/u8/u16/i8/i16/i32/u32 lower to i32; i64/u64 lower to i64.\n\n\
         #[link(wasm_import_module = \"{ABI_MODULE}\")]\n\
         extern \"C\" {{\n\
         {declarations}\n\
         }}\n"
    )
}

fn render_guest_import(function: &Function) -> String {
    let arguments = function
        .args
        .iter()
        .map(|argument| format!("{}: {}", argument.name, lower_value_type_unchecked(&argument.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    let return_type = function
        .return_type
        .as_ref()
        .and_then(lower_return_type_unchecked)
        .map(|ty| format!(" -> {ty}"))
        .unwrap_or_default();

    format!(
        "    #[link_name = \"{}\"]\n    pub fn {}({arguments}){return_type};",
        function.name, function.name
    )
}

fn render_host_linker(import_functions: &FunctionList) -> String {
    let trait_methods = import_functions
        .iter()
        .map(render_host_trait_method)
        .collect::<Vec<_>>()
        .join("\n");
    let registrations = import_functions
        .iter()
        .map(render_host_registration)
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "// Generated private host/linker glue for Wasmtime 45.\n\
         // Keep this module private to the host crate; it exposes no runtime through fp-bindgen.\n\n\
         pub(crate) trait KernalApiV1Imports {{\n\
         {trait_methods}\n\
         }}\n\n\
         pub(crate) fn link_kernal_api_v1<T>(linker: &mut wasmtime::Linker<T>) -> wasmtime::Result<()>\n\
         where\n\
         \u{20}   T: KernalApiV1Imports + Send,\n\
         {{\n\
         {registrations}\n\
         \u{20}   Ok(())\n\
         }}\n"
    )
}

fn render_host_trait_method(function: &Function) -> String {
    let arguments = function
        .args
        .iter()
        .map(|argument| {
            format!(
                ", {}: {}",
                argument.name,
                lower_value_type_unchecked(&argument.ty)
            )
        })
        .collect::<String>();
    let return_type = function
        .return_type
        .as_ref()
        .and_then(lower_return_type_unchecked)
        .map(|ty| format!(" -> {ty}"))
        .unwrap_or_default();

    format!("    fn {}(&mut self{arguments}){return_type};", function.name)
}

fn render_host_registration(function: &Function) -> String {
    let arguments = function
        .args
        .iter()
        .map(|argument| format!("{}: {}", argument.name, lower_value_type_unchecked(&argument.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    let argument_names = function
        .args
        .iter()
        .map(|argument| argument.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "    linker.func_wrap(\"{ABI_MODULE}\", \"{}\", |mut caller: wasmtime::Caller<'_, T>{comma}{arguments}| {{\n\
         \u{20}       caller.data_mut().{}({argument_names})\n\
         \u{20}   }})?;",
        function.name,
        function.name,
        comma = if arguments.is_empty() { "" } else { ", " },
    )
}

fn render_manifest(import_functions: &FunctionList, export_functions: &FunctionList) -> String {
    let imports = import_functions
        .iter()
        .map(|function| render_manifest_function("import", function))
        .collect::<Vec<_>>()
        .join("\n");
    let exports = export_functions
        .iter()
        .map(|function| render_manifest_function("export", function))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "abi = \"{ABI_MODULE}\"\n\
         version = {ABI_VERSION}\n\
         lowering.bool = \"i32\"\n\
         lowering.i8 = \"i32\"\n\
         lowering.i16 = \"i32\"\n\
         lowering.i32 = \"i32\"\n\
         lowering.u8 = \"i32\"\n\
         lowering.u16 = \"i32\"\n\
         lowering.u32 = \"i32\"\n\
         lowering.i64 = \"i64\"\n\
         lowering.u64 = \"i64\"\n\
         lowering.f32 = \"f32\"\n\
         lowering.f64 = \"f64\"\n\
         lowering.unit = \"none\"\n\n\
         {imports}\n\
         {exports}\n"
    )
}

fn render_manifest_function(direction: &str, function: &Function) -> String {
    let arguments = function
        .args
        .iter()
        .map(|argument| lower_value_type_unchecked(&argument.ty))
        .collect::<Vec<_>>()
        .join(",");
    let return_type = function
        .return_type
        .as_ref()
        .and_then(lower_return_type_unchecked)
        .unwrap_or("unit");

    format!("{direction} {}({arguments}) -> {return_type}", function.name)
}

fn lower_value_type_unchecked(ty: &TypeIdent) -> &'static str {
    ty.as_primitive()
        .and_then(lower_primitive)
        .expect("Core Wasm declarations are validated before rendering")
}

fn lower_return_type_unchecked(ty: &TypeIdent) -> Option<&'static str> {
    if ty.name == "()" && !ty.is_array() && ty.generic_args.is_empty() {
        None
    } else {
        Some(lower_value_type_unchecked(ty))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar_functions() -> (FunctionList, FunctionList) {
        let mut imports = FunctionList::new();
        imports.add_function("fn zeta(flag: bool, count: i32, total: u64, ratio: f32, precise: f64);");
        imports.add_function("fn alpha() -> bool;");

        let mut exports = FunctionList::new();
        exports.add_function("fn guest_value(value: u64) -> f64;");
        (imports, exports)
    }

    #[test]
    fn scalar_fixture_renders_canonical_core_wasm_files() {
        let (imports, exports) = scalar_functions();
        let rendered = render_bindings(&imports, &exports, &TypeMap::new()).unwrap();

        assert_eq!(
            rendered.guest_imports,
            r#"// Generated by fp-bindgen: scalar Core Wasm ABI v1.
// Module namespace: `kernal-api:v1`.
// Rust bool/u8/u16/i8/i16/i32/u32 lower to i32; i64/u64 lower to i64.

#[link(wasm_import_module = "kernal-api:v1")]
extern "C" {
    #[link_name = "alpha"]
    pub fn alpha() -> i32;

    #[link_name = "zeta"]
    pub fn zeta(flag: i32, count: i32, total: i64, ratio: f32, precise: f64);
}
"#
        );
        assert!(rendered.manifest.contains("import alpha() -> i32"));
        assert!(rendered.manifest.contains("import zeta(i32,i32,i64,f32,f64) -> unit"));
        assert!(rendered.manifest.contains("export guest_value(i64) -> f64"));

        for legacy_concept in ["FatPtr", "MessagePack", "Wasmer", "Tokio", "rmp"] {
            assert!(!rendered.guest_imports.contains(legacy_concept));
            assert!(!rendered.host_linker.contains(legacy_concept));
            assert!(!rendered.manifest.contains(legacy_concept));
        }
    }

    #[test]
    fn declaration_order_does_not_change_rendered_abi() {
        let (imports, exports) = scalar_functions();
        let first = render_bindings(&imports, &exports, &TypeMap::new()).unwrap();

        let mut reordered_imports = FunctionList::new();
        reordered_imports.add_function("fn alpha() -> bool;");
        reordered_imports.add_function(
            "fn zeta(flag: bool, count: i32, total: u64, ratio: f32, precise: f64);",
        );
        let mut reordered_exports = FunctionList::new();
        reordered_exports.add_function("fn guest_value(value: u64) -> f64;");
        let second = render_bindings(&reordered_imports, &reordered_exports, &TypeMap::new()).unwrap();

        assert_eq!(first.guest_imports, second.guest_imports);
        assert_eq!(first.host_linker, second.host_linker);
        assert_eq!(first.manifest, second.manifest);
    }

    #[test]
    fn non_scalar_and_async_values_are_rejected_before_rendering() {
        for declaration in [
            "fn takes_string(value: String);",
            "fn takes_array(value: [u8; 4]);",
            "fn takes_list(value: Vec<u8>);",
            "fn takes_result(value: Result<u32, u32>);",
            "fn takes_struct(value: Payload);",
        ] {
            let mut imports = FunctionList::new();
            imports.add_function(declaration);
            assert!(matches!(
                render_bindings(&imports, &FunctionList::new(), &TypeMap::new()),
                Err(WasmtimeCoreWasmError::UnsupportedValue { .. })
            ));
        }

        let mut async_export = FunctionList::new();
        async_export.add_function("async fn later() -> i32;");
        assert!(matches!(
            render_bindings(&FunctionList::new(), &async_export, &TypeMap::new()),
            Err(WasmtimeCoreWasmError::AsyncFunction { .. })
        ));

        let mut types = TypeMap::new();
        types.insert(TypeIdent::from("Payload"), Type::String);
        assert!(matches!(
            render_bindings(&FunctionList::new(), &FunctionList::new(), &types),
            Err(WasmtimeCoreWasmError::UnsupportedTypeDefinition { .. })
        ));
    }
}
