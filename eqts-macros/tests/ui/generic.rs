use eqts_macros::export;

#[export]
pub fn identity<T>(value: T) -> T {
    value
}

fn main() {}
