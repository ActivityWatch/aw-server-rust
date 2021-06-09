#[macro_use]
extern crate log;
extern crate serde;
extern crate serde_json;

use std::fmt;

use aw_models::TimeInterval;

use aw_datastore::Datastore;

pub mod datatype;

mod ast;
mod functions;
mod interpret;
mod lexer;
#[allow(
    clippy::match_single_binding,
    clippy::redundant_closure_call,
    unused_braces
)]
mod parser;
mod preprocess;
mod varenv;

pub use crate::datatype::DataType;
pub use crate::varenv::VarEnv;

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

fn init_env(ti: &TimeInterval) -> VarEnv {
    let mut env = VarEnv::new();
    env.declare_static("TIMEINTERVAL".to_string(), DataType::String(ti.to_string()));
    functions::fill_env(&mut env);
    env
}

pub fn query(code: &str, ti: &TimeInterval, ds: &Datastore) -> Result<DataType, QueryError> {
    let lexer = lexer::Lexer::new(code);
    let program = match parser::parse(lexer) {
        Ok(p) => p,
        Err(e) => {
            // TODO: Improve parsing error message
            warn!("ParsingError: {:?}", e);
            return Err(QueryError::ParsingError(format!("{:?}", e)));
        }
    };
    let mut env = init_env(ti);
    preprocess::preprocess_prog(&program, &mut env, ds)?;
    interpret::interpret_prog(program, &mut env, ds)
}
