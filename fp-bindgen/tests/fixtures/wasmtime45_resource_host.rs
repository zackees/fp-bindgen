// Generated private Wasmtime 45 glue for Core Wasm ABI `kernal-api:v1`.
// Host trait and invocation helpers use semantic scalar and resource types.

pub(crate) mod resources {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub(crate) struct Archive(pub(crate) u64);

    impl Archive {
        pub(crate) fn decode_i64(raw: i64) -> ::wasmtime::Result<Self> { ::std::result::Result::Ok(Self(raw as u64)) }
        pub(crate) fn encode_i64(self) -> i64 { self.0 as i64 }
    }

    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub(crate) struct Entry(pub(crate) u64);

    impl Entry {
        pub(crate) fn decode_i64(raw: i64) -> ::wasmtime::Result<Self> { ::std::result::Result::Ok(Self(raw as u64)) }
        pub(crate) fn encode_i64(self) -> i64 { self.0 as i64 }
    }

 }



pub(crate) trait KernalApiV1Imports {
    fn open_entry(&mut self, archive: resources::Archive) -> wasmtime::Result<resources::Entry>;
    /// Atomically revoke this guest-owned handle through the host's canonical scope/generation registry.
    fn resource_release_archive(&mut self, resource: resources::Archive) -> wasmtime::Result<i32>;
}

pub(crate) fn link_kernal_api_v1<T>(linker: &mut wasmtime::Linker<T>) -> wasmtime::Result<()>
where
    T: KernalApiV1Imports + Send + 'static,
{
    linker.func_wrap(
        "kernal-api:v1",
        "open_entry",
        |mut caller: wasmtime::Caller<'_, T>,
         archive: i64|
         -> wasmtime::Result<i64> {
            let archive = resources::Archive::decode_i64(archive)?;
            Ok(resources::Entry::encode_i64(caller.data_mut().open_entry(archive)?))
        },
    )?;
    linker.func_wrap(
        "kernal-api:v1",
        "resource_release_archive",
        |mut caller: wasmtime::Caller<'_, T>, resource: i64| -> wasmtime::Result<i32> {
            caller.data_mut().resource_release_archive(resources::Archive::decode_i64(resource)?)
        },
    )?;
    Ok(())
}

pub(crate) fn invoke_visit_entry<T>(
    store: &mut wasmtime::Store<T>,
    instance: &wasmtime::Instance,
    entry: resources::Entry,
) -> wasmtime::Result<resources::Entry> {
    let function = instance.get_typed_func::<i64, i64>(&mut *store, "visit_entry")?;
    let raw = function.call(&mut *store, resources::Entry::encode_i64(entry))?;
    resources::Entry::decode_i64(raw)
}
