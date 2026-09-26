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
pub fn scale(value: f64, factor: f64) -> f64 {
    value * factor
}

#[eqts::export]
pub fn next_byte(value: u8) -> u8 {
    value.wrapping_add(1)
}

#[eqts::export]
pub fn log_only(value: u32) {
    let _ = value;
}

#[eqts::export]
pub fn echo_text(input: String) -> String {
    input
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
pub fn reverse_bytes(mut value: Vec<u8>) -> Vec<u8> {
    value.reverse();
    value
}

#[eqts::export]
pub fn checked_divide(numerator: i32, denominator: i32) -> Result<i32, String> {
    if denominator == 0 {
        return Err("division by zero".to_owned());
    }
    Ok(numerator / denominator)
}

#[derive(eqts::Record)]
pub struct Person {
    pub name: String,
    pub age: u32,
}

#[eqts::export]
pub fn make_person(name: String, age: u32) -> Person {
    Person { name, age }
}

#[derive(eqts::Enum)]
pub enum Status {
    Ready,
    Busy,
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
        eqts::ReactivePoll::Ready("one".to_owned().into_json().expect("string should serialize")),
        eqts::ReactivePoll::Ready("two".to_owned().into_json().expect("string should serialize")),
    ])
}

#[eqts::stream(Person)]
pub fn people() -> impl eqts::ReactiveResource {
    Sequence::new([eqts::ReactivePoll::Ready(
        Person {
            name: "Ada".to_owned(),
            age: 36,
        }
        .into_json()
        .expect("person should serialize"),
    )])
}

#[cfg(test)]
mod tests {
    #[test]
    fn add_returns_sum() {
        assert_eq!(super::add(20, 22), 42);
    }
}
