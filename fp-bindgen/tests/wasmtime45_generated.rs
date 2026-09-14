#![cfg(feature = "wasmtime45-integration")]

#[path = "fixtures/wasmtime45_scalar_host.rs"]
mod generated;

#[derive(Default)]
struct Host {
    calls: usize,
}

impl generated::KernalApiV1Imports for Host {
    fn checked(&mut self, flag: bool, tiny: u8, total: u64) -> wasmtime::Result<bool> {
        self.calls += 1;
        Ok(flag && tiny == 7 && total == u64::MAX)
    }

    fn reset(&mut self) -> wasmtime::Result<()> {
        self.calls += 1;
        Ok(())
    }
}

#[test]
fn generated_wasmtime45_linker_glue_compiles_and_registers() {
    let engine = wasmtime::Engine::default();
    let mut linker = wasmtime::Linker::<Host>::new(&engine);
    generated::link_kernal_api_v1(&mut linker).unwrap();
}

#[test]
fn generated_linker_rejects_mutated_imports_before_host_effects() {
    let exact = r#"
        (module
            (import "kernal-api:v1" "checked"
                (func $checked (param i32 i32 i64) (result i32)))
            (func $start
                i32.const 1
                i32.const 7
                i64.const -1
                call $checked
                drop)
            (start $start))
    "#;
    let mutations = [
        (
            "namespace",
            r#"(module
                (import "kernal-api:v2" "checked"
                    (func $checked (param i32 i32 i64) (result i32))))"#,
        ),
        (
            "name",
            r#"(module
                (import "kernal-api:v1" "checked-mutated"
                    (func $checked (param i32 i32 i64) (result i32))))"#,
        ),
        (
            "signature",
            r#"(module
                (import "kernal-api:v1" "checked"
                    (func $checked (param i32 i32 i32) (result i32))))"#,
        ),
    ];
    let engine = wasmtime::Engine::default();

    let mut linker = wasmtime::Linker::<Host>::new(&engine);
    generated::link_kernal_api_v1(&mut linker).unwrap();

    let exact_module = wasmtime::Module::new(&engine, exact).unwrap();
    let mut exact_store = wasmtime::Store::new(&engine, Host::default());
    linker.instantiate(&mut exact_store, &exact_module).unwrap();
    assert_eq!(exact_store.data().calls, 1);

    for (mutation, wat) in mutations {
        let module = wasmtime::Module::new(&engine, wat).unwrap();
        let mut store = wasmtime::Store::new(&engine, Host::default());
        assert!(
            linker.instantiate(&mut store, &module).is_err(),
            "mutated {mutation} import must fail linker admission"
        );
        assert_eq!(
            store.data().calls,
            0,
            "mutated {mutation} import must fail before a host effect"
        );
    }
}
