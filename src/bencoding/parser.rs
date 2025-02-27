use std::{
    collections::HashMap,
    io::{Error, ErrorKind, Result, Write},
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::AsyncWrite;

use crate::bencoding::value::Value;

pub struct Parser {
    state: State,
    stack: Vec<StackState>,
}

enum State {
    Ready,
    Integer(Option<i64>, i64),
    StringLength(usize),
    StringContents(Vec<u8>, usize),
    Done(Value),
}

enum StackState {
    List(Vec<Value>),
    Dictionary(Option<String>, HashMap<String, Value>),
}

impl StackState {
    fn new_list() -> Self {
        Self::List(Vec::new())
    }

    fn new_dictionary() -> Self {
        Self::Dictionary(None, HashMap::new())
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ready,
            stack: Vec::new(),
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
                self.state = State::Ready;
            }

            // Dictionary
            (State::Ready, b'd') => {
                self.stack.push(StackState::new_dictionary());
                self.state = State::Ready;
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
                    format!("unexpected byte: 0x{byte:x}"),
                ));
            }
        }
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

    pub fn result(self) -> Result<Value> {
        match self.state {
            State::Done(value) => Ok(value),
            _ => Err(Error::new(ErrorKind::UnexpectedEof, "incomplete")),
        }
    }
}

impl Write for Parser {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        for byte in buf {
            self.consume(*byte)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl AsyncWrite for Parser {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Poll::Ready(self.write(buf))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(self.flush())
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn parse_error() {
        let mut parser = Parser::new();

        assert!(parser.write(b"foo").is_err());
    }

    #[test]
    fn string() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"3:foo").expect("unable to write");

        assert_eq!(bytes_written, 5);
        assert_eq!(parser.result().unwrap(), Value::string("foo"));
    }

    #[test]
    fn positive_integer() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"i3e").expect("unable to write");

        assert_eq!(bytes_written, 3);
        assert_eq!(parser.result().unwrap(), Value::Integer(3));
    }

    #[test]
    fn multi_digit_integer() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"i42e").expect("unable to write");

        assert_eq!(bytes_written, 4);
        assert_eq!(parser.result().unwrap(), Value::Integer(42));
    }

    #[test]
    fn negative_integer() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"i-1e").expect("unable to write");

        assert_eq!(bytes_written, 4);
        assert_eq!(parser.result().unwrap(), Value::Integer(-1));
    }

    #[test]
    fn fail_for_minus_zero() {
        let mut parser = Parser::new();

        assert!(parser.write(b"i-0e").is_err());
    }

    #[test]
    fn fail_for_leading_zero() {
        let mut parser = Parser::new();

        assert!(parser.write(b"i03e").is_err());
    }

    #[test]
    fn empty_list() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"le").expect("unable to write");

        assert_eq!(bytes_written, 2);
        assert_eq!(parser.result().unwrap(), Value::List(vec![]));
    }

    #[test]
    fn non_empty_list() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"li1ei2ei3ee").expect("unable to write");

        assert_eq!(bytes_written, 11);
        assert_eq!(
            parser.result().unwrap(),
            Value::List(vec![
                Value::Integer(1),
                Value::Integer(2),
                Value::Integer(3)
            ])
        );
    }

    #[test]
    fn nested_list() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"li1eli2ei3eee").expect("unable to write");

        assert_eq!(bytes_written, 13);
        assert_eq!(
            parser.result().unwrap(),
            Value::List(vec![
                Value::Integer(1),
                Value::List(vec![Value::Integer(2), Value::Integer(3)])
            ])
        );
    }

    #[test]
    fn heterogeneous_list() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"l3:fooi42ee").expect("unable to write");

        assert_eq!(bytes_written, 11);
        assert_eq!(
            parser.result().unwrap(),
            Value::List(vec![Value::string("foo"), Value::Integer(42),])
        );
    }

    #[test]
    fn empty_dictionary() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"de").expect("unable to write");

        assert_eq!(bytes_written, 2);
        assert_eq!(parser.result().unwrap(), Value::Dictionary(HashMap::new()));
    }

    #[test]
    fn non_empty_dictionary() {
        let mut parser = Parser::new();
        let bytes_written = parser
            .write(b"d3:cow3:moo4:spam4:eggse")
            .expect("unable to write");

        assert_eq!(bytes_written, 24);
        assert_eq!(
            parser.result().unwrap(),
            Value::Dictionary(HashMap::from([
                ("cow".to_string(), Value::string("moo")),
                ("spam".to_string(), Value::string("eggs"))
            ]))
        );
    }

    #[test]
    fn fail_for_non_string_keys() {
        let mut parser = Parser::new();

        assert!(parser.write(b"di1ei2ee").is_err());
    }

    #[test]
    fn deeply_nested_structure() {
        let mut parser = Parser::new();
        let bytes_written = parser
            .write(b"d3:food3:barl3:bazee3:quxi42ee")
            .expect("unable to write");

        assert_eq!(bytes_written, 30);
        assert_eq!(
            parser.result().unwrap(),
            Value::Dictionary(HashMap::from([
                (
                    "foo".to_string(),
                    Value::Dictionary(HashMap::from([(
                        "bar".to_string(),
                        Value::List(vec![Value::string("baz")])
                    )]))
                ),
                ("qux".to_string(), Value::Integer(42))
            ]))
        );
    }

    #[test]
    fn incremental_parsing() {
        let mut parser = Parser::new();
        parser.write(b"l3:fo").expect("unable to write");
        parser.write(b"o3:ba").expect("unable to write");
        parser.write(b"re").expect("unable to write");

        assert_eq!(
            parser.result().unwrap(),
            Value::List(vec![Value::string("foo"), Value::string("bar")])
        );
    }

    #[test]
    fn ignore_trailing_whitespace() {
        let mut parser = Parser::new();
        let bytes_written = parser.write(b"i42e ").expect("unable to write");

        assert_eq!(bytes_written, 5);
        assert_eq!(parser.result().unwrap(), Value::Integer(42));
    }
}
