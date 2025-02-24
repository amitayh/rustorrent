use crate::bencoding::value::Value;

use nom::{
    IResult, Parser,
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
    let (input, string) = take(length)(input)?;
    Ok((input, String::from(string)))
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

    use std::collections::BTreeMap;

    #[test]
    fn parse_error() {
        assert!(Value::try_from("foo").is_err());
    }

    #[test]
    fn string() {
        assert_eq!(
            Value::try_from("3:foo"),
            Ok(Value::String(String::from("foo")))
        );
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

    // TODO: fail parsing for "i-0e", "i03e".

    #[test]
    fn list() {
        assert_eq!(
            Value::try_from("l4:spam4:eggse"),
            Ok(Value::List(vec![
                Value::String(String::from("spam")),
                Value::String(String::from("eggs"))
            ]))
        );
    }

    #[test]
    fn dictionary() {
        assert_eq!(
            Value::try_from("d3:cow3:moo4:spam4:eggse"),
            Ok(Value::Dictionary(BTreeMap::from([
                (String::from("cow"), Value::String(String::from("moo"))),
                (String::from("spam"), Value::String(String::from("eggs")))
            ])))
        );
    }
}
