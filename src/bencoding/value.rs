use std::collections::BTreeMap;

#[derive(Debug, PartialEq)]
pub enum Value {
    String(String),
    Integer(i32),
    List(Vec<Value>),
    Dictionary(BTreeMap<String, Value>),
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

    pub fn get_string2(&self) -> Option<&str> {
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

    pub fn get_dictionary(self) -> Option<BTreeMap<String, Self>> {
        match self {
            Self::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        }
    }
}
