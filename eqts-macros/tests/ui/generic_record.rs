use eqts_macros::Record;

#[derive(Record)]
pub struct Value<T> {
    pub value: T,
}

fn main() {}
