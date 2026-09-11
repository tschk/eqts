use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

pub use eqts_macros::{
    Enum, Record, async_export, callback, export, iterator, method, methods, object, stream,
    trait_export,
};
pub use inventory;
#[cfg(feature = "node-napi")]
pub use napi;
#[cfg(feature = "node-napi")]
pub use napi_derive;
pub use serde;
pub use serde_json;
#[cfg(feature = "wasm")]
pub use wasm_bindgen;
#[cfg(feature = "wasm")]
pub use {js_sys, serde_wasm_bindgen, wasm_bindgen_futures};

pub const ABI_OK: i32 = 0;
pub const ABI_PANIC: i32 = 1;
pub const ABI_NULL_OUTPUT: i32 = 2;
pub const ABI_INVALID_INPUT: i32 = 3;
pub const ABI_ENCODE_ERROR: i32 = 4;
pub const ABI_REACTIVE_PENDING: i32 = 10;
pub const ABI_REACTIVE_READY: i32 = 11;
pub const ABI_REACTIVE_DONE: i32 = 12;
pub const ABI_REACTIVE_CALLBACK: i32 = 13;
pub const ABI_UNKNOWN_HANDLE: i32 = 14;

#[derive(Debug, serde::Serialize)]
pub struct Metadata {
    pub schema_version: u32,
    pub capabilities: Capabilities,
    pub functions: Vec<Function>,
    pub method_sets: Vec<MethodSet>,
}

#[derive(Debug, serde::Serialize)]
pub struct MethodSet {
    pub rust_type: &'static str,
    pub methods: Vec<Method>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct Method {
    pub name: &'static str,
    pub parameters: Vec<Parameter>,
    pub result: Type,
    pub mutable: bool,
    pub asynchronous: bool,
}

#[derive(Debug, serde::Serialize)]
#[expect(clippy::struct_excessive_bools)]
pub struct Capabilities {
    pub owned_values: bool,
    pub objects: bool,
    pub async_functions: bool,
    pub callbacks: bool,
    pub traits: bool,
    pub streams: bool,
    pub iterators: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            owned_values: true,
            objects: true,
            async_functions: true,
            callbacks: true,
            traits: true,
            streams: true,
            iterators: true,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct Function {
    pub module: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
    pub parameters: Vec<Parameter>,
    pub abi: Abi,
    pub result: Type,
    pub kind: ExportKind,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExportKind {
    #[default]
    Function,
    Object {
        value: Type,
    },
    Trait {
        value: Type,
    },
    Async {
        value: Type,
    },
    Callback {
        value: Type,
        callback_parameter: &'static str,
    },
    Stream {
        item: Type,
    },
    Iterator {
        item: Type,
    },
}

#[derive(Clone, Debug, serde::Serialize)]
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
    Callback {
        value: Box<Type>,
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

pub enum ReactivePoll {
    Pending,
    Ready(serde_json::Value),
    Done,
    Callback(serde_json::Value),
}

#[derive(serde::Serialize)]
pub struct ReactiveJsPoll {
    pub status: i32,
    pub value: Option<serde_json::Value>,
}

pub struct Callback<T> {
    queue: std::sync::Arc<Mutex<VecDeque<serde_json::Value>>>,
    marker: std::marker::PhantomData<fn(T)>,
}

impl<T: EqtsValue> Callback<T> {
    #[expect(clippy::missing_errors_doc)]
    pub fn emit(&self, value: T) -> Result<(), String> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !queue.is_empty() {
            return Err("callback buffer is full".into());
        }
        queue.push_back(value.into_json()?);
        Ok(())
    }
}

pub trait ReactiveResource: Send + 'static {
    #[expect(clippy::missing_errors_doc)]
    fn poll(&mut self) -> Result<ReactivePoll, String>;
    fn cancel(&mut self) {}
}

pub trait EqtsMethods: Send + 'static {
    #[expect(clippy::missing_errors_doc)]
    fn invoke(
        &mut self,
        method: &str,
        arguments: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, String>;
    #[expect(clippy::missing_errors_doc)]
    fn invoke_async(
        &mut self,
        _method: &str,
        _arguments: Vec<serde_json::Value>,
    ) -> Result<u64, String> {
        Err("unknown async method".into())
    }
}

pub struct MethodRegistration {
    pub describe: fn() -> MethodSet,
}
inventory::collect!(MethodRegistration);

struct ReactiveEntry {
    resource: Arc<Mutex<Box<dyn ReactiveResource>>>,
    cancelled: bool,
    callbacks: Arc<Mutex<VecDeque<serde_json::Value>>>,
    invoke: Option<Arc<Mutex<Box<dyn EqtsMethods>>>>,
}

#[derive(Default)]
struct ReactiveRegistry {
    next: u64,
    entries: HashMap<u64, ReactiveEntry>,
}

fn reactive_registry() -> &'static Mutex<ReactiveRegistry> {
    static REGISTRY: OnceLock<Mutex<ReactiveRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(ReactiveRegistry {
            next: 1,
            entries: HashMap::new(),
        })
    })
}

fn lock_registry() -> std::sync::MutexGuard<'static, ReactiveRegistry> {
    reactive_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn allocate_handle(registry: &mut ReactiveRegistry) -> u64 {
    loop {
        let handle = registry.next;
        let Some(next) = registry.next.checked_add(1) else {
            panic!("eqts reactive handle space exhausted");
        };
        registry.next = next;
        if handle != 0 && !registry.entries.contains_key(&handle) {
            return handle;
        }
    }
}

fn insert_entry(
    resource: Box<dyn ReactiveResource>,
    callbacks: Arc<Mutex<VecDeque<serde_json::Value>>>,
    invoke: Option<Arc<Mutex<Box<dyn EqtsMethods>>>>,
) -> u64 {
    let mut registry = lock_registry();
    let handle = allocate_handle(&mut registry);
    registry.entries.insert(
        handle,
        ReactiveEntry {
            resource: Arc::new(Mutex::new(resource)),
            cancelled: false,
            callbacks,
            invoke,
        },
    );
    handle
}

#[must_use]
pub fn register_reactive(resource: impl ReactiveResource) -> u64 {
    insert_entry(
        Box::new(resource),
        Arc::new(Mutex::new(VecDeque::with_capacity(1))),
        None,
    )
}

struct IdleResource;
impl ReactiveResource for IdleResource {
    fn poll(&mut self) -> Result<ReactivePoll, String> {
        Ok(ReactivePoll::Pending)
    }
}

#[must_use]
pub fn register_object(value: impl EqtsMethods) -> u64 {
    insert_entry(
        Box::new(IdleResource),
        Arc::new(Mutex::new(VecDeque::with_capacity(1))),
        Some(Arc::new(Mutex::new(Box::new(value)))),
    )
}

#[doc(hidden)]
pub fn invoke_handle(
    handle: u64,
    method: &str,
    arguments: Vec<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let mut registry = lock_registry();
    let entry = registry
        .entries
        .get_mut(&handle)
        .ok_or_else(|| "unknown reactive handle".to_string())?;
    let invoke = entry
        .invoke
        .as_ref()
        .ok_or_else(|| "handle has no methods".to_string())?
        .clone();
    drop(registry);
    invoke
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .invoke(method, arguments)
}

#[doc(hidden)]
pub fn invoke_handle_async(
    handle: u64,
    method: &str,
    arguments: Vec<serde_json::Value>,
) -> Result<u64, String> {
    let registry = lock_registry();
    let entry = registry
        .entries
        .get(&handle)
        .ok_or_else(|| "unknown reactive handle".to_string())?;
    let invoke = entry
        .invoke
        .as_ref()
        .ok_or_else(|| "handle has no methods".to_string())?
        .clone();
    drop(registry);
    invoke
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .invoke_async(method, arguments)
}

#[must_use]
pub fn register_callback<T: EqtsValue>(
    factory: impl FnOnce(Callback<T>) -> Box<dyn ReactiveResource>,
) -> u64 {
    let queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
    let resource = factory(Callback {
        queue: Arc::clone(&queue),
        marker: std::marker::PhantomData,
    });
    insert_entry(resource, queue, None)
}

struct FutureResource {
    state: std::sync::Arc<Mutex<Option<Result<serde_json::Value, String>>>>,
}

impl ReactiveResource for FutureResource {
    fn poll(&mut self) -> Result<ReactivePoll, String> {
        match self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            None => Ok(ReactivePoll::Pending),
            Some(Ok(value)) => Ok(ReactivePoll::Ready(value)),
            Some(Err(error)) => Err(error),
        }
    }
}

#[must_use]
pub fn register_future<T, F>(future: F) -> u64
where
    T: EqtsValue + Send,
    F: Future<Output = T> + Send + 'static,
{
    let state = Arc::new(Mutex::new(None));
    let task_state = Arc::clone(&state);
    run_future(async move {
        let value = future.await.into_json();
        *task_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(value);
    });
    register_reactive(FutureResource { state })
}

#[cfg(not(target_arch = "wasm32"))]
fn run_future(future: impl Future<Output = ()> + Send + 'static) {
    std::thread::spawn(move || futures::executor::block_on(future));
}

#[cfg(target_arch = "wasm32")]
fn run_future(future: impl Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(future);
}

#[expect(clippy::missing_errors_doc)]
pub fn enqueue_callback(handle: u64, value: impl EqtsValue) -> Result<(), String> {
    let callbacks = {
        let registry = lock_registry();
        let entry = registry
            .entries
            .get(&handle)
            .ok_or_else(|| "unknown reactive handle".to_string())?;
        Arc::clone(&entry.callbacks)
    };
    let encoded = value.into_json()?;
    let mut queue = callbacks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !queue.is_empty() {
        return Err("callback buffer is full".into());
    }
    queue.push_back(encoded);
    Ok(())
}

#[doc(hidden)]
pub fn cancel_reactive(handle: u64) -> i32 {
    let resource = {
        let mut registry = lock_registry();
        let Some(entry) = registry.entries.get_mut(&handle) else {
            return ABI_OK;
        };
        if entry.cancelled {
            return ABI_OK;
        }
        entry.cancelled = true;
        Arc::clone(&entry.resource)
    };
    resource
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .cancel();
    ABI_OK
}

#[doc(hidden)]
#[expect(clippy::must_use_candidate)]
pub fn dispose_reactive(handle: u64) -> i32 {
    lock_registry().entries.remove(&handle);
    ABI_OK
}

#[doc(hidden)]
pub fn poll_reactive(handle: u64) -> Result<(i32, Option<Vec<u8>>), i32> {
    let (resource, callbacks, cancelled) = {
        let registry = lock_registry();
        let entry = registry.entries.get(&handle).ok_or(ABI_UNKNOWN_HANDLE)?;
        (
            Arc::clone(&entry.resource),
            Arc::clone(&entry.callbacks),
            entry.cancelled,
        )
    };
    let callback = callbacks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop_front();
    if let Some(value) = callback {
        return serde_json::to_vec(&value)
            .map(|value| (ABI_REACTIVE_CALLBACK, Some(value)))
            .map_err(|_| ABI_ENCODE_ERROR);
    }
    if cancelled {
        return Ok((ABI_REACTIVE_DONE, None));
    }
    let poll = resource
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .poll();
    match poll {
        Ok(ReactivePoll::Pending) => Ok((ABI_REACTIVE_PENDING, None)),
        Ok(ReactivePoll::Done) => Ok((ABI_REACTIVE_DONE, None)),
        Ok(ReactivePoll::Ready(value)) => serde_json::to_vec(&value)
            .map(|value| (ABI_REACTIVE_READY, Some(value)))
            .map_err(|_| ABI_ENCODE_ERROR),
        Ok(ReactivePoll::Callback(value)) => serde_json::to_vec(&value)
            .map(|value| (ABI_REACTIVE_CALLBACK, Some(value)))
            .map_err(|_| ABI_ENCODE_ERROR),
        Err(error) => Ok((ABI_ENCODE_ERROR, Some(error.into_bytes()))),
    }
}

fn capabilities_from(functions: &[Function], method_sets: &[MethodSet]) -> Capabilities {
    let mut capabilities = Capabilities {
        owned_values: true,
        objects: false,
        async_functions: false,
        callbacks: false,
        traits: false,
        streams: false,
        iterators: false,
    };
    for function in functions {
        match function.kind {
            ExportKind::Function => {}
            ExportKind::Object { .. } => capabilities.objects = true,
            ExportKind::Trait { .. } => capabilities.traits = true,
            ExportKind::Async { .. } => capabilities.async_functions = true,
            ExportKind::Callback { .. } => capabilities.callbacks = true,
            ExportKind::Stream { .. } => capabilities.streams = true,
            ExportKind::Iterator { .. } => capabilities.iterators = true,
        }
    }
    if method_sets
        .iter()
        .any(|set| set.methods.iter().any(|method| method.asynchronous))
    {
        capabilities.async_functions = true;
    }
    capabilities
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
        let mut method_sets = inventory::iter::<MethodRegistration>
            .into_iter()
            .map(|registration| (registration.describe)())
            .collect::<Vec<_>>();
        method_sets.sort_unstable_by_key(|set| set.rust_type);
        let capabilities = capabilities_from(&functions, &method_sets);
        serde_json::to_vec(&Metadata {
            schema_version: 3,
            capabilities,
            functions,
            method_sets,
        })
        .unwrap_or_else(|error| format!(r#"{{"schema_version":3,"error":"{error}"}}"#).into_bytes())
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

        #[unsafe(no_mangle)]
        pub extern "C" fn eqts_handle_dispose_v1(handle: u64) -> i32 {
            match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| $crate::dispose_reactive(handle))) {
                Ok(status) => status,
                Err(_) => $crate::ABI_PANIC,
            }
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn eqts_reactive_cancel_v1(handle: u64) -> i32 {
            match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| $crate::cancel_reactive(handle))) {
                Ok(status) => status,
                Err(_) => $crate::ABI_PANIC,
            }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn eqts_reactive_poll_v1(handle: u64, output: *mut $crate::OwnedBuffer) -> i32 {
            if output.is_null() { return $crate::ABI_NULL_OUTPUT; }
            match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| $crate::poll_reactive(handle))) {
                Ok(Ok((status, bytes))) => {
                    let buffer = bytes.map_or_else($crate::OwnedBuffer::empty, $crate::OwnedBuffer::from_bytes);
                    // SAFETY: caller provides a valid writable output pointer.
                    unsafe { output.write(buffer) };
                    status
                }
                Ok(Err(status)) => status,
                Err(_) => $crate::ABI_PANIC,
            }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn eqts_handle_invoke_v1(handle: u64, input: *const u8, len: usize, output: *mut $crate::OwnedBuffer) -> i32 {
            if output.is_null() || (input.is_null() && len != 0) { return $crate::ABI_NULL_OUTPUT; }
            let operation = ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| -> ::std::result::Result<::std::vec::Vec<u8>, (i32, ::std::string::String)> {
                let input = if len == 0 { &b"{}"[..] } else { unsafe { std::slice::from_raw_parts(input, len) } };
                let value: ::eqts::__private::Value = ::eqts::__private::from_slice(input).map_err(|error| ($crate::ABI_INVALID_INPUT, error.to_string()))?;
                let Some(method) = value.get("method").and_then(::eqts::__private::Value::as_str) else { return Err(($crate::ABI_INVALID_INPUT, "invalid input".into())); };
                let arguments = value.get("arguments").and_then(::eqts::__private::Value::as_array).cloned().unwrap_or_default();
                $crate::invoke_handle(handle, method, arguments).and_then(|value| ::eqts::__private::to_vec(&value).map_err(|error| error.to_string())).map_err(|error| ($crate::ABI_INVALID_INPUT, error))
            }));
            match operation {
                Ok(Ok(bytes)) => { unsafe { output.write($crate::OwnedBuffer::from_bytes(bytes)) }; $crate::ABI_OK }
                Ok(Err((status, error))) => { unsafe { output.write($crate::OwnedBuffer::from_bytes(error.into_bytes())) }; status }
                Err(_) => { unsafe { output.write($crate::OwnedBuffer::empty()) }; $crate::ABI_PANIC }
            }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn eqts_handle_invoke_async_v1(handle: u64, input: *const u8, len: usize, output: *mut u64) -> i32 {
            if output.is_null() || (input.is_null() && len != 0) { return $crate::ABI_NULL_OUTPUT; }
            let operation = ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| -> ::std::result::Result<u64, i32> {
                let input = if len == 0 { &b"{}"[..] } else { unsafe { std::slice::from_raw_parts(input, len) } };
                let value: ::eqts::__private::Value = ::eqts::__private::from_slice(input).map_err(|_| $crate::ABI_INVALID_INPUT)?;
                let Some(method) = value.get("method").and_then(::eqts::__private::Value::as_str) else { return Err($crate::ABI_INVALID_INPUT); };
                let arguments = value.get("arguments").and_then(::eqts::__private::Value::as_array).cloned().unwrap_or_default();
                $crate::invoke_handle_async(handle, method, arguments).map_err(|_| $crate::ABI_INVALID_INPUT)
            }));
            match operation {
                Ok(Ok(future)) => unsafe { ::eqts::__private::write_output(output, future) },
                Ok(Err(status)) => status,
                Err(_) => $crate::ABI_PANIC,
            }
        }

        #[cfg(all(feature = "node-napi", not(feature = "wasm")))]
        #[::eqts::napi_derive::napi(js_name = "eqtsHandleInvoke")]
        pub fn __eqts_napi_handle_invoke(handle: ::eqts::napi::bindgen_prelude::BigInt, method: String, arguments: Vec<::eqts::__private::Value>) -> ::eqts::napi::Result<::eqts::__private::Value> {
            let (_, handle, lossless) = handle.get_u64(); if !lossless { return Err(::eqts::napi::Error::from_reason("handle out of range")); }
            match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| $crate::invoke_handle(handle, &method, arguments))) {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(::eqts::napi::Error::from_reason(error)),
                Err(_) => Err(::eqts::napi::Error::from_reason("eqts panic")),
            }
        }

        #[cfg(all(feature = "node-napi", not(feature = "wasm")))]
        #[::eqts::napi_derive::napi(js_name = "eqtsHandleInvokeAsync")]
        pub fn __eqts_napi_handle_invoke_async(handle: ::eqts::napi::bindgen_prelude::BigInt, method: String, arguments: Vec<::eqts::__private::Value>) -> ::eqts::napi::Result<::eqts::napi::bindgen_prelude::BigInt> {
            let (_, handle, lossless) = handle.get_u64(); if !lossless { return Err(::eqts::napi::Error::from_reason("handle out of range")); }
            match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| $crate::invoke_handle_async(handle, &method, arguments))) {
                Ok(Ok(future)) => Ok(future.into()),
                Ok(Err(error)) => Err(::eqts::napi::Error::from_reason(error)),
                Err(_) => Err(::eqts::napi::Error::from_reason("eqts panic")),
            }
        }

        #[cfg(all(feature = "wasm", not(feature = "node-napi")))]
        #[::eqts::wasm_bindgen::prelude::wasm_bindgen(js_name = "eqtsHandleInvoke")]
        pub fn __eqts_wasm_handle_invoke(handle: u64, method: String, arguments: ::eqts::wasm_bindgen::JsValue) -> Result<::eqts::wasm_bindgen::JsValue, ::eqts::wasm_bindgen::JsValue> {
            match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| -> Result<_, String> {
                let arguments = ::eqts::serde_wasm_bindgen::from_value(arguments).map_err(|error| error.to_string())?;
                $crate::invoke_handle(handle, &method, arguments)
            })) {
                Ok(Ok(value)) => {
                    let serializer = ::eqts::serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
                    ::eqts::serde::Serialize::serialize(&value, &serializer).map_err(|error| ::eqts::js_sys::Error::new(&error.to_string()).into())
                }
                Ok(Err(error)) => Err(::eqts::js_sys::Error::new(&error).into()),
                Err(_) => Err(::eqts::js_sys::Error::new("eqts panic").into()),
            }
        }

        #[cfg(all(feature = "wasm", not(feature = "node-napi")))]
        #[::eqts::wasm_bindgen::prelude::wasm_bindgen(js_name = "eqtsHandleInvokeAsync")]
        pub fn __eqts_wasm_handle_invoke_async(handle: u64, method: String, arguments: ::eqts::wasm_bindgen::JsValue) -> Result<u64, ::eqts::wasm_bindgen::JsValue> {
            match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| -> Result<_, String> {
                let arguments = ::eqts::serde_wasm_bindgen::from_value(arguments).map_err(|error| error.to_string())?;
                $crate::invoke_handle_async(handle, &method, arguments)
            })) {
                Ok(Ok(future)) => Ok(future),
                Ok(Err(error)) => Err(::eqts::js_sys::Error::new(&error).into()),
                Err(_) => Err(::eqts::js_sys::Error::new("eqts panic").into()),
            }
        }

        #[cfg(all(feature = "node-napi", not(feature = "wasm")))]
        #[::eqts::napi_derive::napi(js_name = "eqtsHandleDispose")]
        pub fn __eqts_napi_handle_dispose(handle: ::eqts::napi::bindgen_prelude::BigInt) -> ::eqts::napi::Result<()> {
            let (_, handle, lossless) = handle.get_u64();
            if !lossless { return Err(::eqts::napi::Error::from_reason("reactive handle is out of range")); }
            $crate::dispose_reactive(handle);
            Ok(())
        }

        #[cfg(all(feature = "node-napi", not(feature = "wasm")))]
        #[::eqts::napi_derive::napi(js_name = "eqtsReactiveCancel")]
        pub fn __eqts_napi_reactive_cancel(handle: ::eqts::napi::bindgen_prelude::BigInt) -> ::eqts::napi::Result<()> {
            let (_, handle, lossless) = handle.get_u64();
            if !lossless { return Err(::eqts::napi::Error::from_reason("reactive handle is out of range")); }
            match $crate::cancel_reactive(handle) { $crate::ABI_OK => Ok(()), _ => Err(::eqts::napi::Error::from_reason("unknown reactive handle")) }
        }

        #[cfg(all(feature = "node-napi", not(feature = "wasm")))]
        #[::eqts::napi_derive::napi(js_name = "eqtsReactivePoll")]
        pub fn __eqts_napi_reactive_poll(handle: ::eqts::napi::bindgen_prelude::BigInt) -> ::eqts::napi::Result<::eqts::__private::Value> {
            let (_, handle, lossless) = handle.get_u64();
            if !lossless { return Err(::eqts::napi::Error::from_reason("reactive handle is out of range")); }
            match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| $crate::poll_reactive(handle))) {
                Ok(Ok((status, bytes))) => {
                    let value = bytes.map(|bytes| ::eqts::__private::from_slice(&bytes)).transpose().map_err(|error| ::eqts::napi::Error::from_reason(error.to_string()))?;
                    Ok(::eqts::serde::Serialize::serialize(&$crate::ReactiveJsPoll { status, value }, ::eqts::serde_json::value::Serializer).expect("JSON value serialization cannot fail"))
                }
                Ok(Err(_)) => Err(::eqts::napi::Error::from_reason("unknown reactive handle")),
                Err(_) => Err(::eqts::napi::Error::from_reason("eqts panic")),
            }
        }

        #[cfg(all(feature = "wasm", not(feature = "node-napi")))]
        #[::eqts::wasm_bindgen::prelude::wasm_bindgen(js_name = "eqtsHandleDispose")]
        pub fn __eqts_wasm_handle_dispose(handle: u64) { $crate::dispose_reactive(handle); }

        #[cfg(all(feature = "wasm", not(feature = "node-napi")))]
        #[::eqts::wasm_bindgen::prelude::wasm_bindgen(js_name = "eqtsReactiveCancel")]
        pub fn __eqts_wasm_reactive_cancel(handle: u64) -> Result<(), ::eqts::wasm_bindgen::JsValue> {
            match $crate::cancel_reactive(handle) { $crate::ABI_OK => Ok(()), _ => Err(::eqts::js_sys::Error::new("unknown reactive handle").into()) }
        }

        #[cfg(all(feature = "wasm", not(feature = "node-napi")))]
        #[::eqts::wasm_bindgen::prelude::wasm_bindgen(js_name = "eqtsReactivePoll")]
        pub fn __eqts_wasm_reactive_poll(handle: u64) -> Result<::eqts::wasm_bindgen::JsValue, ::eqts::wasm_bindgen::JsValue> {
            match ::eqts::__private::catch_unwind(::eqts::__private::AssertUnwindSafe(|| $crate::poll_reactive(handle))) {
                Ok(Ok((status, bytes))) => {
                    let value = bytes.map(|bytes| ::eqts::__private::from_slice(&bytes)).transpose().map_err(|error| ::eqts::js_sys::Error::new(&error.to_string()))?;
                    let serializer = ::eqts::serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
                    ::eqts::serde::Serialize::serialize(&$crate::ReactiveJsPoll { status, value }, &serializer).map_err(|error| ::eqts::js_sys::Error::new(&error.to_string()).into())
                }
                Ok(Err(_)) => Err(::eqts::js_sys::Error::new("unknown reactive handle").into()),
                Err(_) => Err(::eqts::js_sys::Error::new("eqts panic").into()),
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inventory_has_versioned_metadata() {
        let metadata: serde_json::Value =
            serde_json::from_slice(metadata_json()).expect("metadata must be valid JSON");
        assert_eq!(metadata["schema_version"], 3);
        assert_eq!(metadata["capabilities"]["owned_values"], true);
        assert_eq!(metadata["capabilities"]["async_functions"], false);
        assert_eq!(metadata["capabilities"]["objects"], false);
        assert_eq!(metadata["capabilities"]["callbacks"], false);
        assert_eq!(metadata["capabilities"]["traits"], false);
        assert_eq!(metadata["capabilities"]["streams"], false);
        assert_eq!(metadata["capabilities"]["iterators"], false);
        assert_eq!(metadata["functions"], serde_json::json!([]));
    }

    fn dummy_entry() -> ReactiveEntry {
        ReactiveEntry {
            resource: Arc::new(Mutex::new(Box::new(IdleResource))),
            cancelled: false,
            callbacks: Arc::new(Mutex::new(VecDeque::new())),
            invoke: None,
        }
    }

    #[test]
    fn handle_ids_skip_live_entries_and_do_not_wrap() {
        let mut registry = ReactiveRegistry {
            next: 1,
            entries: HashMap::new(),
        };
        registry.entries.insert(1, dummy_entry());
        registry.entries.insert(2, dummy_entry());
        registry.next = 1;
        assert_eq!(allocate_handle(&mut registry), 3);
        registry.next = u64::MAX;
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            allocate_handle(&mut registry);
        }));
        assert!(panicked.is_err());
    }

    #[test]
    fn null_output_is_rejected() {
        // SAFETY: A null pointer is explicitly accepted and rejected before dereference.
        let status = unsafe { __private::write_output::<u32>(std::ptr::null_mut(), 42) };
        assert_eq!(status, ABI_NULL_OUTPUT);
    }

    #[test]
    fn u64_json_requires_a_decimal_string() {
        assert_eq!(
            u64::from_json(serde_json::json!("42")).expect("decimal string"),
            42
        );
        assert!(u64::from_json(serde_json::json!(42)).is_err());
        assert!(u64::from_json(serde_json::json!("")).is_err());
        assert!(u64::from_json(serde_json::json!("-1")).is_err());
    }

    #[test]
    fn vec_json_requires_an_array() {
        assert_eq!(
            Vec::<u32>::from_json(serde_json::json!([1, 2])).expect("array"),
            vec![1, 2]
        );
        assert!(Vec::<u32>::from_json(serde_json::json!("no")).is_err());
    }

    #[test]
    fn result_json_requires_ok_or_error() {
        assert_eq!(
            Result::<u32, String>::from_json(serde_json::json!({"ok": 7})).expect("ok"),
            Ok(7)
        );
        assert_eq!(
            Result::<u32, String>::from_json(serde_json::json!({"error": "no"})).expect("error"),
            Err("no".into())
        );
        assert!(Result::<u32, String>::from_json(serde_json::json!({})).is_err());
        assert!(Result::<u32, String>::from_json(serde_json::json!([])).is_err());
    }
}
