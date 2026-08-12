use std::sync::OnceLock;

pub use eqts_macros::export;
pub use inventory;
#[cfg(feature = "node-napi")]
pub use napi_derive;
pub use serde;
#[cfg(feature = "wasm")]
pub use wasm_bindgen;

pub const ABI_OK: i32 = 0;
pub const ABI_PANIC: i32 = 1;
pub const ABI_NULL_OUTPUT: i32 = 2;

#[derive(Debug, serde::Serialize)]
pub struct Metadata {
    pub schema_version: u32,
    pub functions: Vec<&'static Function>,
}

#[derive(Debug, serde::Serialize)]
pub struct Function {
    pub module: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
    pub parameters: &'static [Parameter],
    pub result: Scalar,
}

#[derive(Debug, serde::Serialize)]
pub struct Parameter {
    pub name: &'static str,
    pub scalar: Scalar,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scalar {
    Void,
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

#[repr(C)]
pub struct MetadataSlice {
    pub ptr: *const u8,
    pub len: usize,
}

inventory::collect!(Function);

#[must_use]
pub fn metadata_json() -> &'static [u8] {
    static METADATA: OnceLock<Box<[u8]>> = OnceLock::new();
    METADATA.get_or_init(|| {
        let mut functions = inventory::iter::<Function>.into_iter().collect::<Vec<_>>();
        functions.sort_unstable_by_key(|function| (function.module, function.name));
        serde_json::to_vec(&Metadata {
            schema_version: 1,
            functions,
        })
        .unwrap_or_else(|error| format!(r#"{{"schema_version":1,"error":"{error}"}}"#).into_bytes())
        .into_boxed_slice()
    })
}

#[doc(hidden)]
pub mod __private {
    pub use std::panic::{AssertUnwindSafe, catch_unwind};

    pub use crate::{ABI_NULL_OUTPUT, ABI_OK};

    #[inline]
    pub unsafe fn write_output<T>(output: *mut T, value: T) -> i32 {
        if output.is_null() {
            return ABI_NULL_OUTPUT;
        }
        // SAFETY: The exported function's contract requires a valid, aligned pointer to writable T.
        unsafe { output.write(value) };
        ABI_OK
    }
}

#[macro_export]
macro_rules! setup {
    () => {
        #[unsafe(no_mangle)]
        pub extern "C" fn eqts_metadata_v1() -> $crate::MetadataSlice {
            match ::std::panic::catch_unwind($crate::metadata_json) {
                Ok(metadata) => $crate::MetadataSlice {
                    ptr: metadata.as_ptr(),
                    len: metadata.len(),
                },
                Err(_) => $crate::MetadataSlice {
                    ptr: ::std::ptr::null(),
                    len: 0,
                },
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inventory_has_versioned_metadata() {
        assert_eq!(metadata_json(), br#"{"schema_version":1,"functions":[]}"#);
    }

    #[test]
    fn null_output_is_rejected() {
        // SAFETY: A null pointer is explicitly accepted and rejected before dereference.
        let status = unsafe { __private::write_output::<u32>(std::ptr::null_mut(), 42) };
        assert_eq!(status, ABI_NULL_OUTPUT);
    }
}
