use std::collections::VecDeque;

#[derive(Clone)]
pub struct Values(VecDeque<u32>);

#[eqts::methods]
impl Values {
    #[eqts::method]
    #[expect(clippy::missing_panics_doc)]
    pub fn push(&mut self, value: u32) -> u32 {
        self.0.push_back(value);
        u32::try_from(self.0.len()).expect("test queue length fits u32")
    }

    #[eqts::method]
    #[expect(clippy::missing_panics_doc, clippy::unused_async)]
    pub async fn size(&self) -> u32 {
        u32::try_from(self.0.len()).expect("test queue length fits u32")
    }
}

impl eqts::ReactiveResource for Values {
    fn poll(&mut self) -> Result<eqts::ReactivePoll, String> {
        Ok(self
            .0
            .pop_front()
            .map_or(eqts::ReactivePoll::Done, |value| {
                eqts::ReactivePoll::Ready(serde_json::json!(value))
            }))
    }
}

eqts::setup!();

#[eqts::stream(u32)]
#[must_use]
pub fn numbers() -> Values {
    Values([1, 2].into())
}

#[eqts::async_export(String)]
#[must_use]
#[expect(clippy::unused_async)]
pub async fn delayed_name() -> String {
    "ready".into()
}

#[eqts::callback(String)]
#[must_use]
#[expect(clippy::missing_panics_doc, clippy::needless_pass_by_value)]
pub fn events(callback: eqts::Callback<String>) -> Values {
    callback
        .emit("created".into())
        .expect("callback queue available");
    Values(VecDeque::new())
}

#[eqts::object(String)]
#[must_use]
pub fn state() -> Values {
    Values(VecDeque::new())
}

#[eqts::iterator(u32)]
#[must_use]
pub fn range(end: u32) -> Values {
    Values((0..end).collect())
}

#[eqts::stream(u32)]
#[must_use]
#[expect(clippy::missing_panics_doc)]
pub fn exploding() -> Values {
    panic!("ctor boom");
}

struct PanicPoll;

impl eqts::ReactiveResource for PanicPoll {
    fn poll(&mut self) -> Result<eqts::ReactivePoll, String> {
        panic!("poll boom");
    }
}

struct PanicMethods;

impl eqts::EqtsMethods for PanicMethods {
    fn invoke(
        &mut self,
        _method: &str,
        _arguments: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        panic!("invoke boom");
    }
}

struct Reenter;

impl eqts::ReactiveResource for Reenter {
    fn poll(&mut self) -> Result<eqts::ReactivePoll, String> {
        let nested = eqts::register_reactive(Values(VecDeque::new()));
        assert_eq!(eqts::dispose_reactive(nested), eqts::ABI_OK);
        Ok(eqts::ReactivePoll::Done)
    }
}

#[test]
fn reactive_poll_is_demand_driven_and_disposal_is_idempotent() {
    let handle = eqts::register_reactive(Values([1, 2].into()));
    let mut output = eqts::OwnedBuffer::empty();
    // SAFETY: output points to live writable storage.
    assert_eq!(
        unsafe { eqts_reactive_poll_v1(handle, &raw mut output) },
        eqts::ABI_REACTIVE_READY
    );
    // SAFETY: allocation came from the poll wrapper and is consumed once.
    unsafe { drop(Vec::from_raw_parts(output.ptr, output.len, output.capacity)) };
    assert_eq!(eqts_handle_dispose_v1(handle), eqts::ABI_OK);
    assert_eq!(eqts_handle_dispose_v1(handle), eqts::ABI_OK);
}

#[test]
fn callback_queue_has_one_item_backpressure() {
    let handle = eqts::register_reactive(Values(VecDeque::new()));
    eqts::enqueue_callback(handle, "first".to_string()).expect("first callback fits");
    assert!(eqts::enqueue_callback(handle, "second".to_string()).is_err());
    assert_eq!(eqts_reactive_cancel_v1(handle), eqts::ABI_OK);
    assert_eq!(eqts_reactive_cancel_v1(handle), eqts::ABI_OK);
}

#[test]
fn reactive_annotations_emit_handles_and_kinds() {
    let mut handle = 0;
    // SAFETY: output points to live writable storage.
    assert_eq!(
        unsafe { eqts_numbers(std::ptr::null(), 0, &raw mut handle) },
        eqts::ABI_OK
    );
    assert_ne!(handle, 0);
    let metadata: serde_json::Value =
        serde_json::from_slice(eqts::metadata_json()).expect("metadata must be JSON");
    let kinds = metadata["functions"]
        .as_array()
        .expect("functions must be an array")
        .iter()
        .map(|function| function["kind"]["kind"].as_str().expect("kind string"))
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            "async", "callback", "stream", "stream", "iterator", "object"
        ]
    );
    assert_eq!(metadata["capabilities"]["owned_values"], true);
    assert_eq!(metadata["capabilities"]["async_functions"], true);
    assert_eq!(metadata["capabilities"]["callbacks"], true);
    assert_eq!(metadata["capabilities"]["streams"], true);
    assert_eq!(metadata["capabilities"]["iterators"], true);
    assert_eq!(metadata["capabilities"]["objects"], true);
    assert_eq!(metadata["capabilities"]["traits"], false);
    let input = b"[2]";
    // SAFETY: input and output remain valid for the call.
    assert_eq!(
        unsafe { eqts_range(input.as_ptr(), input.len(), &raw mut handle) },
        eqts::ABI_OK
    );
    let object = state();
    let object_handle = eqts::register_object(object);
    assert_eq!(
        eqts::invoke_handle(object_handle, "push", vec![serde_json::json!(7)])
            .expect("method works"),
        serde_json::json!(1)
    );
    assert_eq!(metadata["method_sets"][0]["methods"][0]["name"], "push");
    let future =
        eqts::invoke_handle_async(object_handle, "size", vec![]).expect("async method starts");
    assert_ne!(future, 0);
    let mut ready = None;
    for _ in 0..100 {
        match eqts::poll_reactive(future).expect("future handle exists") {
            (eqts::ABI_REACTIVE_READY, value) => {
                ready = value;
                break;
            }
            (eqts::ABI_REACTIVE_PENDING, _) => std::thread::yield_now(),
            (status, _) => panic!("unexpected async status {status}"),
        }
    }
    assert_eq!(ready.expect("future completes"), b"1".as_slice());
    let cancelled =
        eqts::invoke_handle_async(object_handle, "size", vec![]).expect("async method starts");
    assert_eq!(eqts::cancel_reactive(cancelled), eqts::ABI_OK);
    assert_eq!(
        eqts::poll_reactive(cancelled).expect("cancelled handle").0,
        eqts::ABI_REACTIVE_DONE
    );
    assert_eq!(eqts::dispose_reactive(cancelled), eqts::ABI_OK);
    assert_eq!(
        metadata["method_sets"][0]["methods"][1]["asynchronous"],
        true
    );
}

#[test]
fn reactive_abi_catches_panics() {
    let mut handle = 0;
    // SAFETY: output points to live writable storage.
    assert_eq!(
        unsafe { eqts_exploding(std::ptr::null(), 0, &raw mut handle) },
        eqts::ABI_PANIC
    );

    let poll_handle = eqts::register_reactive(PanicPoll);
    let mut output = eqts::OwnedBuffer::empty();
    // SAFETY: output points to live writable storage.
    assert_eq!(
        unsafe { eqts_reactive_poll_v1(poll_handle, &raw mut output) },
        eqts::ABI_PANIC
    );

    let object = eqts::register_object(PanicMethods);
    let input = br#"{"method":"push","arguments":[]}"#;
    let mut invoke_output = eqts::OwnedBuffer::empty();
    // SAFETY: input and output remain valid for the call.
    assert_eq!(
        unsafe {
            eqts_handle_invoke_v1(object, input.as_ptr(), input.len(), &raw mut invoke_output)
        },
        eqts::ABI_PANIC
    );
}

#[test]
fn poll_does_not_hold_registry_lock() {
    let handle = eqts::register_reactive(Reenter);
    let mut output = eqts::OwnedBuffer::empty();
    // SAFETY: output points to live writable storage.
    assert_eq!(
        unsafe { eqts_reactive_poll_v1(handle, &raw mut output) },
        eqts::ABI_REACTIVE_DONE
    );
}
