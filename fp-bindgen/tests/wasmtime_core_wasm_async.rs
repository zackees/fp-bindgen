#![cfg(feature = "wasmtime-core-wasm")]

use fp_bindgen::prelude::*;
use std::{collections::BTreeMap, path::Path, process::Command};

#[cfg(feature = "wasmtime45-integration")]
#[path = "fixtures/wasmtime45_async_host.rs"]
mod generated;

fn build_generated_guest(output: &Path) {
    let build = Command::new(env!("CARGO"))
        .args([
            "build",
            "--offline",
            "--target",
            "wasm32-unknown-unknown",
            "--target-dir",
        ])
        .arg(output.join("target"))
        .arg("--manifest-path")
        .arg(output.join("Cargo.toml"))
        .output()
        .expect("run Cargo for generated guest");
    assert!(
        build.status.success(),
        "generated guest must build to Core Wasm:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        output
            .join("target/wasm32-unknown-unknown/debug/kernal_api_v1_bindings.wasm")
            .is_file(),
        "generated cdylib must produce a Core Wasm artifact"
    );
}

#[test]
fn async_import_generates_a_bounded_operation_handle_protocol() {
    let mut imports = FunctionList::new();
    imports.add_function("async fn pending_total(seed: u32) -> u32;");
    imports.add_function("fn current_total() -> u32;");
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

    // This is the actual guest boundary: the generated scalar operation
    // protocol must compile as a Core Wasm cdylib, not merely parse as Rust.
    // `--offline` also proves the emitted guest has no hidden runtime crate
    // dependency to resolve.
    build_generated_guest(&output);

    std::fs::remove_dir_all(output).unwrap();
}

#[cfg(feature = "wasmtime45-integration")]
#[derive(Default)]
struct AsyncHost {
    next_operation: u64,
    operations: BTreeMap<u64, Operation>,
    events: Vec<String>,
}

#[cfg(feature = "wasmtime45-integration")]
struct Operation {
    value: u32,
    yielded: bool,
}

#[cfg(feature = "wasmtime45-integration")]
impl generated::KernalApiV1Imports for AsyncHost {
    fn pending_total(&mut self, seed: u32) -> wasmtime::Result<u64> {
        self.next_operation += 1;
        let operation = self.next_operation;
        self.operations.insert(
            operation,
            Operation {
                value: seed + 1,
                yielded: false,
            },
        );
        self.events.push(format!("submit:{operation}:{seed}"));
        Ok(operation)
    }

    fn poll_operation(&mut self, operation: u64) -> wasmtime::Result<i32> {
        let operation = self
            .operations
            .get(&operation)
            .ok_or_else(|| wasmtime::Error::msg("unknown operation"))?;
        Ok(i32::from(operation.yielded))
    }

    fn take_operation_result(&mut self, operation: u64) -> wasmtime::Result<i64> {
        let operation = self
            .operations
            .remove(&operation)
            .ok_or_else(|| wasmtime::Error::msg("unknown operation"))?;
        self.events.push(format!("take:{}", operation.value));
        Ok(i64::from(operation.value))
    }

    fn yield_operation(&mut self, operation: u64) -> wasmtime::Result<()> {
        let operation = self
            .operations
            .get_mut(&operation)
            .ok_or_else(|| wasmtime::Error::msg("unknown operation"))?;
        operation.yielded = true;
        self.events.push("yield:1".to_owned());
        Ok(())
    }

    fn cancel_operation(&mut self, operation: u64) -> wasmtime::Result<()> {
        self.operations
            .remove(&operation)
            .ok_or_else(|| wasmtime::Error::msg("unknown operation"))?;
        self.events.push(format!("cancel:{operation}"));
        Ok(())
    }
}

#[cfg(feature = "wasmtime45-integration")]
fn build_async_consumer(output: &Path) -> std::path::PathBuf {
    let consumer = output.join("consumer");
    std::fs::create_dir_all(consumer.join("src")).unwrap();
    std::fs::write(
        consumer.join("Cargo.toml"),
        format!(
            "[package]\nname = \"fp-bindgen-async-guest\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nkernal-api-v1-bindings = {{ path = {:?} }}\n",
            output
        ),
    )
    .unwrap();
    std::fs::write(
        consumer.join("src/lib.rs"),
        "#[no_mangle]\npub extern \"C\" fn await_total(seed: u32) -> u32 {\n    let mut operation = kernal_api_v1_bindings::imports::pending_total(seed).unwrap();\n    assert!(operation.poll().unwrap().is_none());\n    operation.yield_now().unwrap();\n    operation.poll().unwrap().unwrap()\n}\n\n#[no_mangle]\npub extern \"C\" fn abandon_total(seed: u32) {\n    let _operation = kernal_api_v1_bindings::imports::pending_total(seed).unwrap();\n}\n",
    )
    .unwrap();
    let build = Command::new(env!("CARGO"))
        .args([
            "build",
            "--offline",
            "--target",
            "wasm32-unknown-unknown",
            "--target-dir",
        ])
        .arg(consumer.join("target"))
        .arg("--manifest-path")
        .arg(consumer.join("Cargo.toml"))
        .output()
        .expect("build generated async consumer");
    assert!(
        build.status.success(),
        "generated async consumer must build to Core Wasm:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    consumer.join("target/wasm32-unknown-unknown/debug/fp_bindgen_async_guest.wasm")
}

#[cfg(feature = "wasmtime45-integration")]
#[test]
fn generated_async_guest_uses_store_owned_operations_after_prelink() {
    let mut imports = FunctionList::new();
    imports.add_function("async fn pending_total(seed: u32) -> u32;");
    let output = std::env::temp_dir().join(format!(
        "fp-bindgen-wasmtime-core-wasm-async-runtime-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&output);
    try_generate_wasmtime_core_wasm_bindings(
        imports,
        FunctionList::new(),
        TypeMap::new(),
        output.to_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(output.join("wasmtime45_host_linker.rs"))
            .unwrap()
            .split_whitespace()
            .collect::<String>(),
        include_str!("fixtures/wasmtime45_async_host.rs")
            .split_whitespace()
            .collect::<String>(),
        "runtime proof must use the exact generated Wasmtime linker glue"
    );
    let guest = build_async_consumer(&output);
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_file(&engine, guest).unwrap();
    let mut linker = wasmtime::Linker::<AsyncHost>::new(&engine);
    generated::link_kernal_api_v1(&mut linker).unwrap();
    let prelinked = linker.instantiate_pre(&module).unwrap();

    {
        let mut store = wasmtime::Store::new(&engine, AsyncHost::default());
        let instance = prelinked.instantiate(&mut store).unwrap();
        let await_total = instance
            .get_typed_func::<i32, i32>(&mut store, "await_total")
            .unwrap();
        assert_eq!(await_total.call(&mut store, 41).unwrap(), 42);
        assert_eq!(store.data().events, ["submit:1:41", "yield:1", "take:42"]);
        assert!(store.data().operations.is_empty());
    }

    let mut second_store = wasmtime::Store::new(&engine, AsyncHost::default());
    let second = prelinked.instantiate(&mut second_store).unwrap();
    let abandon_total = second
        .get_typed_func::<i32, ()>(&mut second_store, "abandon_total")
        .unwrap();
    abandon_total.call(&mut second_store, 9).unwrap();
    assert_eq!(second_store.data().events, ["submit:1:9", "cancel:1"]);
    assert!(second_store.data().operations.is_empty());
    std::fs::remove_dir_all(output).unwrap();
}
