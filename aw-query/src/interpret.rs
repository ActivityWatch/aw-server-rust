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
) -> Result<DataType, QueryError> {
    let mut env = init_env(ti);
    for expr in p.stmts {
        interpret_expr(&mut env, ds, expr)?;
    }
    match env.remove("RETURN") {
        Some(ret) => Ok(ret),
        None => Err(QueryError::EmptyQuery()),
    }
}

fn interpret_expr(
    env: &mut HashMap<String, DataType>,
    ds: &Datastore,
    expr: Expr,
) -> Result<DataType, QueryError> {
    use crate::ast::Expr_::*;
    match expr.node {
        Add(a, b) => {
            let a_res = interpret_expr(env, ds, *a)?;
            let b_res = interpret_expr(env, ds, *b)?;
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
            let a_res = interpret_expr(env, ds, *a)?;
            let b_res = interpret_expr(env, ds, *b)?;
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
            let a_res = interpret_expr(env, ds, *a)?;
            let b_res = interpret_expr(env, ds, *b)?;
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
            let a_res = interpret_expr(env, ds, *a)?;
            let b_res = interpret_expr(env, ds, *b)?;
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
            let a_res = interpret_expr(env, ds, *a)?;
            let b_res = interpret_expr(env, ds, *b)?;
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
            let lhs_res = interpret_expr(env, ds, *lhs)?;
            let rhs_res = interpret_expr(env, ds, *rhs)?;
            Ok(DataType::Bool(lhs_res.query_eq(&rhs_res)?))
        }
        Assign(var, b) => {
            let val = interpret_expr(env, ds, *b)?;
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
            let val = interpret_expr(env, ds, *e)?;
            // TODO: Once RETURN is deprecated we can fix this
            env.insert("RETURN".to_string(), val);
            Ok(DataType::None())
        }
        If(ifs) => {
            for (cond, block) in ifs {
                let c = interpret_expr(env, ds, *cond)?;
                if c.query_eq(&DataType::Bool(true))? {
                    for expr in block {
                        interpret_expr(env, ds, expr)?;
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
            let args = match interpret_expr(env, ds, *e)? {
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
            for entry in list {
                let res = interpret_expr(env, ds, entry)?;
                l.push(res);
            }
            Ok(DataType::List(l))
        }
        Dict(d) => {
            let mut dict = HashMap::new();
            for (key, val_uninterpreted) in d {
                let val = interpret_expr(env, ds, val_uninterpreted)?;
                dict.insert(key, val);
            }
            Ok(DataType::Dict(dict))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(code: &str, env: &mut VarEnv, ds: &Datastore) -> DataType {
        let program = crate::parser::parse(crate::lexer::Lexer::new(code)).unwrap();
        for expr in program.stmts {
            interpret_expr(env, ds, expr).unwrap();
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
                interpret_expr(&mut env, &ds, call).unwrap(),
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
}
