//! Deterministic scalar and opaque-resource Core Wasm bindings for a Wasmtime 45 host.
//!
//! This target is intentionally independent from the legacy Wasmer generators.
//! It emits source text only and never selects an allocation, serialization, or
//! async transport protocol, except for explicit scalar operation handles on imports.

use crate::{
    functions::FunctionList,
    generators::WasmtimeCoreWasmError,
    primitives::Primitive,
    types::{Type, TypeIdent, TypeMap},
};
use inflector::Inflector;
use std::{collections::BTreeSet, fs, path::Path};

const ABI_NAMESPACE: &str = "kernal-api:v1";
const ABI_VERSION: u8 = 1;
const SCHEMA: &str = "fp-bindgen.core-wasm-abi";
const SCHEMA_REVISION: u8 = 1;
const GENERATOR_REVISION: u8 = 1;
const GUEST_CARGO_FILE: &str = "Cargo.toml";
const GUEST_SOURCE_FILE: &str = "src/lib.rs";
const HOST_LINKER_FILE: &str = "wasmtime45_host_linker.rs";
const MANIFEST_FILE: &str = "kernal-api-v1.abi.toml";
const OPERATION_CONTROLS: [&str; 4] = [
    "poll_operation",
    "take_operation_result",
    "yield_operation",
    "cancel_operation",
];

struct RenderedBindings {
    guest_cargo: String,
    guest_source: String,
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
    fs::create_dir_all(output.join("src")).map_err(|source| io_error(output, source))?;
    write_file(output.join(GUEST_CARGO_FILE), rendered.guest_cargo)?;
    write_file(output.join(GUEST_SOURCE_FILE), rendered.guest_source)?;
    write_file(output.join(HOST_LINKER_FILE), rendered.host_linker)?;
    write_file(output.join(MANIFEST_FILE), rendered.manifest)?;
    Ok(())
}

fn io_error(path: &Path, source: std::io::Error) -> WasmtimeCoreWasmError {
    WasmtimeCoreWasmError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn write_file(path: impl AsRef<Path>, contents: String) -> Result<(), WasmtimeCoreWasmError> {
    let path = path.as_ref();
    fs::write(path, contents).map_err(|source| io_error(path, source))
}

fn render_bindings(
    import_functions: &FunctionList,
    export_functions: &FunctionList,
    types: &TypeMap,
) -> Result<RenderedBindings, WasmtimeCoreWasmError> {
    let imports = lower_functions(import_functions, types, "import", true)?;
    let exports = lower_functions(export_functions, types, "export", false)?;
    validate_type_definitions(types)?;
    validate_owned_resource_release_names(types)?;
    for function in &exports {
        for ty in function
            .args
            .iter()
            .map(|argument| argument.ty)
            .chain(function.return_type)
        {
            if ty.owned_resource {
                return Err(WasmtimeCoreWasmError::OwnedResourceExport {
                    function: function.name.to_owned(),
                    resource: ty.semantic.to_owned(),
                });
            }
        }
    }
    Ok(RenderedBindings {
        guest_cargo: render_guest_cargo(),
        guest_source: render_guest_source(&imports, &exports, types),
        host_linker: render_host_linker(&imports, &exports, types),
        manifest: render_manifest(&imports, &exports, types),
    })
}

struct LoweredArgument<'a> {
    name: &'a str,
    ty: LoweredType<'a>,
}

struct LoweredFunction<'a> {
    name: &'a str,
    is_async: bool,
    args: Vec<LoweredArgument<'a>>,
    return_type: Option<LoweredType<'a>>,
}

fn lower_functions<'a>(
    functions: &'a FunctionList,
    types: &TypeMap,
    direction: &'static str,
    allow_async: bool,
) -> Result<Vec<LoweredFunction<'a>>, WasmtimeCoreWasmError> {
    let mut lowered = Vec::new();
    for function in functions {
        if function.is_async && !allow_async {
            return Err(WasmtimeCoreWasmError::AsyncFunction {
                direction,
                function: function.name.clone(),
            });
        }
        if OPERATION_CONTROLS.contains(&function.name.as_str())
            || owned_resource_release_names(types).contains(&function.name)
        {
            return Err(WasmtimeCoreWasmError::ReservedOperationControl {
                direction,
                function: function.name.clone(),
            });
        }
        let args = function
            .args
            .iter()
            .map(|argument| {
                Ok(LoweredArgument {
                    name: &argument.name,
                    ty: lower_value_type(
                        &argument.ty,
                        types,
                        direction,
                        &function.name,
                        format!("argument `{}`", argument.name),
                    )?,
                })
            })
            .collect::<Result<_, WasmtimeCoreWasmError>>()?;
        let return_type = function
            .return_type
            .as_ref()
            .map(|ty| lower_return_type(ty, types, direction, &function.name))
            .transpose()?
            .flatten();
        lowered.push(LoweredFunction {
            name: &function.name,
            is_async: function.is_async,
            args,
            return_type,
        });
    }
    Ok(lowered)
}

fn owned_resource_release_names(types: &TypeMap) -> BTreeSet<String> {
    types
        .values()
        .filter_map(|ty| match ty {
            Type::Resource(resource) if resource.is_owned() => {
                Some(resource_release_import_name(resource))
            }
            _ => None,
        })
        .collect()
}

fn validate_owned_resource_release_names(types: &TypeMap) -> Result<(), WasmtimeCoreWasmError> {
    let mut releases = std::collections::BTreeMap::new();
    for resource in types.values().filter_map(|ty| match ty {
        Type::Resource(resource) if resource.is_owned() => Some(resource),
        _ => None,
    }) {
        let release = resource_release_import_name(resource);
        if let Some(first_resource) = releases.insert(release.clone(), resource.ident.to_string()) {
            return Err(WasmtimeCoreWasmError::ResourceReleaseNameCollision {
                release,
                first_resource,
                second_resource: resource.ident.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_type_definitions(types: &TypeMap) -> Result<(), WasmtimeCoreWasmError> {
    for (ident, ty) in types {
        match ty {
            Type::Primitive(primitive) if lower_primitive(*primitive).is_some() => {}
            Type::Resource(resource) if resource.ident == *ident => {
                if resource.ident.is_array()
                    || !resource.ident.generic_args.is_empty()
                    || resource.ident.as_primitive().is_some()
                    || resource.ident.name.starts_with("r#")
                    || syn::parse_str::<syn::Ident>(&resource.ident.name).is_err()
                {
                    return Err(WasmtimeCoreWasmError::InvalidResourceName {
                        name: resource.ident.to_string(),
                    });
                }
            }
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

fn lower_value_type<'a>(
    ty: &'a TypeIdent,
    types: &TypeMap,
    direction: &'static str,
    function: &str,
    position: String,
) -> Result<LoweredType<'a>, WasmtimeCoreWasmError> {
    if ty.is_array() || !ty.generic_args.is_empty() || ty.name == "()" {
        return Err(unsupported_value(direction, function, position, ty));
    }
    if matches!(types.get(ty), Some(Type::Resource(_))) {
        let owned_resource = matches!(
            types.get(ty),
            Some(Type::Resource(resource)) if resource.is_owned()
        );
        return Ok(LoweredType {
            semantic: &ty.name,
            abi: "i64",
            resource: true,
            owned_resource,
        });
    }
    ty.as_primitive()
        .and_then(lower_primitive)
        .ok_or_else(|| unsupported_value(direction, function, position, ty))
}

fn lower_return_type<'a>(
    ty: &'a TypeIdent,
    types: &TypeMap,
    direction: &'static str,
    function: &str,
) -> Result<Option<LoweredType<'a>>, WasmtimeCoreWasmError> {
    if ty.name == "()" && !ty.is_array() && ty.generic_args.is_empty() {
        return Ok(None);
    }
    lower_value_type(ty, types, direction, function, "return".to_owned()).map(Some)
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

#[derive(Clone, Copy)]
struct LoweredType<'a> {
    semantic: &'a str,
    abi: &'static str,
    resource: bool,
    owned_resource: bool,
}

impl LoweredType<'_> {
    fn rust_type(self) -> String {
        if self.resource {
            format!("resources::{}", self.semantic)
        } else {
            self.semantic.to_owned()
        }
    }

    fn manifest(self) -> String {
        if self.resource {
            format!(
                "{{ semantic = \"{}\", abi = \"i64\", kind = \"resource\" }}",
                self.semantic
            )
        } else {
            format!(
                "{{ semantic = \"{}\", abi = \"{}\" }}",
                self.semantic, self.abi
            )
        }
    }
}

fn lower_primitive(primitive: Primitive) -> Option<LoweredType<'static>> {
    use Primitive::*;
    let (semantic, abi) = match primitive {
        Bool => ("bool", "i32"),
        I8 => ("i8", "i32"),
        I16 => ("i16", "i32"),
        I32 => ("i32", "i32"),
        U8 => ("u8", "i32"),
        U16 => ("u16", "i32"),
        U32 => ("u32", "i32"),
        I64 => ("i64", "i64"),
        U64 => ("u64", "i64"),
        F32 => ("f32", "f32"),
        F64 => ("f64", "f64"),
    };
    Some(LoweredType {
        semantic,
        abi,
        resource: false,
        owned_resource: false,
    })
}

fn render_guest_cargo() -> String {
    "[package]\n\
     name = \"kernal-api-v1-bindings\"\n\
     version = \"0.1.0\"\n\
     edition = \"2021\"\n\
     publish = false\n\n\
     [lib]\n\
     crate-type = [\"cdylib\", \"rlib\"]\n"
        .to_owned()
}

fn render_guest_source(
    import_functions: &[LoweredFunction<'_>],
    export_functions: &[LoweredFunction<'_>],
    types: &TypeMap,
) -> String {
    let has_async_imports = import_functions.iter().any(|function| function.is_async);
    let has_async_owned_borrow = import_functions.iter().any(|function| {
        function.is_async
            && function
                .args
                .iter()
                .any(|argument| argument.ty.owned_resource)
    });
    let raw_imports = import_functions
        .iter()
        .map(render_guest_raw_import)
        .collect::<Vec<_>>()
        .join("\n");
    let imports = import_functions
        .iter()
        .map(|function| render_guest_typed_import(function, has_async_owned_borrow))
        .collect::<Vec<_>>()
        .join("\n\n");
    let export_fields = export_functions
        .iter()
        .map(render_guest_export_field)
        .collect::<Vec<_>>()
        .join("\n");
    let export_wrappers = export_functions
        .iter()
        .map(render_guest_export_wrapper)
        .collect::<Vec<_>>()
        .join("\n\n");
    let operation_raw_imports = has_async_imports
        .then(render_guest_operation_raw_imports)
        .unwrap_or_default();
    let operation_support = has_async_imports
        .then(|| render_guest_operation_support(has_async_owned_borrow))
        .unwrap_or_default();
    let resource_release_raw_imports = render_guest_resource_release_raw_imports(types);
    let mut import_uses = if has_async_imports {
        "raw_imports, AbiError, PendingOperation"
    } else {
        "raw_imports, AbiError"
    }
    .to_owned();
    if types.values().any(|ty| matches!(ty, Type::Resource(_))) {
        import_uses.push_str(", resources");
    }
    let operation_error_variants = if has_async_imports {
        "            InvalidOperationState(i32),\n            OperationCancelled,\n"
    } else {
        ""
    };
    let resources = render_guest_resource_types(types);
    format!(
        "// Generated Core Wasm guest bindings for `{ABI_NAMESPACE}`.\n\
         // Public APIs use semantic scalar and resource types; raw ABI values stay private.\n\n\
         use std::sync::OnceLock;\n\n\
         #[derive(Clone, Debug, Eq, PartialEq)]\n\
         pub enum AbiError {{\n\
         \u{20}   InvalidBoolean(i32),\n\
         \u{20}   OutOfRange {{ ty: &'static str, value: i32 }},\n\
         \u{20}   ResourceClosed {{ resource: &'static str }},\n\
         \u{20}   ResourceReleaseRejected {{ resource: &'static str, status: i32 }},\n\
         {operation_error_variants}\
         }}\n\n\
         #[derive(Clone, Debug, Eq, PartialEq)]\n\
         pub enum ExportInstallError {{\n\
         \u{20}   AlreadyInstalled,\n\
         }}\n\n\
         {resources}\
         fn bool_from_i32(value: i32) -> Result<bool, AbiError> {{ match value {{ 0 => Ok(false), 1 => Ok(true), value => Err(AbiError::InvalidBoolean(value)) }} }}\n\
         fn i8_from_i32(value: i32) -> Result<i8, AbiError> {{ value.try_into().map_err(|_| AbiError::OutOfRange {{ ty: \"i8\", value }}) }}\n\
         fn i16_from_i32(value: i32) -> Result<i16, AbiError> {{ value.try_into().map_err(|_| AbiError::OutOfRange {{ ty: \"i16\", value }}) }}\n\
         fn u8_from_i32(value: i32) -> Result<u8, AbiError> {{ value.try_into().map_err(|_| AbiError::OutOfRange {{ ty: \"u8\", value }}) }}\n\
         fn u16_from_i32(value: i32) -> Result<u16, AbiError> {{ value.try_into().map_err(|_| AbiError::OutOfRange {{ ty: \"u16\", value }}) }}\n\n\
         fn i32_from_i32(value: i32) -> Result<i32, AbiError> {{ Ok(value) }}\n\
         fn u32_from_i32(value: i32) -> Result<u32, AbiError> {{ Ok(value as u32) }}\n\
         fn i64_from_i64(value: i64) -> Result<i64, AbiError> {{ Ok(value) }}\n\
         fn u64_from_i64(value: i64) -> Result<u64, AbiError> {{ Ok(value as u64) }}\n\
         fn f32_from_f32(value: f32) -> Result<f32, AbiError> {{ Ok(value) }}\n\
         fn f64_from_f64(value: f64) -> Result<f64, AbiError> {{ Ok(value) }}\n\
         fn bool_to_i32(value: bool) -> i32 {{ i32::from(value) }}\n\
         fn i8_to_i32(value: i8) -> i32 {{ value as i32 }}\n\
         fn i16_to_i32(value: i16) -> i32 {{ value as i32 }}\n\
         fn i32_to_i32(value: i32) -> i32 {{ value }}\n\
         fn u8_to_i32(value: u8) -> i32 {{ value as i32 }}\n\
         fn u16_to_i32(value: u16) -> i32 {{ value as i32 }}\n\
         fn u32_to_i32(value: u32) -> i32 {{ value as i32 }}\n\
         fn i64_to_i64(value: i64) -> i64 {{ value }}\n\
         fn u64_to_i64(value: u64) -> i64 {{ value as i64 }}\n\
         fn f32_to_f32(value: f32) -> f32 {{ value }}\n\
         fn f64_to_f64(value: f64) -> f64 {{ value }}\n\n\
         mod raw_imports {{\n\
         \u{20}   #[link(wasm_import_module = \"{ABI_NAMESPACE}\")]\n\
         \u{20}   extern \"C\" {{\n\
         {raw_imports}\n\
         {operation_raw_imports}\n\
         {resource_release_raw_imports}\n\
         \u{20}   }}\n\
         }}\n\n\
         {operation_support}\n\
         pub mod imports {{\n\
         \u{20}   use super::{{{import_uses}}};\n\n\
         {imports}\n\
         }}\n\n\
         pub struct KernalApiV1Exports {{\n\
         {export_fields}\n\
         }}\n\n\
         static EXPORTS: OnceLock<KernalApiV1Exports> = OnceLock::new();\n\n\
         pub fn install_exports(exports: KernalApiV1Exports) -> Result<(), ExportInstallError> {{ EXPORTS.set(exports).map_err(|_| ExportInstallError::AlreadyInstalled) }}\n\n\
         fn installed_exports() -> &'static KernalApiV1Exports {{ EXPORTS.get().expect(\"install KernalApiV1Exports before invoking guest exports\") }}\n\
         fn require_abi<T>(value: Result<T, AbiError>) -> T {{ value.expect(\"host passed an invalid scalar Core Wasm ABI value\") }}\n\n\
         {export_wrappers}\n"
    )
}

fn render_guest_resource_types(types: &TypeMap) -> String {
    let definitions = types
        .values()
        .filter_map(|ty| match ty {
            Type::Resource(resource) if resource.is_owned() => Some(format!(
                "#[derive(Debug, Eq, Hash, PartialEq)]\npub struct {}(::core::option::Option<u64>);\n\nimpl {} {{\n    pub(crate) fn from_abi(raw: u64) -> Self {{ Self(::core::option::Option::Some(raw)) }}\n    pub(crate) fn decode_i64(raw: i64) -> ::core::result::Result<Self, super::AbiError> {{ ::core::result::Result::Ok(Self::from_abi(raw as u64)) }}\n    pub(crate) fn encode_i64(&self) -> ::core::result::Result<i64, super::AbiError> {{ self.0.map(|raw| raw as i64).ok_or(super::AbiError::ResourceClosed {{ resource: \"{}\" }}) }}\n    pub fn close(mut self) -> ::core::result::Result<(), super::AbiError> {{ self.release() }}\n    fn release(&mut self) -> ::core::result::Result<(), super::AbiError> {{ let raw = self.0.take().ok_or(super::AbiError::ResourceClosed {{ resource: \"{}\" }})?; let status = unsafe {{ super::raw_imports::{}(raw as i64) }}; if status == 0 {{ ::core::result::Result::Ok(()) }} else {{ ::core::result::Result::Err(super::AbiError::ResourceReleaseRejected {{ resource: \"{}\", status }}) }} }}\n}}\n\nimpl ::core::ops::Drop for {} {{ fn drop(&mut self) {{ if self.0.is_some() {{ let _ = self.release(); }} }} }}\n\n",
                resource.ident,
                resource.ident,
                resource.ident,
                resource.ident,
                guest_resource_release_raw_name(resource),
                resource.ident,
                resource.ident,
            )),
            Type::Resource(resource) => Some(format!(
                "#[repr(transparent)]\n#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\npub struct {}(u64);\n\nimpl {} {{\n    pub(crate) fn from_abi(raw: u64) -> Self {{ Self(raw) }}\n    pub(crate) fn into_abi(self) -> u64 {{ self.0 }}\n    pub(crate) fn decode_i64(raw: i64) -> ::std::result::Result<Self, super::AbiError> {{ ::std::result::Result::Ok(Self::from_abi(raw as u64)) }}\n    pub(crate) fn encode_i64(self) -> i64 {{ self.into_abi() as i64 }}\n}}\n\n",
                resource.ident, resource.ident
            )),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if definitions.is_empty() {
        definitions
    } else {
        format!("pub mod resources {{\n{} }}\n\n", indent(&definitions, 4))
    }
}

fn resource_release_import_name(resource: &crate::types::Resource) -> String {
    format!("resource_release_{}", resource.ident.name.to_snake_case())
}

fn guest_resource_release_raw_name(resource: &crate::types::Resource) -> String {
    format!(
        "__kernal_api_v1_import_{}",
        resource_release_import_name(resource)
    )
}

fn render_guest_resource_release_raw_imports(types: &TypeMap) -> String {
    types
        .values()
        .filter_map(|ty| match ty {
            Type::Resource(resource) if resource.is_owned() => Some(format!(
                "        #[link_name = \"{}\"]\n        pub(super) fn {}(resource: i64) -> i32;\n",
                resource_release_import_name(resource),
                guest_resource_release_raw_name(resource),
            )),
            _ => None,
        })
        .collect()
}

fn render_guest_raw_import(function: &LoweredFunction<'_>) -> String {
    format!(
        "        #[link_name = \"{}\"]\n        pub(super) fn {}({}){};",
        function.name,
        raw_import_name(function),
        render_abi_arguments(function),
        if function.is_async {
            " -> i64".to_owned()
        } else {
            render_abi_result(function)
        }
    )
}

fn render_guest_typed_import(
    function: &LoweredFunction<'_>,
    has_async_owned_borrow: bool,
) -> String {
    let call_arguments = function
        .args
        .iter()
        .map(|argument| {
            if argument.ty.owned_resource {
                format!(
                    "super::resources::{}::encode_i64({})?",
                    argument.ty.semantic, argument.name
                )
            } else {
                format!("super::{}", encode_expression(argument.name, argument.ty))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let call = format!(
        "unsafe {{ raw_imports::{}({call_arguments}) }}",
        raw_import_name(function)
    );
    if function.is_async {
        let borrows_owned = function
            .args
            .iter()
            .any(|argument| argument.ty.owned_resource);
        let pending = if has_async_owned_borrow {
            if borrows_owned {
                format!("PendingOperation<'a, {}>", semantic_return(function))
            } else {
                format!("PendingOperation<'static, {}>", semantic_return(function))
            }
        } else {
            format!("PendingOperation<{}>", semantic_return(function))
        };
        return format!(
            "    pub fn {}{}({}) -> Result<{pending}, AbiError> {{\n        let operation = {call};\n        Ok(PendingOperation::new(operation as u64, {}))\n    }}",
            function.name,
            if borrows_owned { "<'a>" } else { "" },
            render_guest_semantic_arguments(function),
            render_operation_result_decoder(function),
        );
    }
    let body = match function.return_type {
        Some(lowered) => format!(
            "let raw = {call};\n        {}",
            format!("super::{}", decode_expression("raw", lowered))
        ),
        None => format!("{call};\n        Ok(())"),
    };
    format!(
        "    pub fn {}({}) -> Result<{}, AbiError> {{\n        {body}\n    }}",
        function.name,
        render_guest_semantic_arguments(function),
        semantic_return(function)
    )
}

fn render_guest_semantic_arguments(function: &LoweredFunction<'_>) -> String {
    function
        .args
        .iter()
        .map(|argument| {
            if argument.ty.owned_resource {
                let borrow = if function.is_async { "&'a " } else { "&" };
                format!(
                    "{}: {borrow}resources::{}",
                    argument.name, argument.ty.semantic
                )
            } else {
                format!("{}: {}", argument.name, argument.ty.rust_type())
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_guest_operation_raw_imports() -> String {
    "        #[link_name = \"poll_operation\"]\n        pub(super) fn __kernal_api_v1_import_poll_operation(operation: i64) -> i32;\n        #[link_name = \"take_operation_result\"]\n        pub(super) fn __kernal_api_v1_import_take_operation_result(operation: i64) -> i64;\n        #[link_name = \"yield_operation\"]\n        pub(super) fn __kernal_api_v1_import_yield_operation(operation: i64);\n        #[link_name = \"cancel_operation\"]\n        pub(super) fn __kernal_api_v1_import_cancel_operation(operation: i64);\n"
        .to_owned()
}

fn render_guest_operation_support(has_owned_borrows: bool) -> String {
    let struct_generics = if has_owned_borrows { "<'a, T>" } else { "<T>" };
    let impl_generics = if has_owned_borrows { "<'a, T>" } else { "<T>" };
    let type_generics = if has_owned_borrows { "<'a, T>" } else { "<T>" };
    let marker = if has_owned_borrows {
        "    _borrow: ::std::marker::PhantomData<&'a ()>,\n"
    } else {
        ""
    };
    let initialize_marker = if has_owned_borrows {
        ", _borrow: ::std::marker::PhantomData"
    } else {
        ""
    };
    format!(
        "pub struct PendingOperation{struct_generics} {{\n    operation: Option<u64>,\n    decode: fn(i64) -> Result<T, AbiError>,\n{marker}}}\n\nimpl{impl_generics} PendingOperation{type_generics} {{\n    fn new(operation: u64, decode: fn(i64) -> Result<T, AbiError>) -> Self {{ Self {{ operation: Some(operation), decode{initialize_marker} }} }}\n\n    pub fn poll(&mut self) -> Result<Option<T>, AbiError> {{\n        let operation = self.operation.ok_or(AbiError::OperationCancelled)?;\n        match unsafe {{ raw_imports::__kernal_api_v1_import_poll_operation(operation as i64) }} {{\n            0 => Ok(None),\n            1 => {{\n                self.operation = None;\n                (self.decode)(unsafe {{ raw_imports::__kernal_api_v1_import_take_operation_result(operation as i64) }}).map(Some)\n            }}\n            2 => {{ self.operation = None; Err(AbiError::OperationCancelled) }}\n            state => Err(AbiError::InvalidOperationState(state)),\n        }}\n    }}\n\n    pub fn yield_now(&self) -> Result<(), AbiError> {{\n        let operation = self.operation.ok_or(AbiError::OperationCancelled)?;\n        unsafe {{ raw_imports::__kernal_api_v1_import_yield_operation(operation as i64) }};\n        Ok(())\n    }}\n\n    pub fn cancel(&mut self) -> Result<(), AbiError> {{\n        let operation = self.operation.take().ok_or(AbiError::OperationCancelled)?;\n        unsafe {{ raw_imports::__kernal_api_v1_import_cancel_operation(operation as i64) }};\n        Ok(())\n    }}\n}}\n\nimpl{impl_generics} Drop for PendingOperation{type_generics} {{\n    fn drop(&mut self) {{\n        if let Some(operation) = self.operation.take() {{\n            unsafe {{ raw_imports::__kernal_api_v1_import_cancel_operation(operation as i64) }};\n        }}\n    }}\n}}\n\n"
    )
}

fn render_guest_export_field(function: &LoweredFunction<'_>) -> String {
    let arguments = function
        .args
        .iter()
        .map(|argument| argument.ty.rust_type())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "    pub {}: fn({arguments}) -> {},",
        function.name,
        semantic_return(function)
    )
}

fn render_guest_export_wrapper(function: &LoweredFunction<'_>) -> String {
    let decoded = function
        .args
        .iter()
        .map(|argument| {
            format!(
                "let {} = require_abi({});",
                argument.name,
                decode_expression(argument.name, argument.ty)
            )
        })
        .collect::<Vec<_>>()
        .join("\n    ");
    let names = function
        .args
        .iter()
        .map(|argument| argument.name)
        .collect::<Vec<_>>()
        .join(", ");
    let result = function
        .return_type
        .map(|lowered| format!("{}(result)", encode_function(lowered)))
        .unwrap_or_default();
    format!(
        "#[no_mangle]\npub extern \"C\" fn {}({}){} {{\n    {decoded}\n    let result = (installed_exports().{})({names});\n    {result}\n}}",
        function.name,
        render_abi_arguments(function),
        render_abi_result(function),
        function.name
    )
}

fn render_host_linker(
    import_functions: &[LoweredFunction<'_>],
    export_functions: &[LoweredFunction<'_>],
    types: &TypeMap,
) -> String {
    let has_async_imports = import_functions.iter().any(|function| function.is_async);
    let helpers = render_host_helpers(import_functions, export_functions);
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
    let operation_trait_methods = has_async_imports
        .then(render_host_operation_trait_methods)
        .unwrap_or_default();
    let operation_registrations = has_async_imports
        .then(render_host_operation_registrations)
        .unwrap_or_default();
    let resource_trait_methods = render_host_resource_release_trait_methods(types);
    let resource_registrations = render_host_resource_release_registrations(types);
    let invocations = export_functions
        .iter()
        .map(render_host_invocation)
        .collect::<Vec<_>>()
        .join("\n\n");
    let resources = render_host_resource_types(types);
    format!(
        "// Generated private Wasmtime 45 glue for Core Wasm ABI `{ABI_NAMESPACE}`.\n\
         // Host trait and invocation helpers use semantic scalar and resource types.\n\n\
         {resources}\
         {helpers}\n\n\
         pub(crate) trait KernalApiV1Imports {{\n{trait_methods}\n{operation_trait_methods}{resource_trait_methods}}}\n\n\
         pub(crate) fn link_kernal_api_v1<T>(linker: &mut wasmtime::Linker<T>) -> wasmtime::Result<()>\n\
         where\n\
         \u{20}   T: KernalApiV1Imports + Send + 'static,\n\
         {{\n{registrations}\n{operation_registrations}{resource_registrations}    Ok(())\n}}\n\n\
         {invocations}\n"
    )
}

fn render_host_resource_release_trait_methods(types: &TypeMap) -> String {
    types
        .values()
        .filter_map(|ty| match ty {
            Type::Resource(resource) if resource.is_owned() => Some(format!(
                "    /// Atomically revoke this guest-owned handle through the host's canonical scope/generation registry.\n    fn {}(&mut self, resource: resources::{}) -> wasmtime::Result<i32>;\n",
                resource_release_import_name(resource), resource.ident
            )),
            _ => None,
        })
        .collect()
}

fn render_host_resource_release_registrations(types: &TypeMap) -> String {
    types
        .values()
        .filter_map(|ty| match ty {
            Type::Resource(resource) if resource.is_owned() => Some(format!(
                "    linker.func_wrap(\n        \"{ABI_NAMESPACE}\",\n        \"{}\",\n        |mut caller: wasmtime::Caller<'_, T>, resource: i64| -> wasmtime::Result<i32> {{\n            caller.data_mut().{}(resources::{}::decode_i64(resource)?)\n        }},\n    )?;\n",
                resource_release_import_name(resource),
                resource_release_import_name(resource),
                resource.ident,
            )),
            _ => None,
        })
        .collect()
}

fn render_host_resource_types(types: &TypeMap) -> String {
    let definitions = types
        .values()
        .filter_map(|ty| match ty {
            Type::Resource(resource) => Some(format!(
                "#[repr(transparent)]\n#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\npub(crate) struct {}(pub(crate) u64);\n\nimpl {} {{\n    pub(crate) fn decode_i64(raw: i64) -> ::wasmtime::Result<Self> {{ ::std::result::Result::Ok(Self(raw as u64)) }}\n    pub(crate) fn encode_i64(self) -> i64 {{ self.0 as i64 }}\n}}\n\n",
                resource.ident, resource.ident
            )),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if definitions.is_empty() {
        definitions
    } else {
        format!(
            "pub(crate) mod resources {{\n{} }}\n\n",
            indent(&definitions, 4)
        )
    }
}

fn indent(source: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    source
        .lines()
        .map(|line| {
            if line.is_empty() {
                "\n".to_owned()
            } else {
                format!("{prefix}{line}\n")
            }
        })
        .collect()
}

fn render_host_helpers(
    import_functions: &[LoweredFunction<'_>],
    export_functions: &[LoweredFunction<'_>],
) -> String {
    let mut used = BTreeSet::new();
    for function in import_functions.iter().chain(export_functions.iter()) {
        for argument in &function.args {
            if !argument.ty.resource {
                used.insert(argument.ty.semantic);
            }
        }
        if let Some(lowered) = function.return_type {
            if !lowered.resource {
                used.insert(lowered.semantic);
            }
        }
    }
    if import_functions.iter().any(|function| function.is_async) {
        used.insert("u64");
    }
    [
        "bool", "i8", "i16", "i32", "u8", "u16", "u32", "i64", "u64", "f32", "f64",
    ]
    .iter()
    .filter(|semantic| used.contains(*semantic))
    .map(|semantic| host_helper_source(semantic))
    .collect::<Vec<_>>()
    .join("\n")
}

fn host_helper_source(semantic: &str) -> &'static str {
    match semantic {
        "bool" => "fn bool_from_i32(value: i32) -> wasmtime::Result<bool> {\n    match value {\n        0 => Ok(false),\n        1 => Ok(true),\n        value => Err(wasmtime::Error::msg(format!(\n            \"invalid bool ABI value: {value}\"\n        ))),\n    }\n}\nfn bool_to_i32(value: bool) -> i32 {\n    i32::from(value)\n}",
        "i8" => "fn i8_from_i32(value: i32) -> wasmtime::Result<i8> {\n    value\n        .try_into()\n        .map_err(|_| wasmtime::Error::msg(format!(\"invalid i8 ABI value: {value}\")))\n}\nfn i8_to_i32(value: i8) -> i32 {\n    value as i32\n}",
        "i16" => "fn i16_from_i32(value: i32) -> wasmtime::Result<i16> {\n    value\n        .try_into()\n        .map_err(|_| wasmtime::Error::msg(format!(\"invalid i16 ABI value: {value}\")))\n}\nfn i16_to_i32(value: i16) -> i32 {\n    value as i32\n}",
        "i32" => "fn i32_from_i32(value: i32) -> wasmtime::Result<i32> {\n    Ok(value)\n}\nfn i32_to_i32(value: i32) -> i32 {\n    value\n}",
        "u8" => "fn u8_from_i32(value: i32) -> wasmtime::Result<u8> {\n    value\n        .try_into()\n        .map_err(|_| wasmtime::Error::msg(format!(\"invalid u8 ABI value: {value}\")))\n}\nfn u8_to_i32(value: u8) -> i32 {\n    value as i32\n}",
        "u16" => "fn u16_from_i32(value: i32) -> wasmtime::Result<u16> {\n    value\n        .try_into()\n        .map_err(|_| wasmtime::Error::msg(format!(\"invalid u16 ABI value: {value}\")))\n}\nfn u16_to_i32(value: u16) -> i32 {\n    value as i32\n}",
        "u32" => "fn u32_from_i32(value: i32) -> wasmtime::Result<u32> {\n    Ok(value as u32)\n}\nfn u32_to_i32(value: u32) -> i32 {\n    value as i32\n}",
        "i64" => "fn i64_from_i64(value: i64) -> wasmtime::Result<i64> {\n    Ok(value)\n}\nfn i64_to_i64(value: i64) -> i64 {\n    value\n}",
        "u64" => "fn u64_from_i64(value: i64) -> wasmtime::Result<u64> {\n    Ok(value as u64)\n}\nfn u64_to_i64(value: u64) -> i64 {\n    value as i64\n}",
        "f32" => "fn f32_from_f32(value: f32) -> wasmtime::Result<f32> {\n    Ok(value)\n}\nfn f32_to_f32(value: f32) -> f32 {\n    value\n}",
        "f64" => "fn f64_from_f64(value: f64) -> wasmtime::Result<f64> {\n    Ok(value)\n}\nfn f64_to_f64(value: f64) -> f64 {\n    value\n}",
        _ => unreachable!("scalar Core Wasm lowering table is closed"),
    }
}

fn render_host_trait_method(function: &LoweredFunction<'_>) -> String {
    let arguments = render_semantic_arguments(function);
    let arguments = if arguments.is_empty() {
        String::new()
    } else {
        format!(", {arguments}")
    };
    format!(
        "    fn {}(&mut self{arguments}) -> wasmtime::Result<{}>;",
        function.name,
        if function.is_async {
            "u64".to_owned()
        } else {
            semantic_return(function)
        }
    )
}

fn render_host_operation_trait_methods() -> String {
    "    fn poll_operation(&mut self, operation: u64) -> wasmtime::Result<i32>;\n    fn take_operation_result(&mut self, operation: u64) -> wasmtime::Result<i64>;\n    fn yield_operation(&mut self, operation: u64) -> wasmtime::Result<()>;\n    fn cancel_operation(&mut self, operation: u64) -> wasmtime::Result<()>;\n"
        .to_owned()
}

fn render_host_registration(function: &LoweredFunction<'_>) -> String {
    let decoded = function
        .args
        .iter()
        .map(|argument| {
            format!(
                "let {} = {}?;",
                argument.name,
                decode_expression(argument.name, argument.ty)
            )
        })
        .collect::<Vec<_>>()
        .join("\n            ");
    let decoded = if decoded.is_empty() {
        String::new()
    } else {
        format!("{decoded}\n            ")
    };
    let names = function
        .args
        .iter()
        .map(|argument| argument.name)
        .collect::<Vec<_>>()
        .join(", ");
    let result = if function.is_async {
        format!("Ok(caller.data_mut().{}({names})? as i64)", function.name)
    } else {
        match function.return_type {
            Some(lowered) => format!(
                "Ok({}(caller.data_mut().{}({names})?))",
                encode_function(lowered),
                function.name
            ),
            None => format!(
                "caller.data_mut().{}({names})?;\n            Ok(())",
                function.name
            ),
        }
    };
    let abi_arguments = render_abi_arguments(function);
    let abi_arguments = if abi_arguments.is_empty() {
        String::new()
    } else {
        format!(", {abi_arguments}")
    };
    let closure = if abi_arguments.is_empty() {
        format!(
            "|mut caller: wasmtime::Caller<'_, T>| -> wasmtime::Result<{}> {{\n            {result}\n        }}",
            if function.is_async { "i64" } else { render_abi_result_only(function) }
        )
    } else {
        let arguments = abi_arguments
            .trim_start_matches(", ")
            .replace(", ", ",\n         ");
        format!(
            "|mut caller: wasmtime::Caller<'_, T>,\n         {arguments}|\n         -> wasmtime::Result<{}> {{\n            {decoded}{result}\n        }}",
            if function.is_async { "i64" } else { render_abi_result_only(function) }
        )
    };
    format!(
        "    linker.func_wrap(\n        \"{ABI_NAMESPACE}\",\n        \"{}\",\n        {closure},\n    )?;",
        function.name
    )
}

fn render_host_operation_registrations() -> String {
    format!(
        "    linker.func_wrap(\n        \"{ABI_NAMESPACE}\",\n        \"poll_operation\",\n        |mut caller: wasmtime::Caller<'_, T>, operation: i64| -> wasmtime::Result<i32> {{\n            caller.data_mut().poll_operation(operation as u64)\n        }},\n    )?;\n    linker.func_wrap(\n        \"{ABI_NAMESPACE}\",\n        \"take_operation_result\",\n        |mut caller: wasmtime::Caller<'_, T>, operation: i64| -> wasmtime::Result<i64> {{\n            caller.data_mut().take_operation_result(operation as u64)\n        }},\n    )?;\n    linker.func_wrap(\n        \"{ABI_NAMESPACE}\",\n        \"yield_operation\",\n        |mut caller: wasmtime::Caller<'_, T>, operation: i64| -> wasmtime::Result<()> {{\n            caller.data_mut().yield_operation(operation as u64)\n        }},\n    )?;\n    linker.func_wrap(\n        \"{ABI_NAMESPACE}\",\n        \"cancel_operation\",\n        |mut caller: wasmtime::Caller<'_, T>, operation: i64| -> wasmtime::Result<()> {{\n            caller.data_mut().cancel_operation(operation as u64)\n        }},\n    )?;\n"
    )
}

fn render_host_invocation(function: &LoweredFunction<'_>) -> String {
    let call_arguments = render_wasm_call_params(function);
    let result = match function.return_type {
        Some(lowered) => decode_expression("raw", lowered),
        None => "Ok(())".to_owned(),
    };
    let arguments = render_semantic_arguments(function);
    let arguments = if arguments.is_empty() {
        String::new()
    } else {
        format!(",\n    {}", arguments.replace(", ", ",\n    "))
    };
    format!(
        "pub(crate) fn invoke_{}<T>(\n    store: &mut wasmtime::Store<T>,\n    instance: &wasmtime::Instance{arguments},\n) -> wasmtime::Result<{}> {{\n    let function = instance.get_typed_func::<{}, {}>(&mut *store, \"{}\")?;\n    let raw = function.call(&mut *store, {call_arguments})?;\n    {result}\n}}",
        function.name,
        semantic_return(function),
        render_wasm_function_params(function),
        render_abi_result_only(function),
        function.name
    )
}

fn render_manifest(
    import_functions: &[LoweredFunction<'_>],
    export_functions: &[LoweredFunction<'_>],
    types: &TypeMap,
) -> String {
    let resources = types
        .values()
        .filter_map(|ty| match ty {
            Type::Resource(resource) => Some(format!(
                "[[resources]]\nname = \"{}\"\nkind = \"opaque_host_handle\"\nownership = \"{}\"\nabi = \"i64\"\n\n",
                resource.ident,
                if resource.is_stream() {
                    "stream"
                } else if resource.is_owned() {
                    "owned"
                } else {
                    "transport"
                },
            )),
            _ => None,
        })
        .collect::<String>();
    let imports = import_functions
        .iter()
        .map(|function| render_manifest_record("imports", "guest-to-host", function))
        .collect::<Vec<_>>()
        .join("\n");
    let exports = export_functions
        .iter()
        .map(|function| render_manifest_record("exports", "host-to-guest", function))
        .collect::<Vec<_>>()
        .join("\n");
    let operation_controls = if import_functions.iter().any(|function| function.is_async) {
        format!("\n{}", render_operation_control_manifest_records())
    } else {
        "\n".to_owned()
    };
    let resource_controls = render_resource_release_manifest_records(types);
    format!(
        "schema = \"{SCHEMA}\"\nschema_revision = {SCHEMA_REVISION}\ngenerator_revision = {GENERATOR_REVISION}\nabi_version = {ABI_VERSION}\nnamespace = \"{ABI_NAMESPACE}\"\n\n{resources}{imports}{operation_controls}{resource_controls}{exports}"
    )
}

fn render_resource_release_manifest_records(types: &TypeMap) -> String {
    types
        .values()
        .filter_map(|ty| match ty {
            Type::Resource(resource) if resource.is_owned() => Some(format!(
                "[[imports]]\nnamespace = \"{ABI_NAMESPACE}\"\nname = \"{}\"\ndirection = \"guest-to-host\"\ngenerated = true\nresource_control = \"release\"\nparams = [{{ semantic = \"{}\", abi = \"i64\", kind = \"resource\" }}]\nresults = [{{ semantic = \"i32\", abi = \"i32\" }}]\n\n",
                resource_release_import_name(resource), resource.ident
            )),
            _ => None,
        })
        .collect()
}

fn render_operation_control_manifest_records() -> String {
    [
        ("poll_operation", "i32"),
        ("take_operation_result", "i64"),
        ("yield_operation", "unit"),
        ("cancel_operation", "unit"),
    ]
    .iter()
    .map(|(name, result)| {
        let result = match *result {
            "i32" => "{ semantic = \"i32\", abi = \"i32\" }",
            "i64" => "{ semantic = \"i64\", abi = \"i64\" }",
            "unit" => "{ semantic = \"()\", abi = \"unit\" }",
            _ => unreachable!("closed operation control table"),
        };
        format!("[[imports]]\nnamespace = \"{ABI_NAMESPACE}\"\nname = \"{name}\"\ndirection = \"guest-to-host\"\ngenerated = true\nparams = [{{ semantic = \"u64\", abi = \"i64\" }}]\nresults = [{result}]\n\n")
    })
    .collect()
}

fn render_manifest_record(table: &str, direction: &str, function: &LoweredFunction<'_>) -> String {
    let params = function
        .args
        .iter()
        .map(|argument| argument.ty.manifest())
        .collect::<Vec<_>>()
        .join(", ");
    let results = if function.is_async {
        "{ semantic = \"operation\", abi = \"i64\" }".to_owned()
    } else {
        function
            .return_type
            .map(LoweredType::manifest)
            .unwrap_or_else(|| "{ semantic = \"()\", abi = \"unit\" }".to_owned())
    };
    let operation = if function.is_async {
        let result = function
            .return_type
            .map(LoweredType::manifest)
            .unwrap_or_else(|| "{ semantic = \"()\", abi = \"unit\" }".to_owned());
        format!("async = true\noperation_result = {result}\n")
    } else {
        String::new()
    };
    format!(
        "[[{table}]]\nnamespace = \"{ABI_NAMESPACE}\"\nname = \"{}\"\ndirection = \"{direction}\"\nparams = [{params}]\nresults = [{results}]\n{operation}",
        function.name
    )
}

fn render_operation_result_decoder(function: &LoweredFunction<'_>) -> String {
    match function.return_type {
        Some(lowered) if lowered.resource => {
            format!("|raw| super::{}", decode_expression("raw", lowered))
        }
        Some(lowered) => match lowered.semantic {
            "bool" => "|raw| super::bool_from_i32(raw as i32)".to_owned(),
            "i8" => "|raw| super::i8_from_i32(raw as i32)".to_owned(),
            "i16" => "|raw| super::i16_from_i32(raw as i32)".to_owned(),
            "i32" => "|raw| super::i32_from_i32(raw as i32)".to_owned(),
            "u8" => "|raw| super::u8_from_i32(raw as i32)".to_owned(),
            "u16" => "|raw| super::u16_from_i32(raw as i32)".to_owned(),
            "u32" => "|raw| super::u32_from_i32(raw as i32)".to_owned(),
            "i64" => "|raw| super::i64_from_i64(raw)".to_owned(),
            "u64" => "|raw| super::u64_from_i64(raw)".to_owned(),
            "f32" => "|raw| super::f32_from_f32(f32::from_bits(raw as u32))".to_owned(),
            "f64" => "|raw| super::f64_from_f64(f64::from_bits(raw as u64))".to_owned(),
            _ => unreachable!("closed lowering table"),
        },
        None => "|_| Ok(())".to_owned(),
    }
}

fn raw_import_name(function: &LoweredFunction<'_>) -> String {
    format!("__kernal_api_v1_import_{}", function.name)
}

fn render_semantic_arguments(function: &LoweredFunction<'_>) -> String {
    function
        .args
        .iter()
        .map(|argument| format!("{}: {}", argument.name, argument.ty.rust_type()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_abi_arguments(function: &LoweredFunction<'_>) -> String {
    render_abi_arguments_only(function)
}

fn render_abi_arguments_only(function: &LoweredFunction<'_>) -> String {
    function
        .args
        .iter()
        .map(|argument| format!("{}: {}", argument.name, argument.ty.abi))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_wasm_function_params(function: &LoweredFunction<'_>) -> String {
    let params = function
        .args
        .iter()
        .map(|argument| argument.ty.abi)
        .collect::<Vec<_>>();
    match params.as_slice() {
        [] => "()".to_owned(),
        [one] => (*one).to_owned(),
        many => format!("({})", many.join(", ")),
    }
}

fn render_wasm_call_params(function: &LoweredFunction<'_>) -> String {
    let params = function
        .args
        .iter()
        .map(|argument| encode_expression(argument.name, argument.ty))
        .collect::<Vec<_>>();
    match params.as_slice() {
        [] => "()".to_owned(),
        [one] => one.clone(),
        many => format!("({})", many.join(", ")),
    }
}

fn render_abi_result(function: &LoweredFunction<'_>) -> String {
    format!(" -> {}", render_abi_result_only(function))
}

fn render_abi_result_only(function: &LoweredFunction<'_>) -> &'static str {
    function
        .return_type
        .map(|lowered| lowered.abi)
        .unwrap_or("()")
}

fn semantic_return(function: &LoweredFunction<'_>) -> String {
    function
        .return_type
        .map(LoweredType::rust_type)
        .unwrap_or_else(|| "()".to_owned())
}

fn encode_expression(name: &str, lowered: LoweredType<'_>) -> String {
    format!("{}({name})", encode_function(lowered))
}

fn encode_function(lowered: LoweredType<'_>) -> String {
    if lowered.resource {
        return format!("resources::{}::encode_i64", lowered.semantic);
    }
    match lowered.semantic {
        "bool" => "bool_to_i32",
        "i8" => "i8_to_i32",
        "i16" => "i16_to_i32",
        "i32" => "i32_to_i32",
        "u8" => "u8_to_i32",
        "u16" => "u16_to_i32",
        "u32" => "u32_to_i32",
        "i64" => "i64_to_i64",
        "u64" => "u64_to_i64",
        "f32" => "f32_to_f32",
        "f64" => "f64_to_f64",
        _ => unreachable!("closed lowering table"),
    }
    .to_owned()
}

fn decode_expression(name: &str, lowered: LoweredType<'_>) -> String {
    if lowered.resource {
        return format!("resources::{}::decode_i64({name})", lowered.semantic);
    }
    let function = match lowered.semantic {
        "bool" => "bool_from_i32",
        "i8" => "i8_from_i32",
        "i16" => "i16_from_i32",
        "i32" => "i32_from_i32",
        "u8" => "u8_from_i32",
        "u16" => "u16_from_i32",
        "u32" => "u32_from_i32",
        "i64" => "i64_from_i64",
        "u64" => "u64_from_i64",
        "f32" => "f32_from_f32",
        "f64" => "f64_from_f64",
        _ => unreachable!("closed lowering table"),
    };
    format!("{function}({name})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar_functions() -> (FunctionList, FunctionList) {
        let mut imports = FunctionList::new();
        imports
            .add_function("fn zeta(flag: bool, count: i32, total: u64, ratio: f32, precise: f64);");
        imports.add_function("fn alpha() -> bool;");
        let mut exports = FunctionList::new();
        exports.add_function("fn guest_value(value: u64) -> f64;");
        (imports, exports)
    }

    #[test]
    fn scalar_fixture_is_typed_bidirectional_and_free_of_legacy_runtime_words() {
        let (imports, exports) = scalar_functions();
        let rendered = render_bindings(&imports, &exports, &TypeMap::new()).unwrap();
        assert!(rendered
            .guest_cargo
            .contains("crate-type = [\"cdylib\", \"rlib\"]"));
        assert!(rendered.guest_source.contains("pub fn zeta(flag: bool, count: i32, total: u64, ratio: f32, precise: f64) -> Result<(), AbiError>"));
        assert!(rendered
            .guest_source
            .contains("#[no_mangle]\npub extern \"C\" fn guest_value(value: i64) -> f64"));
        assert!(rendered.host_linker.contains("fn zeta(&mut self, flag: bool, count: i32, total: u64, ratio: f32, precise: f64) -> wasmtime::Result<()>"));
        assert!(rendered
            .host_linker
            .contains("T: KernalApiV1Imports + Send + 'static"));
        assert!(rendered.host_linker.contains("invoke_guest_value"));
        assert!(!rendered.guest_source.contains("OperationCancelled"));
        for legacy in ["FatPtr", "MessagePack", "Wasmer", "Tokio", "rmp"] {
            assert!(!rendered.guest_source.contains(legacy));
            assert!(!rendered.host_linker.contains(legacy));
            assert!(!rendered.manifest.contains(legacy));
        }
    }

    #[test]
    fn resource_declarations_render_nominal_opaque_handles_on_both_sides() {
        let mut types = TypeMap::new();
        types.insert(
            TypeIdent::from("Archive"),
            Type::Resource(crate::types::Resource::new("Archive")),
        );
        let rendered = render_bindings(&FunctionList::new(), &FunctionList::new(), &types).unwrap();

        assert!(rendered.guest_source.contains("pub mod resources {"));
        assert!(rendered.guest_source.contains("pub struct Archive(u64);"));
        assert!(rendered
            .guest_source
            .contains("pub(crate) fn from_abi(raw: u64) -> Self"));
        assert!(rendered.host_linker.contains("pub(crate) mod resources {"));
        assert!(rendered
            .host_linker
            .contains("pub(crate) struct Archive(pub(crate) u64);"));
        let manifest: toml::Value = rendered.manifest.parse().unwrap();
        assert_eq!(manifest["resources"][0]["name"].as_str(), Some("Archive"));
        assert_eq!(
            manifest["resources"][0]["kind"].as_str(),
            Some("opaque_host_handle")
        );
        assert_eq!(manifest["resources"][0]["abi"].as_str(), Some("i64"));
    }

    fn resource_types() -> TypeMap {
        ["Archive", "Entry"]
            .into_iter()
            .map(|name| {
                (
                    TypeIdent::from(name),
                    Type::Resource(crate::types::Resource::new(name)),
                )
            })
            .collect()
    }

    fn owned_resource_types() -> TypeMap {
        let mut types = resource_types();
        types.insert(
            TypeIdent::from("Archive"),
            Type::Resource(crate::types::Resource::owned("Archive")),
        );
        types
    }

    #[test]
    fn stream_declaration_is_an_owned_handle_with_distinct_manifest_policy() {
        let mut types = TypeMap::new();
        types.insert(
            TypeIdent::from("Input"),
            Type::Resource(crate::types::Resource::stream("Input")),
        );
        let rendered = render_bindings(&FunctionList::new(), &FunctionList::new(), &types).unwrap();
        assert!(rendered
            .guest_source
            .contains("pub struct Input(::core::option::Option<u64>)"));
        assert!(rendered.guest_source.contains("resource_release_input"));
        let manifest: toml::Value = rendered.manifest.parse().unwrap();
        assert_eq!(
            manifest["resources"][0]["ownership"].as_str(),
            Some("stream")
        );
        assert!(!rendered.guest_source.contains("Vec<u8>"));
        assert!(!rendered.host_linker.contains("MessagePack"));
    }

    #[test]
    fn resources_lower_to_nominal_signatures_and_i64_in_both_directions() {
        let mut imports = FunctionList::new();
        imports.add_function("fn open_entry(archive: Archive, index: u32) -> Entry;");
        let mut exports = FunctionList::new();
        exports.add_function("fn visit_entry(entry: Entry) -> Archive;");
        let rendered = render_bindings(&imports, &exports, &resource_types()).unwrap();
        syn::parse_file(&rendered.guest_source).unwrap();
        syn::parse_file(&rendered.host_linker).unwrap();
        assert!(rendered.guest_source.contains("pub fn open_entry(archive: resources::Archive, index: u32) -> Result<resources::Entry, AbiError>"));
        assert!(rendered
            .guest_source
            .contains("__kernal_api_v1_import_open_entry(archive: i64, index: i32) -> i64"));
        assert!(rendered
            .guest_source
            .contains("pub visit_entry: fn(resources::Entry) -> resources::Archive"));
        assert!(rendered
            .guest_source
            .contains("pub extern \"C\" fn visit_entry(entry: i64) -> i64"));
        assert!(rendered
            .guest_source
            .contains("super::resources::Archive::encode_i64(archive)"));
        assert!(rendered
            .guest_source
            .contains("super::resources::Entry::decode_i64(raw)"));
        assert!(rendered.host_linker.contains("fn open_entry(&mut self, archive: resources::Archive, index: u32) -> wasmtime::Result<resources::Entry>"));
        assert!(rendered
            .host_linker
            .contains("resources::Archive::decode_i64(archive)?"));
        assert!(rendered.host_linker.contains(
            "resources::Entry::encode_i64(caller.data_mut().open_entry(archive, index)?)"
        ));
        assert!(rendered.host_linker.contains("get_typed_func::<i64, i64>"));
        let manifest: toml::Value = rendered.manifest.parse().unwrap();
        for record in [&manifest["imports"][0], &manifest["exports"][0]] {
            for field in ["params", "results"] {
                assert_eq!(record[field][0]["kind"].as_str(), Some("resource"));
                assert_eq!(record[field][0]["abi"].as_str(), Some("i64"));
            }
        }
        assert_eq!(
            manifest["imports"][0]["params"][0]["semantic"].as_str(),
            Some("Archive")
        );
        assert_eq!(
            manifest["imports"][0]["results"][0]["semantic"].as_str(),
            Some("Entry")
        );
        assert_eq!(
            manifest["exports"][0]["params"][0]["semantic"].as_str(),
            Some("Entry")
        );
        assert_eq!(
            manifest["exports"][0]["results"][0]["semantic"].as_str(),
            Some("Archive")
        );
    }

    #[test]
    fn async_resource_import_preserves_typed_completion() {
        let mut imports = FunctionList::new();
        imports.add_function("async fn next_entry(archive: Archive) -> Entry;");
        let rendered = render_bindings(&imports, &FunctionList::new(), &resource_types()).unwrap();
        syn::parse_file(&rendered.guest_source).unwrap();
        syn::parse_file(&rendered.host_linker).unwrap();
        assert!(rendered.guest_source.contains("pub fn next_entry(archive: resources::Archive) -> Result<PendingOperation<resources::Entry>, AbiError>"));
        assert!(rendered
            .guest_source
            .contains("|raw| super::resources::Entry::decode_i64(raw)"));
        assert!(rendered.host_linker.contains(
            "fn next_entry(&mut self, archive: resources::Archive) -> wasmtime::Result<u64>"
        ));
        let manifest: toml::Value = rendered.manifest.parse().unwrap();
        assert_eq!(manifest["imports"][0]["async"].as_bool(), Some(true));
        assert_eq!(
            manifest["imports"][0]["results"][0]["semantic"].as_str(),
            Some("operation")
        );
        assert_eq!(
            manifest["imports"][0]["operation_result"]["semantic"].as_str(),
            Some("Entry")
        );
        assert_eq!(
            manifest["imports"][0]["operation_result"]["kind"].as_str(),
            Some("resource")
        );
        assert_eq!(
            manifest["imports"][0]["operation_result"]["abi"].as_str(),
            Some("i64")
        );
    }

    #[test]
    fn owned_resources_borrow_until_async_completion_and_emit_release_contract() {
        let mut imports = FunctionList::new();
        imports.add_function("async fn next_entry(archive: Archive) -> Entry;");
        let rendered =
            render_bindings(&imports, &FunctionList::new(), &owned_resource_types()).unwrap();
        syn::parse_file(&rendered.guest_source).unwrap();
        syn::parse_file(&rendered.host_linker).unwrap();
        assert!(rendered
            .guest_source
            .contains("pub struct Archive(::core::option::Option<u64>);"));
        assert!(rendered.guest_source.contains("pub fn close(mut self)"));
        assert!(rendered
            .guest_source
            .contains("impl ::core::ops::Drop for Archive"));
        assert!(rendered
            .guest_source
            .contains("pub struct PendingOperation<'a, T>"));
        assert!(rendered.guest_source.contains("pub fn next_entry<'a>(archive: &'a resources::Archive) -> Result<PendingOperation<'a, resources::Entry>, AbiError>"));
        assert!(rendered.guest_source.contains("resource_release_archive"));
        assert!(rendered.host_linker.contains("fn resource_release_archive(&mut self, resource: resources::Archive) -> wasmtime::Result<i32>;"));
        let manifest: toml::Value = rendered.manifest.parse().unwrap();
        assert_eq!(
            manifest["resources"][0]["ownership"].as_str(),
            Some("owned")
        );
        assert_eq!(
            manifest["imports"][5]["name"].as_str(),
            Some("resource_release_archive")
        );
        assert_eq!(
            manifest["imports"][5]["resource_control"].as_str(),
            Some("release")
        );
    }

    #[test]
    fn owned_resources_reject_implicit_export_transfer_and_control_collisions() {
        let mut exports = FunctionList::new();
        exports.add_function("fn pass(archive: Archive) -> Entry;");
        assert!(matches!(
            render_bindings(&FunctionList::new(), &exports, &owned_resource_types()),
            Err(WasmtimeCoreWasmError::OwnedResourceExport { .. })
        ));
        let mut imports = FunctionList::new();
        imports.add_function("fn resource_release_archive();");
        assert!(matches!(
            render_bindings(&imports, &FunctionList::new(), &owned_resource_types()),
            Err(WasmtimeCoreWasmError::ReservedOperationControl { .. })
        ));

        let mut colliding = TypeMap::new();
        for name in ["HTTPServer", "HttpServer"] {
            colliding.insert(
                TypeIdent::from(name),
                Type::Resource(crate::types::Resource::owned(name)),
            );
        }
        assert!(matches!(
            render_bindings(&FunctionList::new(), &FunctionList::new(), &colliding),
            Err(WasmtimeCoreWasmError::ResourceReleaseNameCollision { .. })
        ));
    }

    #[test]
    fn resource_signatures_require_declarations_and_reject_value_containers() {
        for declaration in [
            "fn unknown(value: Undeclared);",
            "fn unknown() -> Undeclared;",
            "async fn unknown() -> Undeclared;",
            "fn list(value: Vec<Archive>);",
            "fn generic(value: Archive<u64>);",
            "fn optional() -> Option<Archive>;",
            "fn array(value: [Archive; 2]);",
        ] {
            let mut imports = FunctionList::new();
            imports.add_function(declaration);
            assert!(
                matches!(
                    render_bindings(&imports, &FunctionList::new(), &resource_types()),
                    Err(WasmtimeCoreWasmError::UnsupportedValue { .. })
                ),
                "{declaration}"
            );
        }
        let mut exports = FunctionList::new();
        exports.add_function("async fn unsupported() -> Archive;");
        assert!(matches!(
            render_bindings(&FunctionList::new(), &exports, &resource_types()),
            Err(WasmtimeCoreWasmError::AsyncFunction { .. })
        ));
    }

    #[test]
    fn zero_length_resource_array_signatures_fail_before_lowering() {
        for declaration in [
            "fn empty(value: [Archive; 0]);",
            "fn empty() -> [Archive; 0];",
            "async fn empty() -> [Archive; 0];",
        ] {
            assert!(
                std::panic::catch_unwind(|| crate::functions::Function::new(declaration)).is_err(),
                "{declaration}"
            );
        }
    }

    #[test]
    fn wasmtime45_resource_fixture_is_exact_generated_host_golden() {
        let mut imports = FunctionList::new();
        imports.add_function("fn open_entry(archive: Archive) -> Entry;");
        let mut exports = FunctionList::new();
        exports.add_function("fn visit_entry(entry: Entry) -> Entry;");
        let rendered = render_bindings(&imports, &exports, &owned_resource_types()).unwrap();
        assert_eq!(
            rendered.host_linker,
            include_str!("../../tests/fixtures/wasmtime45_resource_host.rs")
        );
    }

    #[test]
    fn resource_declaration_must_match_its_type_map_identity() {
        let mut types = TypeMap::new();
        types.insert(
            TypeIdent::from("Archive"),
            Type::Resource(crate::types::Resource::new("Other")),
        );
        assert!(matches!(
            render_bindings(&FunctionList::new(), &FunctionList::new(), &types),
            Err(WasmtimeCoreWasmError::UnsupportedTypeDefinition { .. })
        ));
    }

    #[test]
    fn resource_declaration_is_isolated_from_generated_binding_names() {
        let mut types = TypeMap::new();
        types.insert(
            TypeIdent::from("OnceLock"),
            Type::Resource(crate::types::Resource::new("OnceLock")),
        );
        let rendered = render_bindings(&FunctionList::new(), &FunctionList::new(), &types).unwrap();
        assert!(rendered.guest_source.contains("pub struct OnceLock(u64);"));
    }

    #[test]
    fn resource_declaration_must_be_a_plain_rust_identifier() {
        let mut types = TypeMap::new();
        types.insert(
            TypeIdent::from("not-valid"),
            Type::Resource(crate::types::Resource::new("not-valid")),
        );
        assert!(matches!(
            render_bindings(&FunctionList::new(), &FunctionList::new(), &types),
            Err(WasmtimeCoreWasmError::InvalidResourceName { .. })
        ));
    }

    #[test]
    fn resource_declaration_cannot_shadow_or_alias_a_primitive_identifier() {
        for name in ["u64", "r#Archive"] {
            let mut types = TypeMap::new();
            types.insert(
                TypeIdent::from(name),
                Type::Resource(crate::types::Resource::new(name)),
            );
            assert!(matches!(
                render_bindings(&FunctionList::new(), &FunctionList::new(), &types),
                Err(WasmtimeCoreWasmError::InvalidResourceName { .. })
            ));
        }
    }

    #[test]
    fn manifest_is_parseable_toml_with_canonical_sorted_records() {
        let (imports, exports) = scalar_functions();
        let first = render_bindings(&imports, &exports, &TypeMap::new()).unwrap();
        let manifest: toml::Value = toml::from_str(&first.manifest).unwrap();
        assert_eq!(manifest["schema"].as_str(), Some(SCHEMA));
        assert_eq!(manifest["namespace"].as_str(), Some(ABI_NAMESPACE));
        assert_eq!(manifest["imports"][0]["name"].as_str(), Some("alpha"));
        assert_eq!(
            manifest["imports"][1]["params"][2]["semantic"].as_str(),
            Some("u64")
        );
        assert_eq!(
            first.manifest,
            r#"schema = "fp-bindgen.core-wasm-abi"
schema_revision = 1
generator_revision = 1
abi_version = 1
namespace = "kernal-api:v1"

[[imports]]
namespace = "kernal-api:v1"
name = "alpha"
direction = "guest-to-host"
params = []
results = [{ semantic = "bool", abi = "i32" }]

[[imports]]
namespace = "kernal-api:v1"
name = "zeta"
direction = "guest-to-host"
params = [{ semantic = "bool", abi = "i32" }, { semantic = "i32", abi = "i32" }, { semantic = "u64", abi = "i64" }, { semantic = "f32", abi = "f32" }, { semantic = "f64", abi = "f64" }]
results = [{ semantic = "()", abi = "unit" }]

[[exports]]
namespace = "kernal-api:v1"
name = "guest_value"
direction = "host-to-guest"
params = [{ semantic = "u64", abi = "i64" }]
results = [{ semantic = "f64", abi = "f64" }]
"#
        );
        let mut reordered = FunctionList::new();
        reordered.add_function("fn alpha() -> bool;");
        reordered
            .add_function("fn zeta(flag: bool, count: i32, total: u64, ratio: f32, precise: f64);");
        let second = render_bindings(&reordered, &exports, &TypeMap::new()).unwrap();
        assert_eq!(first.manifest, second.manifest);
        assert_eq!(first.guest_source, second.guest_source);
        assert_eq!(first.host_linker, second.host_linker);
    }

    #[test]
    fn invalid_bool_and_narrow_values_use_checked_decoder_paths() {
        let (imports, exports) = scalar_functions();
        let rendered = render_bindings(&imports, &exports, &TypeMap::new()).unwrap();
        assert!(rendered
            .guest_source
            .contains("match value { 0 => Ok(false), 1 => Ok(true)"));
        let mut narrow_imports = FunctionList::new();
        narrow_imports.add_function("fn narrow(value: u8) -> u16;");
        let narrow =
            render_bindings(&narrow_imports, &FunctionList::new(), &TypeMap::new()).unwrap();
        assert!(narrow
            .host_linker
            .contains("u16_from_i32(value: i32) -> wasmtime::Result<u16>"));
        assert!(rendered.guest_source.contains("u64_to_i64"));
        assert!(rendered.host_linker.contains("u64_from_i64"));
    }

    #[test]
    fn non_scalar_async_export_and_reserved_values_are_rejected_before_output() {
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
        let mut async_exports = FunctionList::new();
        async_exports.add_function("async fn later() -> i32;");
        assert!(matches!(
            render_bindings(&FunctionList::new(), &async_exports, &TypeMap::new()),
            Err(WasmtimeCoreWasmError::AsyncFunction { .. })
        ));
        let mut reserved_imports = FunctionList::new();
        reserved_imports.add_function("fn poll_operation();");
        assert!(matches!(
            render_bindings(&reserved_imports, &FunctionList::new(), &TypeMap::new()),
            Err(WasmtimeCoreWasmError::ReservedOperationControl { .. })
        ));
    }

    #[test]
    fn target_feature_is_dependency_free_and_default_isolated() {
        let cargo: toml::Value = include_str!("../../Cargo.toml").parse().unwrap();
        let features = cargo["features"].as_table().unwrap();
        let dependencies = cargo["dependencies"].as_table().unwrap();

        assert!(features["wasmtime-core-wasm"]
            .as_array()
            .unwrap()
            .is_empty());
        let integration = features["wasmtime45-integration"].as_array().unwrap();
        assert_eq!(integration.len(), 2);
        assert_eq!(integration[0].as_str(), Some("wasmtime-core-wasm"));
        assert_eq!(integration[1].as_str(), Some("dep:wasmtime"));
        assert!(dependencies["wasmtime"]["optional"].as_bool().unwrap());
        assert_eq!(dependencies["wasmtime"]["version"].as_str(), Some("45"));
        assert!(!features["default"]
            .as_array()
            .unwrap()
            .iter()
            .any(|feature| feature
                .as_str()
                .is_some_and(|feature| feature.contains("wasmtime"))));
        assert!(!dependencies.contains_key("wasmer"));
        assert!(!dependencies.contains_key("tokio"));
        assert!(!dependencies.contains_key("fp-bindgen-support"));
    }

    #[test]
    fn wasmtime45_compile_fixture_is_exact_generated_host_golden() {
        let mut imports = FunctionList::new();
        imports.add_function("fn checked(flag: bool, tiny: u8, total: u64) -> bool;");
        imports.add_function("fn reset();");
        let mut exports = FunctionList::new();
        exports.add_function("fn guest_measure(value: i16, ratio: f32) -> u16;");
        exports.add_function("fn guest_unit();");
        let rendered = render_bindings(&imports, &exports, &TypeMap::new()).unwrap();
        assert_eq!(
            rendered.host_linker,
            include_str!("../../tests/fixtures/wasmtime45_scalar_host.rs")
        );
    }
}
