use std::sync::OnceLock;

pub use eqts_macros::export;
pub use inventory;
pub use serde;

#[derive(Debug, serde::Serialize)]
pub struct Function {
    pub name: &'static str,
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
    U8,
    U16,
    U32,
    I8,
    I16,
    I32,
    F32,
    F64,
}

inventory::collect!(Function);

#[must_use]
pub fn metadata_json() -> &'static str {
    static METADATA: OnceLock<String> = OnceLock::new();
    METADATA.get_or_init(|| {
        let functions = inventory::iter::<Function>.into_iter().collect::<Vec<_>>();
        serde_json::to_string(&functions)
            .unwrap_or_else(|error| format!(r#"{{"error":"{error}"}}"#))
    })
}

#[macro_export]
macro_rules! setup {
    () => {
        #[unsafe(no_mangle)]
        pub extern "C" fn eqts_metadata_json() -> *const ::std::ffi::c_char {
            static EQTS_METADATA: ::std::sync::OnceLock<::std::ffi::CString> =
                ::std::sync::OnceLock::new();
            EQTS_METADATA
                .get_or_init(|| {
                    ::std::ffi::CString::new(::eqts::metadata_json())
                        .unwrap_or_else(|_| ::std::ffi::CString::default())
                })
                .as_ptr()
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inventory_serializes_as_array() {
        assert_eq!(metadata_json(), "[]");
    }
}
