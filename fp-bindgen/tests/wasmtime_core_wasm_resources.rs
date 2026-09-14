#![cfg(feature = "wasmtime-core-wasm")]

use fp_bindgen::prelude::*;
use std::{collections::BTreeSet, path::Path, process::Command, time::SystemTime};

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

        fn resource_release_archive(
            &mut self,
            archive: generated_host::resources::Archive,
        ) -> wasmtime::Result<i32> {
            self.seen.push(archive.0);
            Ok(0)
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
            (func (export "visit_entry") (param i64) (result i64)
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
        assert_eq!(
            generated_host::resources::Archive(bits).encode_i64(),
            bits as i64
        );
        let entry = generated_host::invoke_visit_entry(
            &mut store,
            &instance,
            generated_host::resources::Entry(bits),
        )
        .unwrap();
        assert_eq!(entry.0, !bits);
    }
    assert_eq!(store.data().seen, values);
}

/// The host owns scope, generation, and teardown; generated glue must route
/// the release control through that one authority instead of treating a raw
/// `i64` as permission.  The encoded values are deliberately simple test
/// tokens: high 32 bits are Store scope and low 32 bits are generation.
#[cfg(feature = "wasmtime45-integration")]
struct ScopedResourceHost {
    scope: u32,
    live: BTreeSet<u64>,
}

#[cfg(feature = "wasmtime45-integration")]
impl ScopedResourceHost {
    fn with_live(scope: u32, generation: u32) -> Self {
        let raw = (u64::from(scope) << 32) | u64::from(generation);
        Self {
            scope,
            live: BTreeSet::from([raw]),
        }
    }
}

#[cfg(feature = "wasmtime45-integration")]
impl generated_host::KernalApiV1Imports for ScopedResourceHost {
    fn open_entry(
        &mut self,
        _: generated_host::resources::Archive,
    ) -> wasmtime::Result<generated_host::resources::Entry> {
        Err(wasmtime::Error::msg(
            "resource lifecycle probe never opens an entry",
        ))
    }

    fn resource_release_archive(
        &mut self,
        archive: generated_host::resources::Archive,
    ) -> wasmtime::Result<i32> {
        if (archive.0 >> 32) as u32 != self.scope {
            return Ok(2); // foreign Store scope
        }
        Ok(if self.live.remove(&archive.0) {
            0 // atomically revoked
        } else {
            1 // stale/double-close generation
        })
    }
}

#[cfg(feature = "wasmtime45-integration")]
#[test]
fn generated_release_control_rejects_stale_and_foreign_store_handles() {
    let engine = wasmtime::Engine::default();
    let mut linker = wasmtime::Linker::new(&engine);
    generated_host::link_kernal_api_v1(&mut linker).unwrap();
    let module = wasmtime::Module::new(
        &engine,
        r#"
        (module
            (import "kernal-api:v1" "resource_release_archive" (func $release (param i64) (result i32)))
            (func (export "release") (param i64) (result i32)
                local.get 0
                call $release))
    "#,
    )
    .unwrap();
    let scope_one = (1_u64 << 32) | 7;
    let scope_two = (2_u64 << 32) | 7;

    let mut first = wasmtime::Store::new(&engine, ScopedResourceHost::with_live(1, 7));
    let instance = linker.instantiate(&mut first, &module).unwrap();
    let release = instance
        .get_typed_func::<i64, i32>(&mut first, "release")
        .unwrap();
    assert_eq!(release.call(&mut first, scope_one as i64).unwrap(), 0);
    assert_eq!(release.call(&mut first, scope_one as i64).unwrap(), 1);

    let mut second = wasmtime::Store::new(&engine, ScopedResourceHost::with_live(2, 7));
    let instance = linker.instantiate(&mut second, &module).unwrap();
    let release = instance
        .get_typed_func::<i64, i32>(&mut second, "release")
        .unwrap();
    assert_eq!(release.call(&mut second, scope_one as i64).unwrap(), 2);
    assert_eq!(release.call(&mut second, scope_two as i64).unwrap(), 0);
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

fn cargo_fails(output: &Path, args: &[&str]) {
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
        .expect("run Cargo for the generated resource guest rejection proof");
    assert!(
        !result.status.success(),
        "generated owned resource was released while an async borrow was pending"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("cannot move out of `archive` because it is borrowed"),
        "borrow-rejection proof failed for an unexpected reason:\n{}",
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
    imports.add_function("fn acquire() -> Archive;");
    imports.add_function("async fn next_entry(archive: Archive) -> Entry;");
    imports.add_function("async fn next_archive() -> Archive;");
    let mut exports = FunctionList::new();
    exports.add_function("fn visit_entry(entry: Entry) -> Entry;");
    let mut types = ["Entry", "Result", "wasmtime", "std"]
        .into_iter()
        .map(|name| (TypeIdent::from(name), Type::Resource(Resource::new(name))))
        .collect::<TypeMap>();
    types.insert(
        TypeIdent::from("Archive"),
        Type::Resource(Resource::owned("Archive")),
    );
    types.insert(
        TypeIdent::from("Option"),
        Type::Resource(Resource::owned("Option")),
    );
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

    #[export_name = "acquire"]
    extern "C" fn acquire_stub() -> i64 { 0xface_cafe_dead_beefu64 as i64 }

    #[export_name = "next_entry"]
    extern "C" fn next_entry_stub(raw: i64) -> i64 {
        PENDING.store(raw as u64, Ordering::SeqCst);
        -1
    }

    #[export_name = "next_archive"]
    extern "C" fn next_archive_stub() -> i64 {
        PENDING.store(0x9abc_def0_1234_5678, Ordering::SeqCst);
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

    static CANCELLATIONS: AtomicU64 = AtomicU64::new(0);

    #[export_name = "cancel_operation"]
    extern "C" fn cancel_stub(_: i64) { CANCELLATIONS.fetch_add(1, Ordering::SeqCst); }

    static RELEASES: AtomicU64 = AtomicU64::new(0);

    #[export_name = "resource_release_archive"]
    extern "C" fn release_archive_stub(raw: i64) -> i32 {
        RELEASES.fetch_add(1, Ordering::SeqCst);
        if raw == 0xfeed { 9 } else { 0 }
    }

    #[test]
    fn full_width_handles_cross_sync_async_and_export_adapters() {
        install_exports(KernalApiV1Exports {
            visit_entry: |entry| entry,
        }).unwrap();
        for bits in [0, 1, i64::MAX as u64, 1_u64 << 63, u64::MAX, 0xfedc_ba98_7654_3210] {
            let archive = resources::Archive::from_abi(bits);
            let entry = imports::open_entry(&archive).unwrap();
            assert_eq!(entry.into_abi(), bits);
            assert_eq!(visit_entry(entry.encode_i64()) as u64, bits);
            let acquired = imports::acquire().unwrap();
            acquired.close().unwrap();
            let mut pending = imports::next_entry(&archive).unwrap();
            assert_eq!(pending.poll().unwrap().unwrap().into_abi(), bits);
            drop(pending);
            let mut pending = imports::next_archive().unwrap();
            let acquired = pending.poll().unwrap().unwrap();
            drop(pending);
            assert_eq!(
                acquired.encode_i64().unwrap() as u64,
                0x9abc_def0_1234_5678
            );
            acquired.close().unwrap();
            let pending = imports::next_entry(&archive).unwrap();
            drop(pending);
            archive.close().unwrap();
        }
        // Resource names live in their own module and cannot capture codec paths.
        assert_eq!(resources::Result::decode_i64(-1).unwrap().into_abi(), u64::MAX);
        assert_eq!(resources::std::decode_i64(-1).unwrap().into_abi(), u64::MAX);
        assert_eq!(resources::wasmtime::decode_i64(-1).unwrap().into_abi(), u64::MAX);
        let rejected = resources::Archive::from_abi(0xfeed).close().unwrap_err();
        assert_eq!(rejected, AbiError::ResourceReleaseRejected { resource: "Archive", status: 9 });
        drop(resources::Archive::from_abi(0xbeef));
        assert_eq!(CANCELLATIONS.load(Ordering::SeqCst), 6);
        assert_eq!(RELEASES.load(Ordering::SeqCst), 20);
    }
}
"#,
    );
    std::fs::write(source_path, source).unwrap();
    cargo(&output, &["test", "--lib"]);
    std::fs::create_dir(output.join("src/bin")).unwrap();
    std::fs::write(
        output.join("src/bin/borrow_reject.rs"),
        r#"
fn cannot_release_while_pending(archive: kernal_api_v1_bindings::resources::Archive) {
    let pending = kernal_api_v1_bindings::imports::next_entry(&archive).unwrap();
    let _ = archive.close();
    drop(pending);
}

fn main() {}
"#,
    )
    .unwrap();
    std::fs::write(
        output.join("src/bin/borrow_release.rs"),
        r#"
fn release_after_pending(archive: kernal_api_v1_bindings::resources::Archive) {
    let pending = kernal_api_v1_bindings::imports::next_entry(&archive).unwrap();
    drop(pending);
    let _ = archive.close();
}

fn main() {}
"#,
    )
    .unwrap();
    cargo(&output, &["check", "--bin", "borrow_release"]);
    cargo_fails(&output, &["check", "--bin", "borrow_reject"]);
    std::fs::remove_dir_all(output).unwrap();
}
