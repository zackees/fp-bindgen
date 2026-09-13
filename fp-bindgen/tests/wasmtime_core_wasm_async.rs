#![cfg(feature = "wasmtime-core-wasm")]

use fp_bindgen::prelude::*;

#[test]
fn async_import_generates_a_bounded_operation_handle_protocol() {
    let mut imports = FunctionList::new();
    imports.add_function("async fn pending_total(seed: u32) -> u32;");
    let output = std::env::temp_dir().join(format!(
        "fp-bindgen-wasmtime-core-wasm-async-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&output);

    try_generate_wasmtime_core_wasm_bindings(
        imports,
        FunctionList::new(),
        TypeMap::new(),
        output.to_str().unwrap(),
    )
    .expect("async scalar imports must generate operation-handle bindings");

    let guest = std::fs::read_to_string(output.join("src/lib.rs")).unwrap();
    let host = std::fs::read_to_string(output.join("wasmtime45_host_linker.rs")).unwrap();
    let manifest = std::fs::read_to_string(output.join("kernal-api-v1.abi.toml")).unwrap();
    assert!(guest.contains("pub struct PendingOperation"));
    syn::parse_file(&guest).expect("generated async guest source must be syntactically valid Rust");
    assert!(guest.contains("impl<T> Drop for PendingOperation<T>"));
    assert!(guest.contains("super::u32_from_i32(raw as i32)"));
    assert!(guest
        .contains("pub fn pending_total(seed: u32) -> Result<PendingOperation<u32>, AbiError>"));
    assert!(host.contains("fn pending_total(&mut self, seed: u32) -> wasmtime::Result<u64>;"));
    assert!(host.contains("fn poll_operation(&mut self, operation: u64) -> wasmtime::Result<i32>;"));
    assert!(host
        .contains("fn take_operation_result(&mut self, operation: u64) -> wasmtime::Result<i64>;"));
    assert!(host.contains("fn yield_operation(&mut self, operation: u64) -> wasmtime::Result<()>;"));
    assert!(
        host.contains("fn cancel_operation(&mut self, operation: u64) -> wasmtime::Result<()>;")
    );
    assert!(manifest.contains("async = true"));
    assert!(manifest.contains("operation_result = { semantic = \"u32\", abi = \"i32\" }"));
    for control in [
        "poll_operation",
        "take_operation_result",
        "yield_operation",
        "cancel_operation",
    ] {
        assert!(manifest.contains(&format!(
            "name = \"{control}\"\ndirection = \"guest-to-host\"\ngenerated = true"
        )));
    }

    std::fs::remove_dir_all(output).unwrap();
}
