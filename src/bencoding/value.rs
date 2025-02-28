use std::{
    collections::BTreeMap,
    error::Error,
    fmt::{Debug, Display, Formatter, Result},
};

#[derive(PartialEq, Clone)]
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

impl Debug for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
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

#[derive(Debug)]
pub enum ValueType {
    String,
    Integer,
    List,
    Dictionary,
}

impl From<&Value> for ValueType {
    fn from(value: &Value) -> Self {
        match value {
            Value::String(_) => Self::String,
            Value::Integer(_) => Self::Integer,
            Value::List(_) => Self::List,
            Value::Dictionary(_) => Self::Dictionary,
        }
    }
}

#[derive(Debug)]
pub struct TypeMismatch {
    pub expected: ValueType,
    pub got: Value,
}

impl TypeMismatch {
    pub fn new(expected: ValueType, got: Value) -> Self {
        Self { expected, got }
    }
}

impl Display for TypeMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(
            f,
            "type mismatch. expected {:?}, got {:?}",
            self.expected, self.got
        )
    }
}

impl Error for TypeMismatch {}
