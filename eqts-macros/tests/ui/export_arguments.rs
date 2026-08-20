use eqts_macros::export;

#[export(name = "add")]
pub fn add(left: u32, right: u32) -> u32 {
    left + right
}

fn main() {}
