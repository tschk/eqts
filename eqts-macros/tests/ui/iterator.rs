use eqts_macros::export;

#[export]
pub fn values() -> impl Iterator<Item = u32> {
    [1].into_iter()
}

fn main() {}
