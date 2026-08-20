use eqts_macros::methods;

trait Greeter {
    fn greet(&self);
}

struct Person;

#[methods]
impl Greeter for Person {
    fn greet(&self) {}
}

fn main() {}
