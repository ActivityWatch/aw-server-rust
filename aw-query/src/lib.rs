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
    /// Combining values would build one larger than the interpreter's size
    /// bound (1 GiB, approximate), e.g. a string or list doubled by repeated
    /// `a = a + a;`.
    ValueTooLarge(String),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Whether `code` assigns to `TIMEINTERVAL` anywhere, including inside `if`
/// blocks. `query_bucket` reads its range from that variable, so a program that
/// reassigns it can read events outside the requested timeperiod, and a cached
/// result for the requested period would miss writes to the range it actually
/// read (ActivityWatch/aw-server-rust#882 item 7). Code that doesn't parse
/// returns `false`; running it fails anyway.
pub fn assigns_timeinterval(code: &str) -> bool {
    use ast::Expr_;
    let program = match parser::parse(lexer::Lexer::new(code)) {
        Ok(p) => p,
        Err(_) => return false,
    };
    // Consume the children as we walk: borrowing them would leave recursive
    // AST destruction on return. Drain the whole tree even after a match.
    let mut assigns = false;
    let mut stack = program.stmts;
    while let Some(expr) = stack.pop() {
        match expr.node {
            Expr_::Add(a, b)
            | Expr_::Sub(a, b)
            | Expr_::Mul(a, b)
            | Expr_::Div(a, b)
            | Expr_::Mod(a, b)
            | Expr_::Equal(a, b) => {
                stack.push(*a);
                stack.push(*b);
            }
            Expr_::Assign(var, e) => {
                assigns |= var == "TIMEINTERVAL";
                stack.push(*e);
            }
            Expr_::Function(_, e) | Expr_::Return(e) => stack.push(*e),
            Expr_::If(branches) => {
                for (cond, block) in branches {
                    stack.push(*cond);
                    stack.extend(block);
                }
            }
            Expr_::List(items) => stack.extend(items),
            Expr_::Dict(d) => stack.extend(d.into_values()),
            Expr_::Var(_) | Expr_::Bool(_) | Expr_::Number(_) | Expr_::String(_) => (),
        }
    }
    assigns
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
    let result = interpret::interpret_prog(program, ti, ds, deadline)?;
    if result.contains_function() {
        return Err(QueryError::InvalidType(
            "A query can't return a function; call it instead".to_string(),
        ));
    }
    Ok(result)
}
