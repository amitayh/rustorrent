use std::collections::BTreeMap;

#[derive(PartialEq)]
pub enum Value {
    String(Vec<u8>),
    Integer(i64),
    List(Vec<Value>),
    Dictionary(BTreeMap<String, Value>),
}

impl Value {
    #[allow(dead_code)]
    pub fn string(str: &str) -> Self {
        Self::String(str.as_bytes().to_vec())
    }

    pub fn list() -> Self {
        Self::List(Vec::new())
    }

    pub fn with_value(mut self, value: Value) -> Self {
        if let Self::List(values) = &mut self {
            values.push(value);
        }
        self
    }

    pub fn dictionary() -> Self {
        Self::Dictionary(BTreeMap::new())
    }

    pub fn with_entry(mut self, key: &str, value: Value) -> Self {
        if let Self::Dictionary(entries) = &mut self {
            entries.insert(key.to_string(), value);
        }
        self
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(bytes) => match String::from_utf8(bytes.clone()) {
                Ok(string) => write!(f, "{:?}", string),
                Err(_) => write!(f, "<binary length={}>", bytes.len()),
            },
            Self::Integer(integer) => write!(f, "{}", integer),
            Self::List(list) => write!(f, "{:?}", list),
            Self::Dictionary(dictionary) => write!(f, "{:?}", dictionary),
        }
    }
}
