#[eqts::export]
#[must_use]
pub fn invert(value: bool) -> bool {
    !value
}

#[eqts::export]
#[must_use]
pub fn add(left: i64, right: i64) -> i64 {
    left + right
}

eqts::setup!();

#[test]
fn bool_is_lowered_to_u8() {
    let mut output = 0_u8;
    // SAFETY: output points to a live u8 for the duration of the call.
    let status = unsafe { eqts_invert(0, &raw mut output) };
    assert_eq!((status, output), (eqts::ABI_OK, 1));
}

#[test]
fn numeric_output_uses_status_and_out_pointer() {
    let mut output = 0_i64;
    // SAFETY: output points to a live i64 for the duration of the call.
    let status = unsafe { eqts_add(20, 22, &raw mut output) };
    assert_eq!((status, output), (eqts::ABI_OK, 42));
}

#[test]
fn metadata_is_versioned_and_sorted() {
    let metadata: serde_json::Value =
        serde_json::from_slice(eqts::metadata_json()).expect("metadata must be valid JSON");
    assert_eq!(metadata["schema_version"], 3);
    assert_eq!(metadata["capabilities"]["owned_values"], true);
    assert_eq!(metadata["capabilities"]["objects"], false);
    assert_eq!(metadata["capabilities"]["async_functions"], false);
    assert_eq!(metadata["capabilities"]["callbacks"], false);
    assert_eq!(metadata["capabilities"]["traits"], false);
    assert_eq!(metadata["capabilities"]["streams"], false);
    assert_eq!(metadata["capabilities"]["iterators"], false);
    assert_eq!(metadata["functions"][0]["name"], "add");
    assert_eq!(metadata["functions"][1]["name"], "invert");
}
