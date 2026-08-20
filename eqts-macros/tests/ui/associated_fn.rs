use eqts_macros::methods;

struct Counter;

#[methods]
impl Counter {
    pub fn create() -> Self {
        Self
    }
}

fn main() {}
