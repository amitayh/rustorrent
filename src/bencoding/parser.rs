use std::{collections::HashMap, io::ErrorKind};

use crate::bencoding::value::Value;

use nom::{
    AsChar, IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take},
    character::{
        char,
        complete::{i32, usize},
    },
    combinator::map,
    multi::many0,
    sequence::delimited,
};

fn parse_string(input: &str) -> IResult<&str, String> {
    let (input, length) = usize(input)?;
    let (input, _) = tag(":")(input)?;
    let (input, str) = take(length)(input)?;
    Ok((input, String::from(str)))
}

fn parse(input: &str) -> IResult<&str, Value> {
    alt((
        map(parse_string, Value::String),
        map(delimited(char('i'), i32, char('e')), Value::Integer),
        map(delimited(char('l'), many0(parse), char('e')), Value::List),
        map(
            delimited(char('d'), many0(parse_string.and(parse)), char('e')),
            |entries| Value::Dictionary(entries.into_iter().collect()),
        ),
    ))
    .parse(input)
}

impl TryFrom<&str> for Value {
    type Error = String;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        match parse(input) {
            Ok(("", value)) => Ok(value),
            Ok(_) => Err("Remainder".to_string()),
            Err(err) => Err(err.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    #[test]
    fn parse_error() {
        assert!(Value::try_from("foo").is_err());
    }

    #[test]
    fn string() {
        assert_eq!(Value::try_from("3:foo"), Ok(Value::string("foo")));
    }

    #[test]
    fn positive_integer() {
        assert_eq!(Value::try_from("i3e"), Ok(Value::Integer(3)));
    }

    #[test]
    fn multi_digit_integer() {
        assert_eq!(Value::try_from("i42e"), Ok(Value::Integer(42)));
    }

    #[test]
    fn negative_integer() {
        assert_eq!(Value::try_from("i-1e"), Ok(Value::Integer(-1)));
    }

    #[test]
    #[ignore = "not implemented"]
    fn fail_parsing_for_minus_zero() {
        assert!(Value::try_from("i-0e").is_err());
    }

    #[test]
    #[ignore = "not implemented"]
    fn fail_parsing_for_leading_zero() {
        assert!(Value::try_from("i03e").is_err());
    }

    #[test]
    fn list() {
        assert_eq!(
            Value::try_from("l4:spam4:eggse"),
            Ok(Value::List(vec![
                Value::string("spam"),
                Value::string("eggs")
            ]))
        );
    }

    #[test]
    fn dictionary() {
        assert_eq!(
            Value::try_from("d3:cow3:moo4:spam4:eggse"),
            Ok(Value::Dictionary(HashMap::from([
                ("cow".to_string(), Value::string("moo")),
                ("spam".to_string(), Value::string("eggs"))
            ])))
        );
    }
}

struct Parser2 {
    state: ParseState,
    stack: Vec<StackValue>,
}

enum StackValue {
    List(Vec<Value>),
    Dictionary(HashMap<String, Value>),
}

enum ParseState {
    Ready,
    Integer(Option<i32>, i32),
    List(Vec<Value>),
    Done(Value),
}

impl Parser2 {
    fn new() -> Self {
        Self {
            state: ParseState::Ready,
            stack: Vec::new(),
        }
    }

    fn consume(&mut self, byte: u8) -> std::io::Result<()> {
        match (&self.state, byte) {
            (ParseState::Ready, b'i') => {
                self.state = ParseState::Integer(None, 1);
                Ok(())
            }
            (ParseState::Integer(None, _), b'-') => {
                self.state = ParseState::Integer(None, -1);
                Ok(())
            }
            (ParseState::Integer(None, _), b'0') => Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "leading zeros not allowed",
            )),
            (ParseState::Integer(integer, sign), digit) if digit.is_dec_digit() => {
                let digit = (digit - b'0') as i32;
                let integer = integer.unwrap_or(0) * 10 + digit;
                self.state = ParseState::Integer(Some(integer), *sign);
                Ok(())
            }
            (ParseState::Integer(Some(integer), sign), b'e') => {
                self.done(Value::Integer(integer * sign));
                Ok(())
            }
            (ParseState::Ready, b'l') => {
                self.stack.push(StackValue::List(Vec::new()));
                self.state = ParseState::Ready;
                Ok(())
            }
            (_, b'e') => match self.stack.pop() {
                Some(StackValue::List(list)) => {
                    self.done(Value::List(list));
                    Ok(())
                }
                _ => Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("unexpected byte: 0x{byte:x}"),
                )),
            },
            _ => Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("unexpected byte: 0x{byte:x}"),
            )),
        }
    }

    fn done(&mut self, value: Value) {
        match self.stack.last_mut() {
            Some(StackValue::List(list)) => {
                list.push(value);
                self.state = ParseState::Ready;
            }
            _ => {
                self.state = ParseState::Done(value);
            }
        }
    }

    fn result(self) -> Value {
        if let ParseState::Done(value) = self.state {
            return value;
        }
        panic!("ahhh")
    }
}

impl std::io::Write for Parser2 {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for byte in buf {
            self.consume(*byte)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests2 {
    use std::io::Write;

    use super::*;

    //#[test]
    //fn parse_error() {
    //    assert!(Value::try_from("foo").is_err());
    //}

    #[test]
    fn string() {
        assert_eq!(Value::try_from("3:foo"), Ok(Value::string("foo")));
    }

    #[test]
    fn positive_integer() {
        let mut parser = Parser2::new();
        let bytes_written = parser.write(b"i3e").expect("unable to write");

        assert_eq!(bytes_written, 3);
        assert_eq!(parser.result(), Value::Integer(3));
    }

    #[test]
    fn multi_digit_integer() {
        let mut parser = Parser2::new();
        let bytes_written = parser.write(b"i42e").expect("unable to write");

        assert_eq!(bytes_written, 4);
        assert_eq!(parser.result(), Value::Integer(42));
    }

    #[test]
    fn negative_integer() {
        let mut parser = Parser2::new();
        let bytes_written = parser.write(b"i-1e").expect("unable to write");

        assert_eq!(bytes_written, 4);
        assert_eq!(parser.result(), Value::Integer(-1));
    }

    #[test]
    fn fail_parsing_for_minus_zero() {
        let mut parser = Parser2::new();

        assert!(parser.write(b"i-0e").is_err());
    }

    #[test]
    fn fail_parsing_for_leading_zero() {
        let mut parser = Parser2::new();

        assert!(parser.write(b"i03e").is_err());
    }

    #[test]
    fn empty_list() {
        let mut parser = Parser2::new();
        let bytes_written = parser.write(b"le").expect("unable to write");

        assert_eq!(bytes_written, 2);
        assert_eq!(parser.result(), Value::List(vec![]));
    }

    #[test]
    fn non_empty_list() {
        let mut parser = Parser2::new();
        let bytes_written = parser.write(b"li1ei2ei3ee").expect("unable to write");

        assert_eq!(bytes_written, 11);
        assert_eq!(
            parser.result(),
            Value::List(vec![
                Value::Integer(1),
                Value::Integer(2),
                Value::Integer(3)
            ])
        );
    }

    #[test]
    fn nested_list() {
        let mut parser = Parser2::new();
        let bytes_written = parser.write(b"li1eli2ei3eee").expect("unable to write");

        assert_eq!(bytes_written, 13);
        assert_eq!(
            parser.result(),
            Value::List(vec![
                Value::Integer(1),
                Value::List(vec![Value::Integer(2), Value::Integer(3)])
            ])
        );
    }

    //#[test]
    //fn list() {
    //    assert_eq!(
    //        Value::try_from("l4:spam4:eggse"),
    //        Ok(Value::List(vec![
    //            Value::string("spam"),
    //            Value::string("eggs")
    //        ]))
    //    );
    //}

    //#[test]
    //fn dictionary() {
    //    assert_eq!(
    //        Value::try_from("d3:cow3:moo4:spam4:eggse"),
    //        Ok(Value::Dictionary(HashMap::from([
    //            ("cow".to_string(), Value::string("moo")),
    //            ("spam".to_string(), Value::string("eggs"))
    //        ])))
    //    );
    //}
}
