use std::sync::OnceLock;

pub use eqts_macros::{Enum, Record, export};
pub use inventory;
#[cfg(feature = "node-napi")]
pub use napi_derive;
pub use serde;
#[cfg(feature = "wasm")]
pub use wasm_bindgen;

pub const ABI_OK: i32 = 0;
pub const ABI_PANIC: i32 = 1;
pub const ABI_NULL_OUTPUT: i32 = 2;
pub const ABI_INVALID_INPUT: i32 = 3;
pub const ABI_ENCODE_ERROR: i32 = 4;

#[derive(Debug, serde::Serialize)]
pub struct Metadata {
    pub schema_version: u32,
    pub functions: Vec<Function>,
}

#[derive(Debug, serde::Serialize)]
pub struct Function {
    pub module: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
    pub parameters: Vec<Parameter>,
    pub abi: Abi,
    pub result: Type,
}

#[derive(Debug, serde::Serialize)]
pub struct Parameter {
    pub name: &'static str,
    pub ty: Type,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Abi {
    Scalar,
    Json,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Type {
    Scalar {
        scalar: Scalar,
    },
    String,
    Bytes,
    Vec {
        value: Box<Type>,
    },
    Option {
        value: Box<Type>,
    },
    Result {
        ok: Box<Type>,
        error: Box<Type>,
    },
    Record {
        name: &'static str,
        fields: Vec<Field>,
    },
    Enum {
        name: &'static str,
        variants: &'static [&'static str],
    },
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Field {
    pub name: &'static str,
    pub ty: Type,
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

pub trait EqtsValue: Sized {
    fn schema() -> Type;
    #[expect(clippy::missing_errors_doc)]
    fn from_json(value: serde_json::Value) -> Result<Self, String>;
    #[expect(clippy::missing_errors_doc)]
    fn into_json(self) -> Result<serde_json::Value, String>;
}

macro_rules! scalar_value {
    ($type:ty, $scalar:ident) => {
        impl EqtsValue for $type {
            fn schema() -> Type {
                Type::Scalar {
                    scalar: Scalar::$scalar,
                }
            }

            fn from_json(value: serde_json::Value) -> Result<Self, String> {
                serde_json::from_value(value).map_err(|error| error.to_string())
            }

            fn into_json(self) -> Result<serde_json::Value, String> {
                serde_json::to_value(self).map_err(|error| error.to_string())
            }
        }
    };
}

scalar_value!(bool, Bool);
scalar_value!(u8, U8);
scalar_value!(u16, U16);
scalar_value!(u32, U32);
scalar_value!(i8, I8);
scalar_value!(i16, I16);
scalar_value!(i32, I32);
scalar_value!(f32, F32);
scalar_value!(f64, F64);
scalar_value!((), Void);

macro_rules! integer64_value {
    ($type:ty, $scalar:ident) => {
        impl EqtsValue for $type {
            fn schema() -> Type {
                Type::Scalar {
                    scalar: Scalar::$scalar,
                }
            }
            fn from_json(value: serde_json::Value) -> Result<Self, String> {
                let serde_json::Value::String(value) = value else {
                    return Err("expected decimal string".into());
                };
                value
                    .parse()
                    .map_err(|error: std::num::ParseIntError| error.to_string())
            }
            fn into_json(self) -> Result<serde_json::Value, String> {
                Ok(serde_json::Value::String(self.to_string()))
            }
        }
    };
}

integer64_value!(u64, U64);
integer64_value!(i64, I64);

impl EqtsValue for String {
    fn schema() -> Type {
        Type::String
    }
    fn from_json(value: serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }
    fn into_json(self) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::String(self))
    }
}

impl<T: EqtsValue> EqtsValue for Vec<T> {
    fn schema() -> Type {
        Type::Vec {
            value: Box::new(T::schema()),
        }
    }
    fn from_json(value: serde_json::Value) -> Result<Self, String> {
        let serde_json::Value::Array(values) = value else {
            return Err("expected array".into());
        };
        values.into_iter().map(T::from_json).collect()
    }
    fn into_json(self) -> Result<serde_json::Value, String> {
        self.into_iter()
            .map(T::into_json)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array)
    }
}

impl<T: EqtsValue> EqtsValue for Option<T> {
    fn schema() -> Type {
        Type::Option {
            value: Box::new(T::schema()),
        }
    }
    fn from_json(value: serde_json::Value) -> Result<Self, String> {
        if value.is_null() {
            Ok(None)
        } else {
            T::from_json(value).map(Some)
        }
    }
    fn into_json(self) -> Result<serde_json::Value, String> {
        self.map_or(Ok(serde_json::Value::Null), T::into_json)
    }
}

impl<T: EqtsValue, E: EqtsValue> EqtsValue for Result<T, E> {
    fn schema() -> Type {
        Type::Result {
            ok: Box::new(T::schema()),
            error: Box::new(E::schema()),
        }
    }
    fn from_json(value: serde_json::Value) -> Result<Self, String> {
        let serde_json::Value::Object(mut value) = value else {
            return Err("expected result object".into());
        };
        if let Some(value) = value.remove("ok") {
            T::from_json(value).map(Ok)
        } else if let Some(value) = value.remove("error") {
            E::from_json(value).map(Err)
        } else {
            Err("expected ok or error".into())
        }
    }
    fn into_json(self) -> Result<serde_json::Value, String> {
        let (key, value) = match self {
            Ok(value) => ("ok", T::into_json(value)?),
            Err(error) => ("error", E::into_json(error)?),
        };
        Ok(serde_json::Value::Object(
            [(key.into(), value)].into_iter().collect(),
        ))
    }
}

#[repr(C)]
pub struct OwnedBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl OwnedBuffer {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }

    #[must_use]
    pub fn from_bytes(mut bytes: Vec<u8>) -> Self {
        let buffer = Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            capacity: bytes.capacity(),
        };
        std::mem::forget(bytes);
        buffer
    }
}

#[repr(C)]
pub struct MetadataSlice {
    pub ptr: *const u8,
    pub len: usize,
}

pub struct FunctionRegistration {
    pub describe: fn() -> Function,
}

inventory::collect!(FunctionRegistration);

#[must_use]
pub fn metadata_json() -> &'static [u8] {
    static METADATA: OnceLock<Box<[u8]>> = OnceLock::new();
    METADATA.get_or_init(|| {
        let mut functions = inventory::iter::<FunctionRegistration>
            .into_iter()
            .map(|registration| (registration.describe)())
            .collect::<Vec<_>>();
        functions.sort_unstable_by_key(|function| (function.module, function.name));
        serde_json::to_vec(&Metadata {
            schema_version: 2,
            functions,
        })
        .unwrap_or_else(|error| format!(r#"{{"schema_version":2,"error":"{error}"}}"#).into_bytes())
        .into_boxed_slice()
    })
}

#[doc(hidden)]
pub mod __private {
    pub use std::panic::{AssertUnwindSafe, catch_unwind};

    pub use crate::{ABI_NULL_OUTPUT, ABI_OK};
    pub use serde_json::{Value, from_slice, to_vec};

    #[inline]
    #[doc = "# Safety"]
    #[doc = "`output` must be null or valid, aligned, and writable for one `T`."]
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

        #[unsafe(no_mangle)]
        #[doc = "# Safety"]
        #[doc = "The pointer, length, and capacity must come from one eqts output buffer and be released exactly once."]
        pub unsafe extern "C" fn eqts_buffer_free_v1(ptr: *mut u8, len: usize, capacity: usize) {
            if !ptr.is_null() {
                // SAFETY: components originate from OwnedBuffer::from_bytes and must be returned exactly once.
                unsafe { drop(::std::vec::Vec::from_raw_parts(ptr, len, capacity)) };
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inventory_has_versioned_metadata() {
        assert_eq!(metadata_json(), br#"{"schema_version":2,"functions":[]}"#);
    }

    #[test]
    fn null_output_is_rejected() {
        // SAFETY: A null pointer is explicitly accepted and rejected before dereference.
        let status = unsafe { __private::write_output::<u32>(std::ptr::null_mut(), 42) };
        assert_eq!(status, ABI_NULL_OUTPUT);
    }
}
