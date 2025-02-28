use std::{
    collections::BTreeMap,
    fmt::{Debug, Display, Formatter},
    time::Duration,
};

use anyhow::{Error, Result, anyhow};
use sha1::Digest;
use size::Size;

use crate::crypto::{Md5, Sha1};

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
        } else {
            panic!("unable to add values to {:?}", ValueType::from(&self));
        }
        self
    }

    pub fn dictionary() -> Self {
        Self::Dictionary(BTreeMap::new())
    }

    pub fn with_entry(mut self, key: &str, value: Value) -> Self {
        if let Self::Dictionary(entries) = &mut self {
            entries.insert(key.to_string(), value);
        } else {
            panic!("unable to add entries to {:?}", ValueType::from(&self));
        }
        self
    }

    pub fn try_remove_entry(&mut self, key: &str) -> Result<Option<Value>> {
        match self {
            Value::Dictionary(entries) => Ok(entries.remove(key)),
            _ => Err(Error::new(TypeMismatch::new(
                ValueType::Dictionary,
                self.clone(),
            ))),
        }
    }

    pub fn remove_entry(&mut self, key: &str) -> Result<Value> {
        match self {
            Value::Dictionary(entries) => {
                entries.remove(key).ok_or(anyhow!("key missing: {:?}", key))
            }
            _ => Err(Error::new(TypeMismatch::new(
                ValueType::Dictionary,
                self.clone(),
            ))),
        }
    }
}

impl Debug for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
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

impl TryFrom<Value> for Vec<u8> {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::String(bytes) => Ok(bytes),
            _ => Err(Error::new(TypeMismatch::new(ValueType::String, value))),
        }
    }
}

impl TryFrom<Value> for String {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::String(bytes) => String::from_utf8(bytes).map_err(Error::new),
            _ => Err(Error::new(TypeMismatch::new(ValueType::String, value))),
        }
    }
}

impl TryFrom<Value> for u16 {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::Integer(integer) => Self::try_from(integer).map_err(Error::new),
            _ => Err(Error::new(TypeMismatch::new(ValueType::Integer, value))),
        }
    }
}

impl TryFrom<Value> for usize {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::Integer(integer) => Self::try_from(integer).map_err(Error::new),
            _ => Err(Error::new(TypeMismatch::new(ValueType::Integer, value))),
        }
    }
}

impl TryFrom<Value> for Size {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::Integer(bytes) => Ok(Size::from_bytes(bytes)),
            _ => Err(Error::new(TypeMismatch::new(ValueType::Integer, value))),
        }
    }
}

impl TryFrom<Value> for Duration {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::Integer(seconds) => u64::try_from(seconds)
                .map(Duration::from_secs)
                .map_err(Error::new),
            _ => Err(Error::new(TypeMismatch::new(ValueType::Integer, value))),
        }
    }
}

impl TryFrom<Value> for Vec<Value> {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::List(list) => Ok(list),
            _ => Err(Error::new(TypeMismatch::new(ValueType::List, value))),
        }
    }
}

impl TryFrom<Value> for Md5 {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::String(string) => {
                let bytes = hex::decode(&string)?;
                match bytes.try_into() {
                    Ok(md5) => Ok(Md5(md5)),
                    Err(_) => Err(anyhow!("invalid md5")),
                }
            }
            _ => Err(Error::new(TypeMismatch::new(ValueType::String, value))),
        }
    }
}

impl TryFrom<&Value> for Sha1 {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self> {
        let mut hasher = sha1::Sha1::new();
        value.encode(&mut hasher)?;
        let sha1 = hasher.finalize();
        Ok(Sha1(sha1.into()))
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
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "type mismatch. expected {:?}, got {:?}",
            self.expected, self.got
        )
    }
}

impl std::error::Error for TypeMismatch {}
