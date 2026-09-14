#![cfg(feature = "wasmtime-core-wasm")]

use fp_bindgen::prelude::*;
use std::{
    collections::BTreeMap,
    path::Path,
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier, Mutex,
    },
};

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

/// The generated linker deliberately accepts a Store-local host state.  The
/// kernel owns the logical operation registry, though, so a real threaded
/// sketch supplies each Store with a distinct scope over one shared registry.
/// This fixture makes that ownership boundary observable without making the
/// generated ABI or a Wasmtime type public.
#[cfg(feature = "wasmtime45-integration")]
#[derive(Clone)]
struct SharedAsyncHost {
    store_scope: u64,
    next_operation: Arc<AtomicU64>,
    operations: Arc<Mutex<BTreeMap<u64, SharedOperation>>>,
    events: Arc<Mutex<Vec<String>>>,
    launch_gate: Option<Arc<Barrier>>,
}

#[cfg(feature = "wasmtime45-integration")]
struct SharedOperation {
    store_scope: u64,
    value: u32,
    yielded: bool,
}

#[cfg(feature = "wasmtime45-integration")]
impl SharedAsyncHost {
    fn new(
        store_scope: u64,
        next_operation: Arc<AtomicU64>,
        operations: Arc<Mutex<BTreeMap<u64, SharedOperation>>>,
        events: Arc<Mutex<Vec<String>>>,
        launch_gate: Option<Arc<Barrier>>,
    ) -> Self {
        Self {
            store_scope,
            next_operation,
            operations,
            events,
            launch_gate,
        }
    }

    fn operation_mut(
        &self,
        operation: u64,
    ) -> wasmtime::Result<std::sync::MutexGuard<'_, BTreeMap<u64, SharedOperation>>> {
        let operations = self
            .operations
            .lock()
            .map_err(|_| wasmtime::Error::msg("shared operation registry poisoned"))?;
        match operations.get(&operation) {
            Some(shared) if shared.store_scope == self.store_scope => Ok(operations),
            Some(_) => Err(wasmtime::Error::msg("operation belongs to another Store")),
            None => Err(wasmtime::Error::msg("unknown operation")),
        }
    }
}

#[cfg(feature = "wasmtime45-integration")]
impl generated::KernalApiV1Imports for SharedAsyncHost {
    fn pending_total(&mut self, seed: u32) -> wasmtime::Result<u64> {
        let operation = self.next_operation.fetch_add(1, Ordering::Relaxed) + 1;
        self.operations
            .lock()
            .map_err(|_| wasmtime::Error::msg("shared operation registry poisoned"))?
            .insert(
                operation,
                SharedOperation {
                    store_scope: self.store_scope,
                    value: seed + 1,
                    yielded: false,
                },
            );
        self.events
            .lock()
            .map_err(|_| wasmtime::Error::msg("shared operation registry poisoned"))?
            .push(format!("submit:{}:{operation}:{seed}", self.store_scope));
        if let Some(launch_gate) = &self.launch_gate {
            launch_gate.wait();
        }
        Ok(operation)
    }

    fn poll_operation(&mut self, operation: u64) -> wasmtime::Result<i32> {
        Ok(i32::from(
            self.operation_mut(operation)?
                .get(&operation)
                .expect("operation ownership was checked")
                .yielded,
        ))
    }

    fn take_operation_result(&mut self, operation: u64) -> wasmtime::Result<i64> {
        let value = self
            .operation_mut(operation)?
            .remove(&operation)
            .expect("operation ownership was checked")
            .value;
        self.events
            .lock()
            .map_err(|_| wasmtime::Error::msg("shared operation registry poisoned"))?
            .push(format!("take:{}:{operation}", self.store_scope));
        Ok(i64::from(value))
    }

    fn yield_operation(&mut self, operation: u64) -> wasmtime::Result<()> {
        self.operation_mut(operation)?
            .get_mut(&operation)
            .expect("operation ownership was checked")
            .yielded = true;
        self.events
            .lock()
            .map_err(|_| wasmtime::Error::msg("shared operation registry poisoned"))?
            .push(format!("yield:{}:{operation}", self.store_scope));
        Ok(())
    }

    fn cancel_operation(&mut self, operation: u64) -> wasmtime::Result<()> {
        self.operation_mut(operation)?
            .remove(&operation)
            .expect("operation ownership was checked");
        self.events
            .lock()
            .map_err(|_| wasmtime::Error::msg("shared operation registry poisoned"))?
            .push(format!("cancel:{}:{operation}", self.store_scope));
        Ok(())
    }
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

#[cfg(feature = "wasmtime45-integration")]
#[test]
fn generated_async_guest_scopes_shared_operations_across_parallel_stores() {
    let mut imports = FunctionList::new();
    imports.add_function("async fn pending_total(seed: u32) -> u32;");
    let output = std::env::temp_dir().join(format!(
        "fp-bindgen-wasmtime-core-wasm-async-shared-registry-{}",
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
        "the shared-registry proof must exercise exact generated linker glue"
    );
    let guest = build_async_consumer(&output);
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_file(&engine, guest).unwrap();
    let mut linker = wasmtime::Linker::<SharedAsyncHost>::new(&engine);
    generated::link_kernal_api_v1(&mut linker).unwrap();
    let prelinked = Arc::new(linker.instantiate_pre(&module).unwrap());
    let engine = Arc::new(engine);
    let next_operation = Arc::new(AtomicU64::new(0));
    let operations = Arc::new(Mutex::new(BTreeMap::new()));
    let events = Arc::new(Mutex::new(Vec::new()));

    let mut owner = SharedAsyncHost::new(
        10,
        Arc::clone(&next_operation),
        Arc::clone(&operations),
        Arc::clone(&events),
        None,
    );
    let mut foreign = SharedAsyncHost::new(
        11,
        Arc::clone(&next_operation),
        Arc::clone(&operations),
        Arc::clone(&events),
        None,
    );
    let foreign_operation =
        <SharedAsyncHost as generated::KernalApiV1Imports>::pending_total(&mut owner, 7).unwrap();
    assert_eq!(
        <SharedAsyncHost as generated::KernalApiV1Imports>::poll_operation(
            &mut foreign,
            foreign_operation,
        )
        .unwrap_err()
        .to_string(),
        "operation belongs to another Store"
    );
    <SharedAsyncHost as generated::KernalApiV1Imports>::cancel_operation(
        &mut owner,
        foreign_operation,
    )
    .unwrap();
    let launch_gate = Arc::new(Barrier::new(2));

    std::thread::scope(|scope| {
        let first = {
            let engine = Arc::clone(&engine);
            let prelinked = Arc::clone(&prelinked);
            let next_operation = Arc::clone(&next_operation);
            let operations = Arc::clone(&operations);
            let events = Arc::clone(&events);
            let launch_gate = Arc::clone(&launch_gate);
            scope.spawn(move || {
                let mut store = wasmtime::Store::new(
                    &engine,
                    SharedAsyncHost::new(1, next_operation, operations, events, Some(launch_gate)),
                );
                let instance = prelinked.instantiate(&mut store).unwrap();
                let await_total = instance
                    .get_typed_func::<i32, i32>(&mut store, "await_total")
                    .unwrap();
                assert_eq!(await_total.call(&mut store, 41).unwrap(), 42);
            })
        };
        let second = {
            let engine = Arc::clone(&engine);
            let prelinked = Arc::clone(&prelinked);
            let next_operation = Arc::clone(&next_operation);
            let operations = Arc::clone(&operations);
            let events = Arc::clone(&events);
            let launch_gate = Arc::clone(&launch_gate);
            scope.spawn(move || {
                let mut store = wasmtime::Store::new(
                    &engine,
                    SharedAsyncHost::new(2, next_operation, operations, events, Some(launch_gate)),
                );
                let instance = prelinked.instantiate(&mut store).unwrap();
                let abandon_total = instance
                    .get_typed_func::<i32, ()>(&mut store, "abandon_total")
                    .unwrap();
                abandon_total.call(&mut store, 9).unwrap();
            })
        };
        first.join().unwrap();
        second.join().unwrap();
    });

    assert!(operations.lock().unwrap().is_empty());
    assert_eq!(next_operation.load(Ordering::Relaxed), 3);
    let events = events.lock().unwrap();
    assert!(events.iter().any(|event| event.starts_with("submit:1:")));
    assert!(events.iter().any(|event| event.starts_with("submit:2:")));
    assert!(events.iter().any(|event| event.starts_with("take:1:")));
    assert!(events.iter().any(|event| event.starts_with("cancel:2:")));
    drop(events);
    std::fs::remove_dir_all(output).unwrap();
}
