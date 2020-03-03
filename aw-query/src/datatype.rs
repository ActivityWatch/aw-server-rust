use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::fmt;

use super::functions;
use super::QueryError;
use aw_models::Event;
use aw_transform::classify::{RegexRule, Rule};

use serde::Serializer;
use serde_json::value::Value;
use serde_json::Number;

// TODO: greater/less comparisons

#[derive(Clone, Serialize)]
#[serde(untagged)]
pub enum DataType {
    None(),
    Bool(bool),
    Number(f64),
    String(String),
    Event(Event),
    List(Vec<DataType>),
    Dict(HashMap<String, DataType>),
    #[serde(serialize_with = "serialize_function")]
    Function(String, functions::QueryFn),
}

fn serialize_function<S>(
    _element: &String,
    _fun: &functions::QueryFn,
    _serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    panic!("Query function was unevaluated and was attempted to be serialized, panic!");
    //element.id.serialize(serializer)
}

// Needed because of a limitation in rust where you cannot derive(Debug) on a
// enum which has a fn with reference parameters which our QueryFn has
// https://stackoverflow.com/questions/53380040/function-pointer-with-a-reference-argument-cannot-derive-debug
impl fmt::Debug for DataType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DataType::None() => write!(f, "None()"),
            DataType::Bool(b) => write!(f, "Bool({})", b),
            DataType::Number(n) => write!(f, "Number({})", n),
            DataType::String(s) => write!(f, "String({})", s),
            DataType::Event(e) => write!(f, "Event({:?})", e),
            DataType::List(l) => write!(f, "List({:?})", l),
            DataType::Dict(d) => write!(f, "Dict({:?})", d),
            DataType::Function(name, _fun) => write!(f, "Function({})", name),
        }
    }
}

/* Like eq, but raises an error when comparing between different types.
 * Should be used as often as possible */
impl DataType {
    pub fn query_eq(&self, other: &DataType) -> Result<bool, QueryError> {
        match (self, other) {
            // TODO: Comparisons of bool == num, bool == str
            (DataType::None(), DataType::None()) => Ok(false),
            (DataType::Bool(b1), DataType::Bool(b2)) => Ok(b1 == b2),
            (DataType::Number(n1), DataType::Number(n2)) => Ok(n1 == n2),
            (DataType::String(s1), DataType::String(s2)) => Ok(s1 == s2),
            (DataType::Event(e1), DataType::Event(e2)) => Ok(e1 == e2),
            (DataType::List(l1), DataType::List(l2)) => Ok(l1 == l2),
            (DataType::Dict(d1), DataType::Dict(d2)) => Ok(d1 == d2),
            // We do not care about comparing functions
            _ => Err(QueryError::InvalidType(format!(
                "Cannot compare values of different types {:?} and {:?}",
                self, other
            ))),
        }
    }
}

/* Required for query_eq when comparing two dicts */
impl PartialEq for DataType {
    fn eq(&self, other: &DataType) -> bool {
        match (self, other) {
            (DataType::None(), DataType::None()) => true,
            // TODO: Comparisons of bool == num, bool == str
            (DataType::Bool(b1), DataType::Bool(b2)) => b1 == b2,
            (DataType::Number(n1), DataType::Number(n2)) => n1 == n2,
            (DataType::String(s1), DataType::String(s2)) => s1 == s2,
            (DataType::Event(e1), DataType::Event(e2)) => e1 == e2,
            (DataType::List(l1), DataType::List(l2)) => l1 == l2,
            (DataType::Dict(d1), DataType::Dict(d2)) => d1 == d2,
            // We do not care about comparing functions
            _ => false,
        }
    }
}

impl TryFrom<&DataType> for Vec<DataType> {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        match value {
            DataType::List(ref s) => Ok(s.clone()),
            ref invalid_type => Err(QueryError::InvalidFunctionParameters(format!(
                "Expected function parameter of type List, got {:?}",
                invalid_type
            ))),
        }
    }
}

impl TryFrom<&DataType> for String {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        match value {
            DataType::String(s) => Ok(s.clone()),
            ref invalid_type => Err(QueryError::InvalidFunctionParameters(format!(
                "Expected function parameter of type String, list contains {:?}",
                invalid_type
            ))),
        }
    }
}

impl TryFrom<&DataType> for Vec<String> {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        let mut tagged_strings: Vec<DataType> = value.try_into()?;
        let mut strings = Vec::new();
        for string in tagged_strings.drain(..) {
            let s: String = (&string).try_into()?;
            strings.push(s);
        }
        return Ok(strings);
    }
}

impl TryFrom<&DataType> for Vec<Event> {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        let mut tagged_events: Vec<DataType> = value.try_into()?;
        let mut events = Vec::new();
        for event in tagged_events.drain(..) {
            match event {
                DataType::Event(e) => events.push(e.clone()),
                ref invalid_type => {
                    return Err(QueryError::InvalidFunctionParameters(format!(
                        "Expected function parameter of type List of Events, list contains {:?}",
                        invalid_type
                    )))
                }
            }
        }
        return Ok(events);
    }
}

impl TryFrom<&DataType> for Vec<(String, Rule)> {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        let mut tagged_lists: Vec<DataType> = value.try_into()?;
        let mut lists: Vec<(String, Rule)> = Vec::new();
        for list in tagged_lists.drain(..) {
            match list {
                DataType::List(ref l) => {
                    let tag: String = match l.get(0) {
                        Some(tag) => tag.try_into()?,
                        None => return Err(QueryError::InvalidFunctionParameters(
                            format!("Expected function parameter of type list of (tag, rule) tuples, list contains {:?}", l)))
                    };
                    let rule: Rule = match l.get(1) {
                        Some(rule) => rule.try_into()?,
                        None => return Err(QueryError::InvalidFunctionParameters(
                            format!("Expected function parameter of type list of (tag, rule) tuples, list contains {:?}", l)))
                    };
                    lists.push((tag, rule));
                }
                ref invalid_type => {
                    return Err(QueryError::InvalidFunctionParameters(format!(
                        "Expected function parameter of type list of (tag, rule) tuples, got {:?}",
                        invalid_type
                    )))
                }
            }
        }
        return Ok(lists);
    }
}

impl TryFrom<&DataType> for Vec<(Vec<String>, Rule)> {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        let mut tagged_lists: Vec<DataType> = value.try_into()?;
        let mut lists: Vec<(Vec<String>, Rule)> = Vec::new();
        for list in tagged_lists.drain(..) {
            match list {
                DataType::List(ref l) => {
                    let category: Vec<String> = match l.get(0) {
                        Some(category) => category.try_into()?,
                        None => return Err(QueryError::InvalidFunctionParameters(
                            format!("Expected function parameter of type list of (category, rule) tuples, list contains {:?}", l)))
                    };
                    let rule: Rule = match l.get(1) {
                        Some(rule) => rule.try_into()?,
                        None => return Err(QueryError::InvalidFunctionParameters(
                            format!("Expected function parameter of type list of (category, rule) tuples, list contains {:?}", l)))
                    };
                    lists.push((category, rule));
                }
                ref invalid_type => {
                    return Err(QueryError::InvalidFunctionParameters(format!(
                    "Expected function parameter of type list of (category, rule) tuples, got {:?}",
                    invalid_type
                )))
                }
            }
        }
        return Ok(lists);
    }
}

impl TryFrom<&DataType> for f64 {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        match value {
            DataType::Number(f) => Ok(*f),
            ref invalid_type => Err(QueryError::InvalidFunctionParameters(format!(
                "Expected function parameter of type Number, got {:?}",
                invalid_type
            ))),
        }
    }
}

impl TryFrom<&DataType> for usize {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        let f: f64 = value.try_into()?;
        Ok(f as usize)
    }
}

impl TryFrom<&DataType> for Value {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        match value {
            DataType::None() => Ok(Value::Null),
            DataType::Bool(b) => Ok(Value::Bool(*b)),
            DataType::Number(n) => Ok(Value::Number(Number::from_f64(*n).unwrap())),
            DataType::String(s) => Ok(Value::String(s.to_string())),
            DataType::List(_l) => {
                let mut tagged_values: Vec<DataType> = value.try_into()?;
                let mut values: Vec<Value> = Vec::new();
                for value in tagged_values.drain(..) {
                    values.push((&value).try_into()?);
                }
                return Ok(Value::Array(values));
            }
            ref invalid_type => Err(QueryError::InvalidFunctionParameters(format!(
                "Query2 support for parsing values is limited, does not support parsing {:?}",
                invalid_type
            ))),
        }
    }
}

impl TryFrom<&DataType> for Vec<Value> {
    type Error = QueryError;
    fn try_from(value: &DataType) -> Result<Self, Self::Error> {
        let mut tagged_values: Vec<DataType> = value.try_into()?;
        let mut values: Vec<Value> = Vec::new();
        for value in tagged_values.drain(..) {
            values.push((&value).try_into()?);
        }
        return Ok(values);
    }
}

impl TryFrom<&DataType> for Rule {
    type Error = QueryError;

    fn try_from(data: &DataType) -> Result<Self, Self::Error> {
        let obj = match data {
            DataType::Dict(dict) => dict,
            _ => {
                return Err(QueryError::InvalidFunctionParameters(format!(
                    "Expected rule dict, got {:?}",
                    data
                )))
            }
        };
        let rtype_val = match obj.get("type") {
            Some(rtype) => rtype,
            None => {
                return Err(QueryError::InvalidFunctionParameters(
                    "rule does not have a type".to_string(),
                ))
            }
        };
        let rtype = match rtype_val {
            DataType::String(s) => s,
            _ => {
                return Err(QueryError::InvalidFunctionParameters(
                    "rule type is not a string".to_string(),
                ))
            }
        };
        if rtype == "none" {
            return Ok(Self::None);
        } else if rtype == "regex" {
            let regex_val = match obj.get("regex") {
                Some(regex_val) => regex_val,
                None => {
                    return Err(QueryError::InvalidFunctionParameters(
                        "regex rule is missing the 'regex' field".to_string(),
                    ))
                }
            };
            let regex_str = match regex_val {
                DataType::String(s) => s,
                _ => {
                    return Err(QueryError::InvalidFunctionParameters(
                        "the regex field of the regex rule is not a string".to_string(),
                    ))
                }
            };
            let ignore_case_val = match obj.get("ignore_case") {
                Some(case_val) => case_val,
                None => &DataType::Bool(false),
            };
            let ignore_case = match ignore_case_val {
                DataType::Bool(b) => b,
                _ => {
                    return Err(QueryError::InvalidFunctionParameters(
                        "the ignore_case field of the regex rule is not a bool".to_string(),
                    ))
                }
            };
            let regex_rule = match RegexRule::new(regex_str, *ignore_case) {
                Ok(regex_rule) => regex_rule,
                Err(err) => {
                    return Err(QueryError::RegexCompileError(format!(
                        "Failed to compile regex string '{}': '{:?}",
                        regex_str, err
                    )))
                }
            };
            return Ok(Self::Regex(regex_rule));
        } else {
            return Err(QueryError::InvalidFunctionParameters(format!(
                "Unknown rule type '{}'",
                rtype
            )));
        }
    }
}
