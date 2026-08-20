use eqts_macros::methods;

struct Counter {
    value: u32,
}

#[methods]
impl Counter {
    pub async fn bump(&mut self) {
        self.value += 1;
    }
}

fn main() {}
