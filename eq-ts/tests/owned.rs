#[derive(eqts::Record)]
pub struct Person {
    pub name: String,
    pub scores: Vec<u32>,
}

#[derive(eqts::Enum)]
pub enum Status {
    Ready,
    Busy,
}

#[eqts::export]
#[expect(clippy::missing_errors_doc, clippy::needless_pass_by_value)]
pub fn greet(person: Person, status: Option<Status>) -> Result<String, String> {
    match status {
        Some(Status::Busy) => Err("busy".into()),
        _ => Ok(format!("hello {}", person.name)),
    }
}

#[test]
fn owned_values_cross_json_boundary() {
    let input = br#"[{"name":"Ada","scores":[1,2]},"Ready"]"#;
    let mut output = eqts::OwnedBuffer::empty();
    // SAFETY: input and output pointers remain valid for the duration of the call.
    let status = unsafe { eqts_greet(input.as_ptr(), input.len(), &raw mut output) };
    assert_eq!(status, eqts::ABI_OK);
    // SAFETY: wrapper returned a live allocation with these components.
    let bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) };
    assert_eq!(bytes, br#"{"ok":"hello Ada"}"#);
    // SAFETY: wrapper allocation is returned exactly once.
    unsafe { drop(Vec::from_raw_parts(output.ptr, output.len, output.capacity)) };
}

#[test]
fn invalid_input_returns_owned_error() {
    let input = b"no";
    let mut output = eqts::OwnedBuffer::empty();
    // SAFETY: input and output pointers remain valid for the duration of the call.
    let status = unsafe { eqts_greet(input.as_ptr(), input.len(), &raw mut output) };
    assert_eq!(status, eqts::ABI_INVALID_INPUT);
    assert!(!output.ptr.is_null());
    // SAFETY: wrapper allocation is returned exactly once.
    unsafe { drop(Vec::from_raw_parts(output.ptr, output.len, output.capacity)) };
}
