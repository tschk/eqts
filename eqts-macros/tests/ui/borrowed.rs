use eqts_macros::export;

#[export]
pub fn length(value: &str) -> u32 {
    value.len() as u32
}

fn main() {}
