use std::collections::BTreeMap;

#[derive(Debug, PartialEq)]
pub enum Value {
    String(String),
    Integer(i32),
    List(Vec<Value>),
    Dictionary(BTreeMap<String, Value>),
}
