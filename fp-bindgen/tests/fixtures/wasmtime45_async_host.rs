// Generated private Wasmtime 45 glue for Core Wasm ABI `kernal-api:v1`.
// Host trait and invocation helpers use semantic scalar and resource types.

fn u32_from_i32(value: i32) -> wasmtime::Result<u32> {
    Ok(value as u32)
}
fn u32_to_i32(value: u32) -> i32 {
    value as i32
}
fn u64_from_i64(value: i64) -> wasmtime::Result<u64> {
    Ok(value as u64)
}
fn u64_to_i64(value: u64) -> i64 {
    value as i64
}

pub(crate) trait KernalApiV1Imports {
    fn pending_total(&mut self, seed: u32) -> wasmtime::Result<u64>;
    fn poll_operation(&mut self, operation: u64) -> wasmtime::Result<i32>;
    fn take_operation_result(&mut self, operation: u64) -> wasmtime::Result<i64>;
    fn yield_operation(&mut self, operation: u64) -> wasmtime::Result<()>;
    fn cancel_operation(&mut self, operation: u64) -> wasmtime::Result<()>;
}

pub(crate) fn link_kernal_api_v1<T>(linker: &mut wasmtime::Linker<T>) -> wasmtime::Result<()>
where
    T: KernalApiV1Imports + Send + 'static,
{
    linker.func_wrap(
        "kernal-api:v1",
        "pending_total",
        |mut caller: wasmtime::Caller<'_, T>, seed: i32| -> wasmtime::Result<i64> {
            let seed = u32_from_i32(seed)?;
            Ok(caller.data_mut().pending_total(seed)? as i64)
        },
    )?;
    linker.func_wrap(
        "kernal-api:v1",
        "poll_operation",
        |mut caller: wasmtime::Caller<'_, T>, operation: i64| -> wasmtime::Result<i32> {
            caller.data_mut().poll_operation(operation as u64)
        },
    )?;
    linker.func_wrap(
        "kernal-api:v1",
        "take_operation_result",
        |mut caller: wasmtime::Caller<'_, T>, operation: i64| -> wasmtime::Result<i64> {
            caller.data_mut().take_operation_result(operation as u64)
        },
    )?;
    linker.func_wrap(
        "kernal-api:v1",
        "yield_operation",
        |mut caller: wasmtime::Caller<'_, T>, operation: i64| -> wasmtime::Result<()> {
            caller.data_mut().yield_operation(operation as u64)
        },
    )?;
    linker.func_wrap(
        "kernal-api:v1",
        "cancel_operation",
        |mut caller: wasmtime::Caller<'_, T>, operation: i64| -> wasmtime::Result<()> {
            caller.data_mut().cancel_operation(operation as u64)
        },
    )?;
    Ok(())
}
