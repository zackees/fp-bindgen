#![cfg(feature = "wasmtime-core-wasm")]

use fp_bindgen::prelude::*;
use std::{path::Path, process::Command, time::SystemTime};

#[cfg(feature = "wasmtime45-integration")]
#[rustfmt::skip]
#[path = "fixtures/wasmtime45_resource_host.rs"]
mod generated_host;

#[cfg(feature = "wasmtime45-integration")]
#[test]
fn generated_wasmtime_resource_glue_preserves_full_width_imports_and_exports() {
    #[derive(Default)]
    struct Host {
        seen: Vec<u64>,
    }

    impl generated_host::KernalApiV1Imports for Host {
        fn open_entry(
            &mut self,
            archive: generated_host::resources::Archive,
        ) -> wasmtime::Result<generated_host::resources::Entry> {
            self.seen.push(archive.0);
            Ok(generated_host::resources::Entry(!archive.0))
        }
    }

    let engine = wasmtime::Engine::default();
    let mut linker = wasmtime::Linker::new(&engine);
    generated_host::link_kernal_api_v1(&mut linker).unwrap();
    let module = wasmtime::Module::new(
        &engine,
        r#"
        (module
            (import "kernal-api:v1" "open_entry" (func $open_entry (param i64) (result i64)))
            (func (export "visit_archive") (param i64) (result i64)
                local.get 0
                call $open_entry))
    "#,
    )
    .unwrap();
    let mut store = wasmtime::Store::new(&engine, Host::default());
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let values = [
        0,
        1,
        i64::MAX as u64,
        1_u64 << 63,
        u64::MAX,
        0xfedc_ba98_7654_3210,
    ];
    for bits in values {
        let entry = generated_host::invoke_visit_archive(
            &mut store,
            &instance,
            generated_host::resources::Archive(bits),
        )
        .unwrap();
        assert_eq!(entry.0, !bits);
    }
    assert_eq!(store.data().seen, values);
}

fn cargo(output: &Path, args: &[&str]) {
    // Cargo inherits the outer test invocation's Rust compiler wrapper. A
    // second Soldr front door would violate its strict reentrancy contract.
    let result = Command::new(env!("CARGO"))
        .args(args)
        .arg("--offline")
        .arg("--manifest-path")
        .arg(output.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(output.join("target"))
        .env(
            "RUSTUP_TOOLCHAIN",
            std::env::var("RUSTUP_TOOLCHAIN").unwrap_or_else(|_| "1.95.0".to_owned()),
        )
        .output()
        .expect("run Cargo for the generated resource guest");
    assert!(
        result.status.success(),
        "generated resource guest failed {args:?}:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}

#[test]
fn generated_resource_guest_compiles_to_wasm_and_preserves_full_width_handles() {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output = std::env::temp_dir().join(format!(
        "fp-bindgen-resource-signatures-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&output).unwrap();

    let mut imports = FunctionList::new();
    imports.add_function("fn open_entry(archive: Archive) -> Entry;");
    imports.add_function("async fn next_entry(archive: Archive) -> Entry;");
    let mut exports = FunctionList::new();
    exports.add_function("fn visit_entry(entry: Entry) -> Archive;");
    let types = ["Archive", "Entry", "Result", "wasmtime", "std"]
        .into_iter()
        .map(|name| (TypeIdent::from(name), Type::Resource(Resource::new(name))))
        .collect();
    try_generate_wasmtime_core_wasm_bindings(imports, exports, types, output.to_str().unwrap())
        .unwrap();

    // The actual guest crate has no runtime dependencies and builds offline.
    cargo(&output, &["build", "--target", "wasm32-unknown-unknown"]);
    assert!(output
        .join("target/wasm32-unknown-unknown/debug/kernal_api_v1_bindings.wasm")
        .is_file());

    // Native ABI stubs exercise the generated adapters and completion decoder,
    // including values whose high bit must survive the signed i64 wire slot.
    let source_path = output.join("src/lib.rs");
    let mut source = std::fs::read_to_string(&source_path).unwrap();
    source.push_str(
        r#"
#[cfg(test)]
mod resource_codec_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static PENDING: AtomicU64 = AtomicU64::new(0);

    #[export_name = "open_entry"]
    extern "C" fn open_entry_stub(raw: i64) -> i64 { raw }

    #[export_name = "next_entry"]
    extern "C" fn next_entry_stub(raw: i64) -> i64 {
        PENDING.store(raw as u64, Ordering::SeqCst);
        -1
    }

    #[export_name = "poll_operation"]
    extern "C" fn poll_stub(operation: i64) -> i32 {
        assert_eq!(operation, -1);
        1
    }

    #[export_name = "take_operation_result"]
    extern "C" fn result_stub(operation: i64) -> i64 {
        assert_eq!(operation, -1);
        PENDING.load(Ordering::SeqCst) as i64
    }

    #[export_name = "yield_operation"]
    extern "C" fn yield_stub(_: i64) {}

    #[export_name = "cancel_operation"]
    extern "C" fn cancel_stub(_: i64) { panic!("completed operations must not cancel") }

    #[test]
    fn full_width_handles_cross_sync_async_and_export_adapters() {
        install_exports(KernalApiV1Exports {
            visit_entry: |entry| resources::Archive::from_abi(entry.into_abi()),
        }).unwrap();
        for bits in [0, 1, i64::MAX as u64, 1_u64 << 63, u64::MAX, 0xfedc_ba98_7654_3210] {
            let archive = resources::Archive::from_abi(bits);
            assert_eq!(resources::Archive::decode_i64(archive.encode_i64()).unwrap().into_abi(), bits);
            let entry = imports::open_entry(archive).unwrap();
            assert_eq!(entry.into_abi(), bits);
            assert_eq!(visit_entry(entry.encode_i64()) as u64, bits);
            let mut pending = imports::next_entry(archive).unwrap();
            assert_eq!(pending.poll().unwrap().unwrap().into_abi(), bits);
        }
        // Resource names live in their own module and cannot capture codec paths.
        assert_eq!(resources::Result::decode_i64(-1).unwrap().into_abi(), u64::MAX);
        assert_eq!(resources::std::decode_i64(-1).unwrap().into_abi(), u64::MAX);
        assert_eq!(resources::wasmtime::decode_i64(-1).unwrap().into_abi(), u64::MAX);
    }
}
"#,
    );
    std::fs::write(source_path, source).unwrap();
    cargo(&output, &["test", "--lib"]);
    std::fs::remove_dir_all(output).unwrap();
}
