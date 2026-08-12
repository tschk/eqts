use eqts_macros::export;

#[export]
pub fn consume(value: Box<dyn Send>) -> u32 {
    drop(value);
    1
}

fn main() {}
