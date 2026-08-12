#[eqts::export]
#[must_use]
pub fn increment_u64(value: u64) -> u64 {
    value + 1
}

#[eqts::export]
#[must_use]
pub fn decrement_i64(value: i64) -> i64 {
    value - 1
}

#[test]
fn native_bigint_scalars_remain_exact() {
    let mut unsigned = 0;
    // SAFETY: output points to a live u64 for the duration of the call.
    assert_eq!(
        unsafe { eqts_increment_u64(u64::MAX - 1, &raw mut unsigned) },
        0
    );
    assert_eq!(unsigned, u64::MAX);

    let mut signed = 0;
    // SAFETY: output points to a live i64 for the duration of the call.
    assert_eq!(
        unsafe { eqts_decrement_i64(i64::MIN + 1, &raw mut signed) },
        0
    );
    assert_eq!(signed, i64::MIN);
}
