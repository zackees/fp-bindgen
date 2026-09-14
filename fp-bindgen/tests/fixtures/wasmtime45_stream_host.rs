// Generated private Wasmtime 45 glue for Core Wasm ABI `kernal-api:v1`.
// Host trait and invocation helpers use semantic scalar and resource types.

pub(crate) mod resources {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub(crate) struct Input(pub(crate) u64);

    impl Input {
        pub(crate) fn decode_i64(raw: i64) -> ::wasmtime::Result<Self> { ::std::result::Result::Ok(Self(raw as u64)) }
        pub(crate) fn encode_i64(self) -> i64 { self.0 as i64 }
    }

 }


fn bounded_stream_len(value: i32) -> wasmtime::Result<usize> {
    let value = usize::try_from(value).map_err(|_| wasmtime::Error::msg("negative stream chunk length"))?;
    if value <= 65536usize { Ok(value) } else { Err(wasmtime::Error::msg(format!("stream chunk exceeds 65536 bytes: {value}"))) }
}
fn guest_memory_offset(value: i32) -> usize { value as u32 as usize }
fn caller_memory<T>(caller: &mut wasmtime::Caller<'_, T>) -> wasmtime::Result<wasmtime::Memory> {
    caller.get_export("memory").and_then(|export| export.into_memory()).ok_or_else(|| wasmtime::Error::msg("stream controls require exported guest memory named `memory`"))
}
fn checked_guest_memory_range<T>(memory: &wasmtime::Memory, caller: &wasmtime::Caller<'_, T>, offset: usize, length: usize) -> wasmtime::Result<()> {
    let end = offset.checked_add(length).ok_or_else(|| wasmtime::Error::msg("stream guest-memory range overflow"))?;
    if end <= memory.data_size(caller) { Ok(()) } else { Err(wasmtime::Error::msg("stream guest-memory range is out of bounds")) }
}
fn checked_stream_count(transferred: usize, requested: usize) -> wasmtime::Result<()> {
    if transferred <= requested && transferred <= 65536usize { Ok(()) } else { Err(wasmtime::Error::msg(format!("host returned invalid stream count {transferred} for requested {requested}"))) }
}


pub(crate) trait KernalApiV1Imports {

    /// Transfer one bounded caller-memory chunk. The host registry validates the raw stream handle.
    fn stream_read(&mut self, stream: u64, destination: &mut [u8]) -> wasmtime::Result<usize>;
    /// Transfer one bounded caller-memory chunk. The host registry validates the raw stream handle.
    fn stream_write(&mut self, stream: u64, source: &[u8]) -> wasmtime::Result<usize>;
    /// Atomically revoke a stream handle through the host's canonical scope/generation registry.
    fn stream_close(&mut self, stream: u64) -> wasmtime::Result<i32>;
}

pub(crate) fn link_kernal_api_v1<T>(linker: &mut wasmtime::Linker<T>) -> wasmtime::Result<()>
where
    T: KernalApiV1Imports + Send + 'static,
{

    linker.func_wrap(
        "kernal-api:v1",
        "stream_read",
        |mut caller: wasmtime::Caller<'_, T>, stream: i64, destination: i32, destination_len: i32| -> wasmtime::Result<i32> {
            let destination_len = bounded_stream_len(destination_len)?;
            let destination = guest_memory_offset(destination);
            let memory = caller_memory(&mut caller)?;
            checked_guest_memory_range(&memory, &caller, destination, destination_len)?;
            let mut chunk = ::std::vec![0; destination_len];
            let transferred = caller.data_mut().stream_read(stream as u64, &mut chunk)?;
            checked_stream_count(transferred, destination_len)?;
            memory.write(&mut caller, destination, &chunk[..transferred])?;
            Ok(transferred as i32)
        },
    )?;
    linker.func_wrap(
        "kernal-api:v1",
        "stream_write",
        |mut caller: wasmtime::Caller<'_, T>, stream: i64, source: i32, source_len: i32| -> wasmtime::Result<i32> {
            let source_len = bounded_stream_len(source_len)?;
            let source = guest_memory_offset(source);
            let memory = caller_memory(&mut caller)?;
            checked_guest_memory_range(&memory, &caller, source, source_len)?;
            let mut chunk = ::std::vec![0; source_len];
            memory.read(&caller, source, &mut chunk)?;
            let transferred = caller.data_mut().stream_write(stream as u64, &chunk)?;
            checked_stream_count(transferred, source_len)?;
            Ok(transferred as i32)
        },
    )?;
    linker.func_wrap(
        "kernal-api:v1",
        "stream_close",
        |mut caller: wasmtime::Caller<'_, T>, stream: i64| -> wasmtime::Result<i32> {
            caller.data_mut().stream_close(stream as u64)
        },
    )?;
    Ok(())
}
