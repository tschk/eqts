eqts::setup!();

#[eqts::export]
pub fn add(a: u32, b: u32) -> u32 {
    a + b
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
pub fn echo_text(value: String) -> String {
    value
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
pub fn current_status() -> Status {
    Status::Ready
}

#[cfg(feature = "node-napi")]
pub mod node_napi {
    use napi::bindgen_prelude::{Uint8Array, Uint32Array};
    use napi_derive::napi;

    #[napi(object)]
    pub struct Person {
        pub name: String,
        pub age: u32,
    }

    #[napi]
    pub enum Status {
        Ready,
        Busy,
    }

    #[napi]
    pub fn add(a: u32, b: u32) -> u32 {
        super::add(a, b)
    }

    #[napi]
    pub fn echo_text(value: String) -> String {
        super::echo_text(value)
    }

    #[napi]
    pub fn reverse_bytes(value: Uint8Array) -> Vec<u8> {
        super::reverse_bytes(value.to_vec())
    }

    #[napi]
    pub fn sum_values(values: Uint32Array) -> u32 {
        super::sum_values(values.to_vec())
    }

    #[napi]
    pub fn maybe_name(present: bool) -> Option<String> {
        super::maybe_name(present)
    }

    #[napi]
    pub fn checked_divide(numerator: i32, denominator: i32) -> napi::Result<i32> {
        super::checked_divide(numerator, denominator).map_err(napi::Error::from_reason)
    }

    #[napi]
    pub fn make_person(name: String, age: u32) -> Person {
        let person = super::make_person(name, age);
        Person {
            name: person.name,
            age: person.age,
        }
    }

    #[napi]
    pub fn current_status() -> Status {
        match super::current_status() {
            super::Status::Ready => Status::Ready,
            super::Status::Busy => Status::Busy,
        }
    }
}

#[cfg(feature = "wasm")]
pub mod wasm {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(getter_with_clone)]
    pub struct Person {
        pub name: String,
        pub age: u32,
    }

    #[wasm_bindgen]
    pub enum Status {
        Ready,
        Busy,
    }

    #[wasm_bindgen]
    pub fn add(a: u32, b: u32) -> u32 {
        super::add(a, b)
    }

    #[wasm_bindgen(js_name = echoText)]
    pub fn echo_text(value: String) -> String {
        super::echo_text(value)
    }

    #[wasm_bindgen(js_name = reverseBytes)]
    pub fn reverse_bytes(value: Vec<u8>) -> Vec<u8> {
        super::reverse_bytes(value)
    }

    #[wasm_bindgen(js_name = sumValues)]
    pub fn sum_values(values: Vec<u32>) -> u32 {
        super::sum_values(values)
    }

    #[wasm_bindgen(js_name = maybeName)]
    pub fn maybe_name(present: bool) -> Option<String> {
        super::maybe_name(present)
    }

    #[wasm_bindgen(js_name = checkedDivide)]
    pub fn checked_divide(numerator: i32, denominator: i32) -> Result<i32, JsError> {
        super::checked_divide(numerator, denominator).map_err(|error| JsError::new(&error))
    }

    #[wasm_bindgen(js_name = makePerson)]
    pub fn make_person(name: String, age: u32) -> Person {
        let person = super::make_person(name, age);
        Person {
            name: person.name,
            age: person.age,
        }
    }

    #[wasm_bindgen(js_name = currentStatus)]
    pub fn current_status() -> Status {
        match super::current_status() {
            super::Status::Ready => Status::Ready,
            super::Status::Busy => Status::Busy,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn add_returns_sum() {
        assert_eq!(super::add(20, 22), 42);
    }
}
