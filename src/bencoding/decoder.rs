use std::{
    collections::BTreeMap,
    io::{Error, ErrorKind, Result},
};

use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;

use crate::bencoding::value::Value;
use crate::codec::AsyncDecoder;

const BUFFER_SIZE: usize = 1024 * 8;

impl AsyncDecoder for Value {
    async fn decode<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self> {
        let mut parser = Parser::new();
        let mut buf = [0; BUFFER_SIZE];
        loop {
            let read = stream.read(&mut buf).await?;
            if read == 0 {
                break;
            }
            for byte in &buf[0..read] {
                parser.consume(*byte)?;
            }
        }
        parser.result()
    }
}

#[derive(Debug)]
struct Parser {
    state: State,
    stack: Vec<StackState>,
    position: usize,
}

#[derive(Debug)]
enum State {
    Ready,
    Integer(Option<i64>, i64),
    StringLength(usize),
    StringContents(Vec<u8>, usize),
    Done(Value),
}

#[derive(Debug)]
enum StackState {
    List(Vec<Value>),
    Dictionary(Option<String>, BTreeMap<String, Value>),
}

impl StackState {
    fn new_list() -> Self {
        Self::List(Vec::new())
    }

    fn new_dictionary() -> Self {
        Self::Dictionary(None, BTreeMap::new())
    }
}

impl Parser {
    fn new() -> Self {
        Self {
            state: State::Ready,
            stack: Vec::new(),
            position: 0,
        }
    }

    fn consume(&mut self, byte: u8) -> Result<()> {
        match (&mut self.state, byte) {
            // Integer
            (State::Ready, b'i') => {
                self.state = State::Integer(None, 1);
            }
            (State::Integer(None, sign), b'-') => {
                *sign = -1;
            }
            (State::Integer(None, _), b'0') => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "leading zeros not allowed",
                ));
            }
            (State::Integer(integer, _), b'0'..=b'9') => {
                let digit = (byte - b'0') as i64;
                *integer = Some(integer.unwrap_or(0) * 10 + digit);
            }
            (&mut State::Integer(Some(integer), sign), b'e') => {
                self.emit(Value::Integer(integer * sign))?;
            }

            // List
            (State::Ready, b'l') => {
                self.stack.push(StackState::new_list());
            }

            // Dictionary
            (State::Ready, b'd') => {
                self.stack.push(StackState::new_dictionary());
            }

            // String
            (State::Ready, b'1'..=b'9') => {
                let digit = (byte - b'0') as usize;
                self.state = State::StringLength(digit);
            }
            (State::StringLength(length), b'0'..=b'9') => {
                let digit = (byte - b'0') as usize;
                *length = *length * 10 + digit;
            }
            (&mut State::StringLength(length), b':') => {
                let string = Vec::with_capacity(length);
                self.state = State::StringContents(string, length);
            }
            (State::StringContents(bytes, length), _) => {
                bytes.push(byte);
                if bytes.len() == *length {
                    let string = std::mem::take(bytes);
                    self.emit(Value::String(string))?;
                }
            }

            // End collection
            (_, b'e') => match self.stack.pop() {
                Some(StackState::List(list)) => {
                    self.emit(Value::List(list))?;
                }
                Some(StackState::Dictionary(_, entries)) => {
                    self.emit(Value::Dictionary(entries))?;
                }
                None => {
                    return Err(Error::new(ErrorKind::InvalidInput, "nothing to close"));
                }
            },

            // Ignore trailing whitespace
            (State::Done(_), b'\n' | b'\r' | b' ') => return Ok(()),

            // Unexpected input
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("unexpected byte: 0x{byte:02x}, parser state: {self:?}"),
                ));
            }
        }
        self.position += 1;
        Ok(())
    }

    fn emit(&mut self, value: Value) -> Result<()> {
        match (self.stack.last_mut(), value) {
            (Some(StackState::List(list)), value) => {
                list.push(value);
                self.state = State::Ready;
            }
            (Some(StackState::Dictionary(key @ None, _)), Value::String(string)) => {
                let string = String::from_utf8(string).map_err(|_| {
                    Error::new(
                        ErrorKind::InvalidInput,
                        "dictionary key should be valid utf8",
                    )
                })?;
                *key = Some(string);
                self.state = State::Ready;
            }
            (Some(StackState::Dictionary(None, _)), _) => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "only string keys are allowed in dictionaries",
                ));
            }
            (Some(StackState::Dictionary(key, entries)), value) => {
                let key = key.take().expect("key must be present");
                entries.insert(key, value);
                self.state = State::Ready;
            }
            (None, value) => {
                self.state = State::Done(value);
            }
        }
        Ok(())
    }

    fn result(self) -> Result<Value> {
        match self.state {
            State::Done(value) => Ok(value),
            _ => Err(Error::new(ErrorKind::UnexpectedEof, "incomplete")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tokio::io::BufReader;

    use super::*;

    async fn decode<const N: usize>(input: &[u8; N]) -> Result<Value> {
        let cursor = Cursor::new(input);
        let mut buf = BufReader::new(cursor);
        Value::decode(&mut buf).await
    }

    #[tokio::test]
    async fn parse_error() {
        let result = decode(b"foo").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn string() {
        let result = decode(b"3:foo").await;

        assert_eq!(result.unwrap(), Value::string("foo"));
    }

    #[tokio::test]
    async fn positive_integer() {
        let result = decode(b"i3e").await;

        assert_eq!(result.unwrap(), Value::Integer(3));
    }

    #[tokio::test]
    async fn multi_digit_integer() {
        let result = decode(b"i42e").await;

        assert_eq!(result.unwrap(), Value::Integer(42));
    }

    #[tokio::test]
    async fn negative_integer() {
        let result = decode(b"i-1e").await;

        assert_eq!(result.unwrap(), Value::Integer(-1));
    }

    #[tokio::test]
    async fn fail_for_minus_zero() {
        let result = decode(b"i-0e").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fail_for_leading_zero() {
        let result = decode(b"i03e").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn empty_list() {
        let result = decode(b"le").await;

        assert_eq!(result.unwrap(), Value::list());
    }

    #[tokio::test]
    async fn non_empty_list() {
        let result = decode(b"li1ei2ei3ee").await;

        assert_eq!(
            result.unwrap(),
            Value::list()
                .with_value(Value::Integer(1))
                .with_value(Value::Integer(2))
                .with_value(Value::Integer(3))
        );
    }

    #[tokio::test]
    async fn nested_list() {
        let result = decode(b"li1eli2ei3eee").await;

        assert_eq!(
            result.unwrap(),
            Value::list().with_value(Value::Integer(1)).with_value(
                Value::list()
                    .with_value(Value::Integer(2))
                    .with_value(Value::Integer(3))
            )
        );
    }

    #[tokio::test]
    async fn heterogeneous_list() {
        let result = decode(b"l3:fooi42ee").await;

        assert_eq!(
            result.unwrap(),
            Value::list()
                .with_value(Value::string("foo"))
                .with_value(Value::Integer(42))
        );
    }

    #[tokio::test]
    async fn empty_dictionary() {
        let result = decode(b"de").await;

        assert_eq!(result.unwrap(), Value::dictionary());
    }

    #[tokio::test]
    async fn non_empty_dictionary() {
        let result = decode(b"d3:cow3:moo4:spam4:eggse").await;

        assert_eq!(
            result.unwrap(),
            Value::dictionary()
                .with_entry("cow", Value::string("moo"))
                .with_entry("spam", Value::string("eggs"))
        );
    }

    #[tokio::test]
    async fn fail_for_non_string_keys() {
        let result = decode(b"di1ei2ee").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn deeply_nested_structure() {
        let result = decode(b"d3:food3:barl3:bazee3:quxi42ee").await;

        assert_eq!(
            result.unwrap(),
            Value::dictionary()
                .with_entry(
                    "foo",
                    Value::dictionary()
                        .with_entry("bar", Value::list().with_value(Value::string("baz")))
                )
                .with_entry("qux", Value::Integer(42))
        );
    }

    #[tokio::test]
    async fn ignore_trailing_whitespace() {
        let result = decode(b"i42e ").await;

        assert_eq!(result.unwrap(), Value::Integer(42));
    }
}
