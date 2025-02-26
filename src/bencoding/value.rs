use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub enum Value {
    String(String),
    Integer(i32),
    List(Vec<Value>),
    Dictionary(HashMap<String, Value>),
}

impl Value {
    pub fn string(str: &str) -> Self {
        Self::String(str.to_string())
    }

    pub fn get_string(self) -> Option<String> {
        match self {
            Self::String(string) => Some(string),
            _ => None,
        }
    }

    pub fn get_str(&self) -> Option<&str> {
        match self {
            Self::String(string) => Some(string),
            _ => None,
        }
    }

    pub fn get_integer(self) -> Option<i32> {
        match self {
            Self::Integer(integer) => Some(integer),
            _ => None,
        }
    }

    pub fn get_dictionary(self) -> Option<HashMap<String, Self>> {
        match self {
            Self::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        }
    }

    pub fn get_entry(mut self, key: &str) -> Option<Value> {
        match &mut self {
            Self::Dictionary(dictionary) => dictionary.remove(key),
            _ => None,
        }
    }
}

impl<'a> From<&'a Value> for Option<&'a str> {
    fn from(value: &'a Value) -> Self {
        match value {
            Value::String(str) => Some(str),
            _ => None,
        }
    }
}
