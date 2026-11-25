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
    // 64 operators inside 32 nested lists. Calls add an extra argument-list
    // layer and are covered separately below.
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

#[test]
fn negative_literals_do_not_consume_the_operator_budget() {
    let rules = vec!["{\"priority\": -1}"; 100].join(",");
    match run_on_small_stack(format!("return [{rules}];")) {
        Ok(DataType::List(items)) => assert_eq!(items.len(), 100),
        other => panic!("expected 100 rules, got {other:?}"),
    }
}

#[test]
fn sequential_conditionals_have_independent_operator_budgets() {
    let code = format!("{}return a;", "if true { a = 1+1; }".repeat(100));
    match run_on_small_stack(code) {
        Ok(DataType::Number(n)) => assert_eq!(n, 2.0),
        other => panic!("expected 2, got {other:?}"),
    }
}

#[test]
fn statements_inside_a_block_have_independent_operator_budgets() {
    let stmt = format!("a = {};", vec!["1"; 65].join("+"));
    let code = format!("if true {{ {} }} return a;", stmt.repeat(20));
    match run_on_small_stack(code) {
        Ok(DataType::Number(n)) => assert_eq!(n, 65.0),
        other => panic!("expected 65, got {other:?}"),
    }
}

#[test]
fn nested_calls_at_the_combined_limits_still_run() {
    let code = format!(
        "return {}{}{};",
        "print(".repeat(32),
        vec!["1"; 65].join("+"),
        ")".repeat(32)
    );
    match run_on_small_stack(code) {
        Ok(DataType::None()) => (),
        other => panic!("expected None, got {other:?}"),
    }
}

#[test]
fn closed_subexpressions_cannot_hide_operator_depth() {
    // Each scope individually has one operator, but the left children form
    // one deep tree even after their closing parentheses have been read.
    let mut expr = "1".to_string();
    for _ in 0..32 {
        expr = format!("({expr}+1)");
    }
    let code = format!("return {expr}{};", "+1".repeat(33));
    assert!(matches!(
        run_on_small_stack(code),
        Err(QueryError::ParsingError(_))
    ));
}

#[test]
fn subtraction_is_counted_but_negative_rhs_is_not() {
    let code = format!("return 1{};", "--1".repeat(64));
    match run_on_small_stack(code) {
        Ok(DataType::Number(n)) => assert_eq!(n, 65.0),
        other => panic!("expected 65, got {other:?}"),
    }
    let code = format!("return 1{};", "--1".repeat(65));
    assert!(matches!(
        run_on_small_stack(code),
        Err(QueryError::ParsingError(_))
    ));
}

#[test]
fn conditional_condition_and_body_have_independent_budgets() {
    let expr = vec!["1"; 65].join("+");
    let condition = format!("{} == 64", vec!["1"; 64].join("+"));
    let code = format!("if {condition} {{ a = {expr}; }} return a;");
    match run_on_small_stack(code) {
        Ok(DataType::Number(n)) => assert_eq!(n, 65.0),
        other => panic!("expected 65, got {other:?}"),
    }
}
