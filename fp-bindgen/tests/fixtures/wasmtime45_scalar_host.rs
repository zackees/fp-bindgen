// Generated scalar Core Wasm host fixture for `kernal-api:v1`.
// Keep this shape aligned with the Wasmtime 45 linker template.

fn bool_from_i32(value: i32) -> wasmtime::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(wasmtime::Error::msg(format!(
            "invalid bool ABI value: {value}"
        ))),
    }
}

fn u8_from_i32(value: i32) -> wasmtime::Result<u8> {
    value
        .try_into()
        .map_err(|_| wasmtime::Error::msg(format!("invalid u8 ABI value: {value}")))
}

fn u64_from_i64(value: i64) -> wasmtime::Result<u64> {
    Ok(value as u64)
}

fn bool_to_i32(value: bool) -> i32 {
    i32::from(value)
}

pub(crate) trait KernalApiV1Imports {
    fn checked(&mut self, flag: bool, tiny: u8, total: u64) -> wasmtime::Result<bool>;
}

pub(crate) fn link_kernal_api_v1<T>(
    linker: &mut wasmtime::Linker<T>,
) -> wasmtime::Result<()>
where
    T: KernalApiV1Imports + Send + 'static,
{
    linker.func_wrap(
        "kernal-api:v1",
        "checked",
        |mut caller: wasmtime::Caller<'_, T>, flag: i32, tiny: i32, total: i64| -> wasmtime::Result<i32> {
            let flag = bool_from_i32(flag)?;
            let tiny = u8_from_i32(tiny)?;
            let total = u64_from_i64(total)?;
            Ok(bool_to_i32(caller.data_mut().checked(flag, tiny, total)?))
        },
    )?;
    Ok(())
}
