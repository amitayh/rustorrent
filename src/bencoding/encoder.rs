use std::io::{Result, Write};

use crate::bencoding::value::Value;

impl Value {
    pub fn encode(&self, dest: &mut impl Write) -> Result<()> {
        match self {
            Self::String(string) => encode_string(string, dest),
            Self::Integer(integer) => write!(dest, "i{}e", integer),
            Self::List(values) => {
                write!(dest, "l")?;
                for value in values {
                    value.encode(dest)?;
                }
                write!(dest, "e")
            }
            Self::Dictionary(entries) => {
                write!(dest, "d")?;
                for (key, value) in entries {
                    encode_string(key.as_bytes(), dest)?;
                    value.encode(dest)?;
                }
                write!(dest, "e")
            }
        }
    }
}

fn encode_string(string: &[u8], dest: &mut impl Write) -> Result<()> {
    write!(dest, "{}:", string.len())?;
    dest.write_all(string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer() {
        let value = Value::Integer(42);
        let mut buffer = Vec::new();
        value.encode(&mut buffer).expect("unable to encode");

        assert_eq!(buffer, "i42e".as_bytes());
    }

    #[test]
    fn string() {
        let value = Value::string("foo");
        let mut buffer = Vec::new();
        value.encode(&mut buffer).expect("unable to encode");

        assert_eq!(buffer, "3:foo".as_bytes());
    }

    #[test]
    fn list() {
        let value = Value::list()
            .with_value(Value::string("foo"))
            .with_value(Value::string("bar"));
        let mut buffer = Vec::new();
        value.encode(&mut buffer).expect("unable to encode");

        assert_eq!(buffer, "l3:foo3:bare".as_bytes());
    }

    #[test]
    fn dictionary() {
        let value = Value::dictionary()
            .with_entry("foo", Value::Integer(1))
            .with_entry("bar", Value::Integer(2));
        let mut buffer = Vec::new();
        value.encode(&mut buffer).expect("unable to encode");

        // Dictionary keys are sorted
        assert_eq!(buffer, "d3:bari2e3:fooi1ee".as_bytes());
    }
}
