#[macro_use]
extern crate log;
extern crate serde;
extern crate serde_json;

use std::fmt;
use std::time::Instant;

use aw_models::TimeInterval;

use aw_datastore::Datastore;

pub mod datatype;

mod ast;
mod functions;
mod interpret;
mod lexer;
// `unknown_lints` and `clippy::block_scrutinee` are already allowed inside
// parser.rs itself (inner `#![allow(...)]`); listing them here too is what
// triggered clippy::duplicated_attributes (ActivityWatch/aw-server-rust#771).
#[allow(
    clippy::match_single_binding,
    clippy::redundant_closure_call,
    unused_braces
)]
mod parser;

pub use crate::datatype::DataType;
pub use crate::interpret::VarEnv;

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
    BucketNotFound(String),
    BucketQueryError(String),
    RegexCompileError(String),
    /// The caller's time budget ran out before the program finished; the
    /// interpreter checks it between statements, so a single statement is
    /// never interrupted.
    TimeBudgetExceeded(String),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

pub fn query(code: &str, ti: &TimeInterval, ds: &Datastore) -> Result<DataType, QueryError> {
    query_with_deadline(code, ti, ds, None)
}

/// Like [`query`], but gives up with [`QueryError::TimeBudgetExceeded`] once
/// `deadline` has passed. The check runs before every statement, so the
/// program stops at the next statement boundary rather than mid-transform;
/// a `query_bucket` read that has already started always completes.
pub fn query_with_deadline(
    code: &str,
    ti: &TimeInterval,
    ds: &Datastore,
    deadline: Option<Instant>,
) -> Result<DataType, QueryError> {
    let lexer = lexer::Lexer::new(code);
    let program = match parser::parse(lexer) {
        Ok(p) => p,
        Err(e) => {
            // TODO: Improve parsing error message
            warn!("ParsingError: {:?}", e);
            return Err(QueryError::ParsingError(format!("{e:?}")));
        }
    };
    interpret::interpret_prog(program, ti, ds, deadline)
}
