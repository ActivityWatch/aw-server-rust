#[macro_use]
extern crate log;
extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;

use std::fmt;

use aw_models::TimeInterval;

use aw_datastore::Datastore;

pub mod datatype;

mod ast;
mod functions;
mod interpret;
mod lexer;
mod parser;

pub use crate::datatype::DataType;

// TODO: add line numbers to errors
// (works during lexing, but not during parsing I believe)

#[derive(Debug)]
pub enum QueryError {
    // Parser
    ParsingError(String),

    // Execution
    EmptyQuery(),
    VariableNotDefined(String),
    MathError(String),
    InvalidType(String),
    InvalidFunctionParameters(String),
    TimeIntervalError(String),
    BucketQueryError(String),
    RegexCompileError(String),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub fn query<'a>(code: &str, ti: &TimeInterval, ds: &Datastore) -> Result<DataType, QueryError> {
    let lexer = lexer::Lexer::new(code);
    let program = match parser::parse(lexer) {
        Ok(p) => p,
        Err(e) => {
            // TODO: Improve parsing error message
            warn!("ParsingError: {:?}", e);
            return Err(QueryError::ParsingError(format!("{:?}", e)));
        }
    };
    interpret::interpret_prog(&program, ti, ds)
}
