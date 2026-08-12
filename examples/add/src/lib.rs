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

#[cfg(test)]
mod tests {
    #[test]
    fn add_returns_sum() {
        assert_eq!(super::add(20, 22), 42);
    }
}
