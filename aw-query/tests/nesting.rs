//! Deeply nested programs must fail with a parse error, not abort the process
//! with a stack overflow (ActivityWatch/aw-server-rust#882, item 1).
//!
//! Each query runs on a thread with a 2 MiB stack, the default for spawned
//! threads and for the server's worker threads.

use aw_datastore::Datastore;
use aw_models::TimeInterval;
use aw_query::{DataType, QueryError};

fn run_on_small_stack(code: String) -> Result<DataType, QueryError> {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let ds = Datastore::new_in_memory(false);
            let ti =
                TimeInterval::new_from_string("1980-01-01T00:00:00Z/2080-01-02T00:00:00Z").unwrap();
            aw_query::query(&code, &ti, &ds)
        })
        .unwrap()
        .join()
        .expect("query thread panicked")
}

fn sum_of_ones(n: usize) -> String {
    format!("return {};", vec!["1"; n].join("+"))
}

fn nested_list(depth: usize) -> String {
    format!("return {}1{};", "[".repeat(depth), "]".repeat(depth))
}

#[test]
fn long_operator_chain_is_a_parse_error() {
    match run_on_small_stack(sum_of_ones(100_000)) {
        Err(QueryError::ParsingError(_)) => (),
        other => panic!("expected ParsingError, got {other:?}"),
    }
}

#[test]
fn one_operator_over_the_limit_is_a_parse_error() {
    match run_on_small_stack(sum_of_ones(66)) {
        Err(QueryError::ParsingError(_)) => (),
        other => panic!("expected ParsingError, got {other:?}"),
    }
    match run_on_small_stack(nested_list(33)) {
        Err(QueryError::ParsingError(_)) => (),
        other => panic!("expected ParsingError, got {other:?}"),
    }
}

#[test]
fn deep_brackets_are_a_parse_error() {
    match run_on_small_stack(nested_list(100_000)) {
        Err(QueryError::ParsingError(_)) => (),
        other => panic!("expected ParsingError, got {other:?}"),
    }
}

#[test]
fn programs_at_the_limits_still_run() {
    // 64 operators in one statement, inside 32 nested lists: the deepest tree
    // the limits admit.
    let code = format!(
        "return {}{}{};",
        "[".repeat(32),
        vec!["1"; 65].join("+"),
        "]".repeat(32)
    );
    let mut value = run_on_small_stack(code).expect("query at the limits failed");
    for _ in 0..32 {
        value = match value {
            DataType::List(mut items) if items.len() == 1 => items.remove(0),
            other => panic!("expected a one-element list, got {other:?}"),
        };
    }
    match value {
        DataType::Number(n) => assert_eq!(n, 65.0),
        other => panic!("expected 65, got {other:?}"),
    }
}

#[test]
fn the_operator_limit_is_per_statement() {
    let stmt = format!("a = {};", vec!["1"; 65].join("+"));
    let code = format!("{}return a;", stmt.repeat(20));
    match run_on_small_stack(code) {
        Ok(DataType::Number(n)) => assert_eq!(n, 65.0),
        other => panic!("expected 65, got {other:?}"),
    }
}
