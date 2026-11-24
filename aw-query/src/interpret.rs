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
    match env.remove("RETURN") {
        Some(ret) => Ok(ret),
        None => Err(QueryError::EmptyQuery()),
    }
}

/// Fail if `deadline` has passed. Called before every statement, at the top
/// level and inside `if` blocks, so a budget overrun stops at the next
/// statement boundary wherever it happens.
fn check_deadline(deadline: Option<Instant>, what: &str) -> Result<(), QueryError> {
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

/// Upper bound, in approximate heap bytes, on any value the interpreter
/// builds by combining others: `+` on strings and lists, and list or dict
/// literals (which include function argument lists, so `concat` is covered).
///
/// Without it `a = a + a;` (or `a = [a, a];`, or `a = concat(a, a);`) doubles
/// a value per statement, so about 30 statements in a sub-1 KB query exhaust
/// the server's memory long before the per-statement time budget is checked
/// (ActivityWatch/aw-server-rust#882 item 6). Values read from the datastore
/// are only limited where they are combined; 1 GiB is several million events,
/// far beyond any real query result. Unit tests use 1 MiB so they can drive a
/// value past the bound quickly.
#[cfg(not(test))]
pub(crate) const MAX_VALUE_SIZE: usize = 1 << 30;
#[cfg(test)]
pub(crate) const MAX_VALUE_SIZE: usize = 1 << 20;

/// Approximate heap size of `value`, or `None` once it exceeds `limit`.
///
/// Iterative, so a deeply nested value can't overflow the stack here, and it
/// stops as soon as `limit` is passed, so checking a huge value costs at most
/// `limit` worth of walking.
fn approx_size_within(value: &DataType, limit: usize) -> Option<usize> {
    use serde_json::Value;
    const NODE: usize = std::mem::size_of::<DataType>();
    const JSON_NODE: usize = std::mem::size_of::<Value>();
    let mut total = 0usize;
    let mut values = vec![value];
    let mut json: Vec<&Value> = Vec::new();
    let add = |n: usize, total: &mut usize| -> bool {
        *total = total.saturating_add(n);
        *total <= limit
    };
    while let Some(v) = values.pop() {
        let extra = match v {
            DataType::String(s) => s.len(),
            DataType::List(l) => {
                values.extend(l.iter());
                0
            }
            DataType::Dict(d) => {
                let mut keys = 0usize;
                for (k, v) in d {
                    keys = keys.saturating_add(k.len() + std::mem::size_of::<String>());
                    values.push(v);
                }
                keys
            }
            DataType::Event(e) => {
                let mut keys = 0usize;
                for (k, v) in e.data.iter() {
                    keys = keys.saturating_add(k.len() + std::mem::size_of::<String>());
                    json.push(v);
                }
                keys
            }
            DataType::Function(name, _) | DataType::ReadOnlyFunction(name, _) => name.len(),
            DataType::None() | DataType::Bool(_) | DataType::Number(_) => 0,
        };
        if !add(NODE + extra, &mut total) {
            return None;
        }
        while let Some(j) = json.pop() {
            let extra = match j {
                Value::String(s) => s.len(),
                Value::Array(a) => {
                    json.extend(a.iter());
                    0
                }
                Value::Object(o) => {
                    let mut keys = 0usize;
                    for (k, v) in o {
                        keys = keys.saturating_add(k.len() + std::mem::size_of::<String>());
                        json.push(v);
                    }
                    keys
                }
                Value::Null | Value::Bool(_) | Value::Number(_) => 0,
            };
            if !add(JSON_NODE + extra, &mut total) {
                return None;
            }
        }
    }
    Some(total)
}

/// Add `value`'s approximate size to `used`, failing with
/// [`QueryError::ValueTooLarge`] once the result being built (`what`) would
/// pass [`MAX_VALUE_SIZE`].
fn charge_size(used: &mut usize, value: &DataType, what: &str) -> Result<(), QueryError> {
    let remaining = MAX_VALUE_SIZE.saturating_sub(*used);
    match approx_size_within(value, remaining) {
        Some(size) => {
            *used += size;
            Ok(())
        }
        None => Err(QueryError::ValueTooLarge(format!(
            "{what} would be larger than {} MiB",
            MAX_VALUE_SIZE >> 20
        ))),
    }
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
            if matches!(
                (&a_res, &b_res),
                (DataType::List(_), DataType::List(_)) | (DataType::String(_), DataType::String(_))
            ) {
                let mut used = 0;
                charge_size(&mut used, &a_res, "the result of +")?;
                charge_size(&mut used, &b_res, "the result of +")?;
            }
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
            if b_num == 0.0 {
                return Err(QueryError::MathError(
                    "Cannot take the remainder with a zero divisor".to_string(),
                ));
            }
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
                        return fun(&refs, env, ds);
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
                DataType::Function(_, fun) => fun(args, env, ds),
                DataType::ReadOnlyFunction(_, fun) => {
                    let refs: Vec<_> = args.iter().collect();
                    fun(&refs, env, ds)
                }
                _ => Err(QueryError::InvalidType(fname.to_string())),
            }
        }
        List(list) => {
            let mut l = Vec::new();
            let mut used = 0;
            for entry in list {
                let res = interpret_expr(env, ds, entry, deadline)?;
                charge_size(&mut used, &res, "a list (or a function's arguments)")?;
                l.push(res);
            }
            Ok(DataType::List(l))
        }
        Dict(d) => {
            let mut dict = HashMap::new();
            let mut used: usize = 0;
            for (key, val_uninterpreted) in d {
                let val = interpret_expr(env, ds, val_uninterpreted, deadline)?;
                used = used.saturating_add(key.len());
                charge_size(&mut used, &val, "a dict")?;
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
        fn check(args: &[&DataType], env: &VarEnv, _: &Datastore) -> Result<DataType, QueryError> {
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
        fn check(args: &[&DataType], env: &VarEnv, _: &Datastore) -> Result<DataType, QueryError> {
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

    fn try_run(code: &str, env: &mut VarEnv, ds: &Datastore) -> Result<(), QueryError> {
        let program = crate::parser::parse(crate::lexer::Lexer::new(code)).unwrap();
        for stmt in program.stmts {
            interpret_expr(env, ds, stmt, None)?;
        }
        Ok(())
    }

    /// Repeated self-concatenation used to double a value per statement with
    /// no bound, so a sub-1 KB query could exhaust the server's memory
    /// (ActivityWatch/aw-server-rust#882 item 6). Every growth form must stop
    /// with ValueTooLarge long before the 40th doubling (2^40 copies).
    #[test]
    fn self_concatenation_is_bounded() {
        let ds = Datastore::new_in_memory(false);
        let ti =
            TimeInterval::new_from_string("2000-01-01T00:00:00Z/2000-01-02T00:00:00Z").unwrap();
        let mut event = aw_models::Event::default();
        event
            .data
            .insert("title".to_string(), serde_json::json!("some window"));
        let cases = [
            ("a = \"x\";", "a = a + a;"),
            ("a = [1];", "a = a + a;"),
            ("a = [1];", "a = [a, a];"),
            ("a = {\"k\": 1};", "a = {\"l\": a, \"r\": a};"),
            ("a = events;", "a = concat(a, a);"),
        ];
        for (init, step) in cases {
            let mut env = init_env(&ti);
            env.insert(
                "events".to_string(),
                DataType::List(vec![DataType::Event(event.clone())]),
            );
            let code = format!("{init} {}", step.repeat(40));
            match try_run(&code, &mut env, &ds) {
                Err(QueryError::ValueTooLarge(_)) => (),
                other => panic!("{step}: expected ValueTooLarge, got {other:?}"),
            }
            let a = env.get("a").unwrap();
            assert!(
                approx_size_within(a, MAX_VALUE_SIZE).is_some(),
                "{step}: stored value passed the bound"
            );
        }

        // Ordinary growth under the bound still works.
        let mut env = init_env(&ti);
        try_run(
            &format!("a = [1]; {}", "a = a + a;".repeat(10)),
            &mut env,
            &ds,
        )
        .unwrap();
        match env.get("a") {
            Some(DataType::List(l)) => assert_eq!(l.len(), 1024),
            other => panic!("expected a list, got {other:?}"),
        }
    }
}
