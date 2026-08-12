use eqts::EqtsValue as _;

eqts::setup!();

#[eqts::export]
pub fn add(a: u32, b: u32) -> u32 {
    a + b
}

#[eqts::export]
pub fn negate(value: bool) -> bool {
    !value
}

#[eqts::export]
pub fn add_i64(value: i64, delta: i64) -> i64 {
    value + delta
}

#[eqts::export]
pub fn add_u64(value: u64, delta: u64) -> u64 {
    value + delta
}

#[derive(eqts::Record)]
pub struct Person {
    pub name: String,
    pub age: u32,
}

#[derive(eqts::Enum)]
pub enum Status {
    Ready,
    Busy,
}

#[derive(Clone, eqts::Record)]
pub struct Counter {
    pub value: u32,
}

#[eqts::methods]
impl Counter {
    #[eqts::method]
    pub fn increment(&mut self, by: u32) -> u32 {
        self.value += by;
        self.value
    }

    #[eqts::method]
    pub fn current(&self) -> u32 {
        self.value
    }

    #[eqts::method]
    pub async fn pending(&self) -> String {
        std::future::pending().await
    }
}

#[derive(Clone, eqts::Record)]
pub struct Greeter {
    pub prefix: String,
}

#[eqts::methods]
impl Greeter {
    #[eqts::method]
    pub fn greet(&self, name: String) -> String {
        format!("{} {name}", self.prefix)
    }
}

#[eqts::object(Counter)]
pub fn counter(value: u32) -> Counter {
    Counter { value }
}

#[eqts::trait_export(Greeter)]
pub fn greeter(prefix: String) -> Greeter {
    Greeter { prefix }
}

#[eqts::export]
pub fn echo_text(input: String) -> String {
    input
}

#[eqts::export]
pub fn reverse_bytes(mut value: Vec<u8>) -> Vec<u8> {
    value.reverse();
    value
}

#[eqts::export]
pub fn sum_values(values: Vec<u32>) -> u32 {
    values.into_iter().sum()
}

#[eqts::export]
pub fn maybe_name(present: bool) -> Option<String> {
    present.then(|| "eqts".to_owned())
}

#[eqts::export]
pub fn maybe_bytes(present: bool) -> Option<Vec<u8>> {
    present.then(|| vec![1, 2, 3])
}

#[eqts::export]
pub fn flatten_values(values: Vec<Vec<u32>>) -> Vec<u32> {
    values.into_iter().flatten().collect()
}

#[eqts::export]
pub fn checked_divide(numerator: i32, denominator: i32) -> Result<i32, String> {
    if denominator == 0 {
        return Err("division by zero".to_owned());
    }
    Ok(numerator / denominator)
}

#[eqts::export]
pub fn make_person(name: String, age: u32) -> Person {
    Person { name, age }
}

#[eqts::export]
pub fn person_or_error(succeed: bool) -> Result<Person, Person> {
    let person = Person {
        name: "Ada".to_owned(),
        age: 36,
    };
    if succeed { Ok(person) } else { Err(person) }
}

#[eqts::export]
pub fn current_status() -> Status {
    Status::Ready
}

struct Sequence {
    values: std::collections::VecDeque<eqts::ReactivePoll>,
}

impl Sequence {
    fn new(values: impl IntoIterator<Item = eqts::ReactivePoll>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }
}

impl eqts::ReactiveResource for Sequence {
    fn poll(&mut self) -> Result<eqts::ReactivePoll, String> {
        Ok(self.values.pop_front().unwrap_or(eqts::ReactivePoll::Done))
    }
}

struct CallbackSequence {
    callback: eqts::Callback<String>,
    emitted_second: bool,
}

impl eqts::ReactiveResource for CallbackSequence {
    fn poll(&mut self) -> Result<eqts::ReactivePoll, String> {
        if !self.emitted_second {
            self.callback.emit("second".to_owned())?;
            self.emitted_second = true;
        }
        Ok(eqts::ReactivePoll::Pending)
    }
}

#[eqts::async_export(String)]
pub async fn pending_task() -> String {
    std::future::pending().await
}

#[eqts::callback(String)]
pub fn callback_events(callback: eqts::Callback<String>) -> impl eqts::ReactiveResource {
    callback
        .emit("first".to_owned())
        .expect("empty callback queue should accept first value");
    CallbackSequence {
        callback,
        emitted_second: false,
    }
}

#[eqts::stream(u32)]
pub fn numbers() -> impl eqts::ReactiveResource {
    Sequence::new([
        eqts::ReactivePoll::Pending,
        eqts::ReactivePoll::Ready(1_u32.into_json().expect("u32 should serialize")),
        eqts::ReactivePoll::Ready(2_u32.into_json().expect("u32 should serialize")),
    ])
}

#[eqts::iterator(String)]
pub fn words() -> impl eqts::ReactiveResource {
    Sequence::new([
        eqts::ReactivePoll::Ready(
            "one"
                .to_owned()
                .into_json()
                .expect("string should serialize"),
        ),
        eqts::ReactivePoll::Ready(
            "two"
                .to_owned()
                .into_json()
                .expect("string should serialize"),
        ),
    ])
}

#[cfg(test)]
mod tests {
    #[test]
    fn add_returns_sum() {
        assert_eq!(super::add(20, 22), 42);
    }
}
