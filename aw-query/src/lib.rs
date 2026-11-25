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

/// Whether `code` assigns to `TIMEINTERVAL` anywhere, including inside `if`
/// blocks. `query_bucket` reads its range from that variable, so a program that
/// reassigns it can read events outside the requested timeperiod, and a cached
/// result for the requested period would miss writes to the range it actually
/// read (ActivityWatch/aw-server-rust#882 item 7). Code that doesn't parse,
/// or exceeds the nesting limits, returns `false`; running it fails anyway.
pub fn assigns_timeinterval(code: &str) -> bool {
    use ast::Expr_;
    let program = match parse_bounded(lexer::Lexer::new(code)) {
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

/// Limits on how deeply a program can nest (ActivityWatch/aw-server-rust#882).
///
/// The interpreter, serde and the AST's generated `Drop` all recurse once per
/// tree level, so a deep enough program overflows the thread stack and aborts
/// the whole process, which `catch_unwind` cannot stop. The grammar is
/// left-recursive, so each binary operator in a statement can add a level, and
/// so can each bracket (calls add both a call and an argument-list node).
/// Bounding both bounds the tree depth while the parser builds it.
/// Measured on a 2 MiB thread stack: a debug build overflows at about
/// 150 levels and a release build passes 3000, so these keep debug builds safe
/// too. aw-webui's queries use functions such as `concat` rather than operators
/// and nest brackets only a few levels deep.
const MAX_OPERATORS_PER_STATEMENT: usize = 64;
const MAX_BRACKET_DEPTH: usize = 32;

fn parse_bounded<I: Iterator<Item = (lexer::Token, lexer::Span)>>(
    mut tokens: I,
) -> Result<ast::Program, QueryError> {
    use lexer::Token;
    // Keep the operators on the ancestor path, not in sibling expressions.
    // Semicolons and conditionals start independent statements in their scope.
    // Each frame holds (operators here, deepest completed child expression,
    // deepest comma-separated sibling). Sibling depth is retained for the
    // enclosing expression, but never added to the next sibling's operators.
    let mut operators = vec![(0usize, 0usize, 0usize)];
    let mut ends_operand = false;
    let mut limit_error = None;
    let checked = std::iter::from_fn(|| {
        if limit_error.is_some() {
            return None;
        }
        let (token, span) = tokens.next()?;
        match &token {
            Token::Plus | Token::Star | Token::Slash | Token::Percent | Token::Equals => {
                operators.last_mut().unwrap().0 += 1;
            }
            Token::Minus if ends_operand => operators.last_mut().unwrap().0 += 1,
            // A brace following an operand opens an if/elif body, not a dict
            // literal. Its condition and body are siblings in the AST.
            Token::LBrace if ends_operand => {
                *operators.last_mut().unwrap() = (0, 0, 0);
                operators.push((0, 0, 0));
            }
            Token::LParen | Token::LBracket | Token::LBrace => operators.push((0, 0, 0)),
            // Unbalanced/mismatched closers are left for the parser to report.
            Token::RParen | Token::RBracket | Token::RBrace if operators.len() > 1 => {
                let (own, child, sibling) = operators.pop().unwrap();
                let parent = operators.last_mut().unwrap();
                parent.1 = parent.1.max((own + child).max(sibling));
            }
            Token::Comma => {
                let frame = operators.last_mut().unwrap();
                frame.2 = frame.2.max(frame.0 + frame.1);
                frame.0 = 0;
                frame.1 = 0;
            }
            Token::Semi | Token::If | Token::ElseIf | Token::Else => {
                *operators.last_mut().unwrap() = (0, 0, 0);
            }
            _ => (),
        }
        let path_operators = operators
            .iter()
            .rev()
            .fold(0, |depth, &(own, child, sibling)| {
                sibling.max(own + child.max(depth))
            });
        let message = if path_operators > MAX_OPERATORS_PER_STATEMENT {
            Some(format!(
                "an expression path has more than {MAX_OPERATORS_PER_STATEMENT} operators"
            ))
        } else if operators.len() > MAX_BRACKET_DEPTH + 1 {
            Some(format!(
                "brackets are nested more than {MAX_BRACKET_DEPTH} deep"
            ))
        } else {
            None
        };
        if let Some(message) = message {
            limit_error = Some(QueryError::ParsingError(message));
            return None;
        }
        ends_operand = matches!(
            token,
            Token::Ident(_)
                | Token::Number(_)
                | Token::Bool(_)
                | Token::String(_)
                | Token::RParen
                | Token::RBracket
                | Token::RBrace
        );
        Some((token, span))
    });
    let program = parser::parse(checked);
    if let Some(error) = limit_error {
        return Err(error);
    }
    program.map_err(|e| {
        // TODO: Improve parsing error message
        warn!("ParsingError: {:?}", e);
        QueryError::ParsingError(format!("{e:?}"))
    })
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
    let program = parse_bounded(lexer::Lexer::new(code))?;
    let result = interpret::interpret_prog(program, ti, ds, deadline)?;
    if result.contains_function() {
        return Err(QueryError::InvalidType(
            "A query can't return a function; call it instead".to_string(),
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod nesting_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn parser_stops_reading_at_the_first_syntax_error() {
        let reads = Cell::new(0);
        let tokens =
            lexer::Lexer::new("return ; return 1;").inspect(|_| reads.set(reads.get() + 1));
        assert!(matches!(
            parse_bounded(tokens),
            Err(QueryError::ParsingError(_))
        ));
        assert_eq!(reads.get(), 2);
    }

    #[test]
    fn parser_stops_reading_at_the_nesting_limit() {
        let reads = Cell::new(0);
        let code = format!("return {}1{};", "[".repeat(100_000), "]".repeat(100_000));
        let tokens = lexer::Lexer::new(&code).inspect(|_| reads.set(reads.get() + 1));
        assert!(matches!(
            parse_bounded(tokens),
            Err(QueryError::ParsingError(_))
        ));
        assert_eq!(reads.get(), MAX_BRACKET_DEPTH + 2);
    }
}
