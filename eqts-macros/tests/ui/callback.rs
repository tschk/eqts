use eqts_macros::export;

#[export]
pub fn invoke(callback: fn(u32) -> u32) -> u32 {
    callback(1)
}

fn main() {}
