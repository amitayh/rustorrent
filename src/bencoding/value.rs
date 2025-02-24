use std::collections::BTreeMap;

#[derive(Debug, PartialEq)]
pub enum Value {
    String(String),
    Integer(i32),
    List(Vec<Value>),
    Dictionary(BTreeMap<String, Value>),
}

mod parsers {
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

    use super::Value;

    fn parse_string(input: &str) -> IResult<&str, String> {
        let (input, length) = usize(input)?;
        let (input, _) = tag(":")(input)?;
        let (input, string) = take(length)(input)?;
        Ok((input, String::from(string)))
    }

    pub fn parse(input: &str) -> IResult<&str, Value> {
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
}

impl TryFrom<&str> for Value {
    type Error = String;
    fn try_from(input: &str) -> Result<Self, Self::Error> {
        match parsers::parse(input) {
            Ok(("", value)) => Ok(value),
            Ok(_) => Err("Remainder".to_string()),
            Err(_) => Err("Error".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_string() {
        assert_eq!(
            Value::try_from("3:foo"),
            Ok(Value::String(String::from("foo")))
        );
    }

    #[test]
    fn decode_positive_integer() {
        assert_eq!(Value::try_from("i3e"), Ok(Value::Integer(3)));
    }

    #[test]
    fn decode_positive_multi_digit_integer() {
        assert_eq!(Value::try_from("i42e"), Ok(Value::Integer(42)));
    }

    #[test]
    fn decode_positive_negative_intiger() {
        assert_eq!(Value::try_from("i-1e"), Ok(Value::Integer(-1)));
    }

    // TODO: fail parsing for "i-0e", "i03e".

    #[test]
    fn decode_list() {
        assert_eq!(
            Value::try_from("l4:spam4:eggse"),
            Ok(Value::List(vec![
                Value::String(String::from("spam")),
                Value::String(String::from("eggs"))
            ]))
        );
    }

    #[test]
    fn decode_dictionary() {
        assert_eq!(
            Value::try_from("d3:cow3:moo4:spam4:eggse"),
            Ok(Value::Dictionary(BTreeMap::from([
                (String::from("cow"), Value::String(String::from("moo"))),
                (String::from("spam"), Value::String(String::from("eggs")))
            ])))
        );
    }
}
