use std::time::Instant;
use std::{borrow::Cow, collections::HashMap};

use crate::functions;

use aw_datastore::Datastore;
use aw_models::TimeInterval;

use crate::ast::*;
use crate::DataType;
use crate::QueryError;

pub type VarEnv = HashMap<String, DataType>;

fn init_env(ti: &TimeInterval) -> VarEnv {
    let mut env = HashMap::new();
    env.insert("TIMEINTERVAL".to_string(), DataType::String(ti.to_string()));
    functions::fill_env(&mut env);
    env
}

pub fn interpret_prog(
    p: Program,
    ti: &TimeInterval,
    ds: &Datastore,
    deadline: Option<Instant>,
) -> Result<DataType, QueryError> {
    let mut env = init_env(ti);
    let total = p.stmts.len();
    for (i, expr) in p.stmts.into_iter().enumerate() {
        check_deadline(deadline, &format!("statement {} of {total}", i + 1))?;
        interpret_expr(&mut env, ds, expr, deadline)?;
    }
    // The last statement may have overrun the budget inside a function that
    // finished anyway; don't report that as a successful result.
    check_deadline(deadline, "returning the result")?;
    match env.remove("RETURN") {
        Some(ret) => Ok(ret),
        None => Err(QueryError::EmptyQuery()),
    }
}

/// Fail if `deadline` has passed. Called before every statement, at the top
/// level and inside `if` blocks, around every function call, and by
/// long-running functions between units of work (events, rules, regex
/// matches), so a budget overrun stops at the next such checkpoint.
pub(crate) fn check_deadline(deadline: Option<Instant>, what: &str) -> Result<(), QueryError> {
    match deadline {
        Some(deadline) if Instant::now() >= deadline => {
            Err(QueryError::TimeBudgetExceeded(format!(
                "query stopped before {what}: time budget exceeded \
                 (split the query into shorter timeperiods, or raise query_timeout_secs)"
            )))
        }
        _ => Ok(()),
    }
}

/// Run a query function between two deadline checks. The function itself
/// receives the deadline too; the check afterwards catches a function that
/// has no internal checkpoints (or overshot inside one engine call) so its
/// late result is not used.
fn call_checked(
    fname: &str,
    deadline: Option<Instant>,
    call: impl FnOnce() -> Result<DataType, QueryError>,
) -> Result<DataType, QueryError> {
    check_deadline(deadline, &format!("calling {fname}"))?;
    let result = call()?;
    check_deadline(deadline, &format!("using the result of {fname}"))?;
    Ok(result)
}

fn interpret_expr(
    env: &mut HashMap<String, DataType>,
    ds: &Datastore,
    expr: Expr,
    deadline: Option<Instant>,
) -> Result<DataType, QueryError> {
    use crate::ast::Expr_::*;
    match expr.node {
        Add(a, b) => {
            let a_res = interpret_expr(env, ds, *a, deadline)?;
            let b_res = interpret_expr(env, ds, *b, deadline)?;
            let res = match a_res {
                DataType::Number(n1) => match b_res {
                    DataType::Number(n2) => DataType::Number(n1 + n2),
                    _ => {
                        return Err(QueryError::InvalidType(
                            "Cannot use + on something that is not a number with a number!"
                                .to_string(),
                        ))
                    }
                },
                DataType::List(mut l1) => match b_res {
                    DataType::List(mut l2) => {
                        l1.append(&mut l2);
                        DataType::List(l1)
                    }
                    _ => {
                        return Err(QueryError::InvalidType(
                            "Cannot use + on something that is not a list with a list!".to_string(),
                        ))
                    }
                },
                DataType::String(s1) => match b_res {
                    DataType::String(s2) => {
                        let mut new_string = s1;
                        new_string.push_str(&s2);
                        DataType::String(new_string)
                    }
                    _ => {
                        return Err(QueryError::InvalidType(
                            "Cannot use + on something that is not a list with a list!".to_string(),
                        ))
                    }
                },
                _ => {
                    return Err(QueryError::InvalidType(
                        "Cannot use + on something that is not a number, list or string!"
                            .to_string(),
                    ))
                }
            };
            Ok(res)
        }
        Sub(a, b) => {
            let a_res = interpret_expr(env, ds, *a, deadline)?;
            let b_res = interpret_expr(env, ds, *b, deadline)?;
            let a_num = match a_res {
                DataType::Number(n) => n,
                _ => {
                    return Err(QueryError::InvalidType(
                        "Cannot sub something that is not a number!".to_string(),
                    ))
                }
            };
            let b_num = match b_res {
                DataType::Number(n) => n,
                _ => {
                    return Err(QueryError::InvalidType(
                        "Cannot sub something that is not a number!".to_string(),
                    ))
                }
            };
            Ok(DataType::Number(a_num - b_num))
        }
        Mul(a, b) => {
            let a_res = interpret_expr(env, ds, *a, deadline)?;
            let b_res = interpret_expr(env, ds, *b, deadline)?;
            let a_num = match a_res {
                DataType::Number(n) => n,
                _ => {
                    return Err(QueryError::InvalidType(
                        "Cannot sub something that is not a number!".to_string(),
                    ))
                }
            };
            let b_num = match b_res {
                DataType::Number(n) => n,
                _ => {
                    return Err(QueryError::InvalidType(
                        "Cannot sub something that is not a number!".to_string(),
                    ))
                }
            };
            Ok(DataType::Number(a_num * b_num))
        }
        Div(a, b) => {
            let a_res = interpret_expr(env, ds, *a, deadline)?;
            let b_res = interpret_expr(env, ds, *b, deadline)?;
            let a_num = match a_res {
                DataType::Number(n) => n,
                _ => {
                    return Err(QueryError::InvalidType(
                        "Cannot sub something that is not a number!".to_string(),
                    ))
                }
            };
            let b_num = match b_res {
                DataType::Number(n) => n,
                _ => {
                    return Err(QueryError::InvalidType(
                        "Cannot sub something that is not a number!".to_string(),
                    ))
                }
            };
            if b_num == 0.0 {
                return Err(QueryError::MathError(
                    "Tried to divide by zero!".to_string(),
                ));
            }
            Ok(DataType::Number(a_num / b_num))
        }
        Mod(a, b) => {
            let a_res = interpret_expr(env, ds, *a, deadline)?;
            let b_res = interpret_expr(env, ds, *b, deadline)?;
            let a_num = match a_res {
                DataType::Number(n) => n,
                _ => {
                    return Err(QueryError::InvalidType(
                        "Cannot sub something that is not a number!".to_string(),
                    ))
                }
            };
            let b_num = match b_res {
                DataType::Number(n) => n,
                _ => {
                    return Err(QueryError::InvalidType(
                        "Cannot sub something that is not a number!".to_string(),
                    ))
                }
            };
            Ok(DataType::Number(a_num % b_num))
        }
        Equal(lhs, rhs) => {
            let lhs_res = interpret_expr(env, ds, *lhs, deadline)?;
            let rhs_res = interpret_expr(env, ds, *rhs, deadline)?;
            Ok(DataType::Bool(lhs_res.query_eq(&rhs_res)?))
        }
        Assign(var, b) => {
            let val = interpret_expr(env, ds, *b, deadline)?;
            env.insert(var, val);
            Ok(DataType::None())
        }
        // This clone is the one necessary copy per variable reference: the env
        // must retain the value since it may be referenced again, while
        // function arguments and transforms need owned events.
        Var(var) => match env.get(&var) {
            Some(v) => Ok(v.clone()),
            None => Err(QueryError::VariableNotDefined(var.to_string())),
        },
        Bool(lit) => Ok(DataType::Bool(lit)),
        Number(lit) => Ok(DataType::Number(lit)),
        String(litstr) => Ok(DataType::String(litstr)),
        Return(e) => {
            let val = interpret_expr(env, ds, *e, deadline)?;
            // TODO: Once RETURN is deprecated we can fix this
            env.insert("RETURN".to_string(), val);
            Ok(DataType::None())
        }
        If(ifs) => {
            for (cond, block) in ifs {
                let c = interpret_expr(env, ds, *cond, deadline)?;
                if c.query_eq(&DataType::Bool(true))? {
                    for expr in block {
                        check_deadline(deadline, "the next statement of an if block")?;
                        interpret_expr(env, ds, expr, deadline)?;
                    }
                    break;
                }
            }
            Ok(DataType::None())
        }
        Function(fname, e) => {
            // Borrow variables only when every argument is an atom. Evaluating
            // arbitrary arguments may assign to the environment, including the
            // function binding itself, so those retain the owned evaluation path.
            if let Expr_::List(exprs) = &e.node {
                if exprs.iter().all(|expr| {
                    matches!(
                        expr.node,
                        Expr_::Var(_) | Expr_::Bool(_) | Expr_::Number(_) | Expr_::String(_)
                    )
                }) {
                    if let Some(DataType::ReadOnlyFunction(_, fun)) = env.get(&fname) {
                        let args: Result<Vec<Cow<'_, DataType>>, QueryError> = exprs
                            .iter()
                            .map(|expr| {
                                Ok(match &expr.node {
                                    Expr_::Var(name) => {
                                        Cow::Borrowed(env.get(name).ok_or_else(|| {
                                            QueryError::VariableNotDefined(name.clone())
                                        })?)
                                    }
                                    Expr_::Bool(value) => Cow::Owned(DataType::Bool(*value)),
                                    Expr_::Number(value) => Cow::Owned(DataType::Number(*value)),
                                    Expr_::String(value) => {
                                        Cow::Owned(DataType::String(value.clone()))
                                    }
                                    _ => unreachable!(),
                                })
                            })
                            .collect();
                        let args = args?;
                        let refs: Vec<_> = args.iter().map(|arg| arg.as_ref()).collect();
                        let fun = *fun;
                        return call_checked(&fname, deadline, || fun(&refs, env, ds, deadline));
                    }
                }
            }
            let args = match interpret_expr(env, ds, *e, deadline)? {
                DataType::List(l) => l,
                _ => unreachable!(),
            };
            let var = match env.get(&fname[..]) {
                Some(v) => v,
                None => return Err(QueryError::VariableNotDefined(fname.clone())),
            };
            match var {
                DataType::Function(_, fun) => {
                    call_checked(&fname, deadline, || fun(args, env, ds, deadline))
                }
                DataType::ReadOnlyFunction(_, fun) => {
                    let refs: Vec<_> = args.iter().collect();
                    call_checked(&fname, deadline, || fun(&refs, env, ds, deadline))
                }
                _ => Err(QueryError::InvalidType(fname.to_string())),
            }
        }
        List(list) => {
            let mut l = Vec::new();
            for entry in list {
                let res = interpret_expr(env, ds, entry, deadline)?;
                l.push(res);
            }
            Ok(DataType::List(l))
        }
        Dict(d) => {
            let mut dict = HashMap::new();
            for (key, val_uninterpreted) in d {
                let val = interpret_expr(env, ds, val_uninterpreted, deadline)?;
                dict.insert(key, val);
            }
            Ok(DataType::Dict(dict))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_deadline_stops_before_the_next_statement() {
        let ds = Datastore::new_in_memory(false);
        let ti =
            TimeInterval::new_from_string("2000-01-01T00:00:00Z/2000-01-02T00:00:00Z").unwrap();
        let program = crate::parser::parse(crate::lexer::Lexer::new("RETURN = 1;")).unwrap();
        let expired = Instant::now() - std::time::Duration::from_secs(1);
        match interpret_prog(program, &ti, &ds, Some(expired)) {
            Err(QueryError::TimeBudgetExceeded(msg)) => {
                assert!(msg.contains("statement 1 of 1"), "{msg}")
            }
            other => panic!("expected TimeBudgetExceeded, got {other:?}"),
        }
        // Statements inside an if block are checked too: run the parsed `if`
        // expression directly, bypassing the top-level check.
        let mut program =
            crate::parser::parse(crate::lexer::Lexer::new("if True { RETURN = 1; }")).unwrap();
        let if_expr = program.stmts.remove(0);
        let mut env = init_env(&ti);
        match interpret_expr(&mut env, &ds, if_expr, Some(expired)) {
            Err(QueryError::TimeBudgetExceeded(msg)) => {
                assert!(msg.contains("if block"), "{msg}")
            }
            other => panic!("expected TimeBudgetExceeded, got {other:?}"),
        }
        assert!(!env.contains_key("RETURN"), "block body must not have run");

        // No deadline, or one in the future, runs the program as before.
        let program = crate::parser::parse(crate::lexer::Lexer::new("RETURN = 1;")).unwrap();
        let far = Instant::now() + std::time::Duration::from_secs(60);
        assert!(interpret_prog(program, &ti, &ds, Some(far)).is_ok());
    }

    fn run(code: &str, env: &mut VarEnv, ds: &Datastore) -> DataType {
        let program = crate::parser::parse(crate::lexer::Lexer::new(code)).unwrap();
        for expr in program.stmts {
            interpret_expr(env, ds, expr, None).unwrap();
        }
        env.remove("RETURN").unwrap_or(DataType::None())
    }

    #[test]
    fn read_only_arguments_borrow_the_original_value_and_support_aliases() {
        fn check(
            args: &[&DataType],
            env: &VarEnv,
            _: &Datastore,
            _: Option<Instant>,
        ) -> Result<DataType, QueryError> {
            assert!(std::ptr::eq(args[0], env.get("events").unwrap()));
            Ok(DataType::Bool(true))
        }
        let ds = Datastore::new_in_memory(false);
        let mut env = VarEnv::new();
        env.insert(
            "check".into(),
            DataType::ReadOnlyFunction("check".into(), check),
        );
        env.insert(
            "events".into(),
            DataType::List(vec![DataType::Event(aw_models::Event::default())]),
        );
        assert_eq!(
            run("alias = check; return alias(events);", &mut env, &ds),
            DataType::Bool(true)
        );
        assert!(env.contains_key("events"));
    }

    #[test]
    fn side_effecting_arguments_keep_snapshots_and_resolve_the_function_after_evaluation() {
        fn check(
            args: &[&DataType],
            env: &VarEnv,
            _: &Datastore,
            _: Option<Instant>,
        ) -> Result<DataType, QueryError> {
            assert_eq!(args[0], &DataType::List(vec![DataType::Number(1.0)]));
            assert_eq!(env["items"], DataType::List(vec![DataType::Number(2.0)]));
            Ok(DataType::Bool(true))
        }
        let ds = Datastore::new_in_memory(false);
        let mut env = VarEnv::new();
        env.insert(
            "check".into(),
            DataType::ReadOnlyFunction("check".into(), check),
        );
        // Assignment nodes are supported by the interpreter, although the
        // current parser only emits them at statement level.
        let expr = |node| Expr {
            span: crate::lexer::Span {
                lo: 0,
                hi: 0,
                line: 1,
            },
            node,
        };
        for rebind in [false, true] {
            env.insert("items".into(), DataType::List(vec![DataType::Number(1.0)]));
            env.insert(
                "f".into(),
                if rebind {
                    DataType::Number(0.0)
                } else {
                    env["check"].clone()
                },
            );
            let mut args = vec![
                expr(Expr_::Var("items".into())),
                expr(Expr_::Assign(
                    "items".into(),
                    Box::new(expr(Expr_::List(vec![expr(Expr_::Number(2.0))]))),
                )),
            ];
            if rebind {
                args.push(expr(Expr_::Assign(
                    "f".into(),
                    Box::new(expr(Expr_::Var("check".into()))),
                )));
            }
            let call = expr(Expr_::Function(
                "f".into(),
                Box::new(expr(Expr_::List(args))),
            ));
            assert_eq!(
                interpret_expr(&mut env, &ds, call, None).unwrap(),
                DataType::Bool(true)
            );
        }
    }

    #[test]
    fn read_only_builtins_keep_values_available_and_accept_computed_arguments() {
        let ds = Datastore::new_in_memory(false);
        let mut env = VarEnv::new();
        functions::fill_env(&mut env);
        let mut event = aw_models::Event::default();
        event.duration = chrono::Duration::seconds(3);
        env.insert(
            "events".into(),
            DataType::List(vec![DataType::Event(event)]),
        );
        assert_eq!(run("total = sum_durations; a = total(events); b = total(events + events); return a + b;", &mut env, &ds), DataType::Number(9.0));
        assert_eq!(
            run(
                "values = [1, 2]; f = contains; a = f(values, 2); return [a, values];",
                &mut env,
                &ds
            ),
            DataType::List(vec![
                DataType::Bool(true),
                DataType::List(vec![DataType::Number(1.0), DataType::Number(2.0)])
            ])
        );
    }

    fn expect_timeout(result: Result<DataType, QueryError>) -> String {
        match result {
            Err(QueryError::TimeBudgetExceeded(msg)) => msg,
            other => panic!("expected TimeBudgetExceeded, got {other:?}"),
        }
    }

    /// Events whose titles each cost the backtracking VM its full backtrack
    /// limit against `BACKTRACKING_RULE` (ActivityWatch/aw-server-rust#893).
    fn slow_events(n: usize) -> DataType {
        DataType::List(
            (0..n)
                .map(|i| {
                    let mut e = aw_models::Event::default();
                    e.data.insert(
                        "title".into(),
                        serde_json::json!(format!("{}b{i}", "a".repeat(100))),
                    );
                    DataType::Event(e)
                })
                .collect(),
        )
    }

    const BACKTRACKING_RULE: &str = "^(a|aa|(?=a)a)*$";

    fn regex_rule(pattern: &str) -> DataType {
        DataType::Dict(HashMap::from([
            ("type".to_string(), DataType::String("regex".into())),
            ("regex".to_string(), DataType::String(pattern.into())),
        ]))
    }

    #[test]
    fn aliased_classifiers_stop_inside_the_final_statement() {
        let ds = Datastore::new_in_memory(false);
        let ti =
            TimeInterval::new_from_string("2000-01-01T00:00:00Z/2000-01-02T00:00:00Z").unwrap();
        // 200 slow titles take far longer than the budget without checkpoints.
        let cases = [
            (
                "f = categorize; RETURN = f(events, [[[\"c\"], rule]]);",
                "before categorize",
            ),
            (
                "f = tag; RETURN = f(events, [[\"t\", rule]]);",
                "before tag",
            ),
            (
                "f = filter_keyvals_regex; RETURN = f(events, \"title\", pattern);",
                "before filter_keyvals_regex",
            ),
        ];
        for (code, expected) in cases {
            let program = crate::parser::parse(crate::lexer::Lexer::new(code)).unwrap();
            let mut env = init_env(&ti);
            env.insert("events".into(), slow_events(200));
            env.insert("rule".into(), regex_rule(BACKTRACKING_RULE));
            env.insert("pattern".into(), DataType::String(BACKTRACKING_RULE.into()));
            let started = Instant::now();
            let deadline = Some(started + std::time::Duration::from_millis(200));
            let result = program
                .stmts
                .into_iter()
                .try_for_each(|expr| interpret_expr(&mut env, &ds, expr, deadline).map(drop));
            // The alias reaches the function's own checkpoints, which name the
            // function rather than the alias `f`.
            let msg = expect_timeout(result.map(|_| DataType::None()));
            assert!(msg.contains(expected), "{code}: {msg}");
            assert!(
                !env.contains_key("RETURN"),
                "{code}: late result was stored"
            );
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "{code}: ran {:?}",
                started.elapsed()
            );
        }
    }

    #[test]
    fn a_function_that_overruns_cannot_return_a_result() {
        fn slow(_: &[&DataType], _: &VarEnv, _: &Datastore, _: Option<Instant>) -> QueryResult {
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(DataType::Number(1.0))
        }
        type QueryResult = Result<DataType, QueryError>;
        let ds = Datastore::new_in_memory(false);
        let mut env = VarEnv::new();
        env.insert(
            "slow".into(),
            DataType::ReadOnlyFunction("slow".into(), slow),
        );
        let call = crate::parser::parse(crate::lexer::Lexer::new("RETURN = slow();"))
            .unwrap()
            .stmts
            .remove(0);
        let deadline = Some(Instant::now() + std::time::Duration::from_millis(20));
        let msg = expect_timeout(interpret_expr(&mut env, &ds, call, deadline));
        assert!(msg.contains("using the result of slow"), "{msg}");
        assert!(!env.contains_key("RETURN"));
    }

    #[test]
    fn rule_compilation_checks_before_each_rule() {
        let rules = |n: usize| {
            DataType::List(
                (0..n)
                    .map(|i| {
                        DataType::List(vec![
                            DataType::List(vec![DataType::String(format!("c{i}"))]),
                            regex_rule(&format!("r{i}")),
                        ])
                    })
                    .collect(),
            )
        };
        let tag_rules = |n: usize| {
            DataType::List(
                (0..n)
                    .map(|i| {
                        DataType::List(vec![
                            DataType::String(format!("t{i}")),
                            regex_rule(&format!("r{i}")),
                        ])
                    })
                    .collect(),
            )
        };
        let stop_at = |fail_at: usize| {
            let mut calls = 0;
            move || {
                calls += 1;
                if calls == fail_at {
                    Err(QueryError::TimeBudgetExceeded(format!("check {calls}")))
                } else {
                    Ok(())
                }
            }
        };
        // 3 rules: one check before each and one after the last.
        for fail_at in 1..=4 {
            let expected = format!("check {fail_at}");
            match crate::datatype::category_rules_checked(rules(3), stop_at(fail_at)) {
                Err(QueryError::TimeBudgetExceeded(msg)) => assert_eq!(msg, expected),
                other => panic!(
                    "expected cancellation at {fail_at}, got {:?}",
                    other.is_ok()
                ),
            }
            match crate::datatype::tag_rules_checked(tag_rules(3), stop_at(fail_at)) {
                Err(QueryError::TimeBudgetExceeded(msg)) => assert_eq!(msg, expected),
                other => panic!(
                    "expected cancellation at {fail_at}, got {:?}",
                    other.is_ok()
                ),
            }
        }
        assert_eq!(
            crate::datatype::category_rules_checked(rules(3), stop_at(5))
                .unwrap()
                .len(),
            3
        );
    }
}
