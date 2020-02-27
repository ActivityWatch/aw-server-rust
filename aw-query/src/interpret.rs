use std::collections::HashMap;

use crate::functions;

use aw_datastore::Datastore;
use aw_models::TimeInterval;

use crate::ast::*;
use crate::DataType;
use crate::QueryError;

fn init_env<'a>(ti: &TimeInterval) -> HashMap<&'a str, DataType> {
    let mut env = HashMap::new();
    env.insert("TIMEINTERVAL", DataType::String(ti.to_string()));
    functions::fill_env(&mut env);
    return env;
}

pub fn interpret_prog<'a>(
    p: &'a Program,
    ti: &TimeInterval,
    ds: &Datastore,
) -> Result<DataType, QueryError> {
    let mut env = init_env(ti);
    let mut ret = None;
    for expr in &p.stmts {
        ret = Some(interpret_expr(&mut env, ds, expr)?)
    }
    match ret {
        Some(ret) => Ok(ret),
        None => Err(QueryError::EmptyQuery()),
    }
}

fn interpret_expr<'a>(
    env: &mut HashMap<&'a str, DataType>,
    ds: &Datastore,
    expr: &'a Expr,
) -> Result<DataType, QueryError> {
    use crate::ast::Expr_::*;
    match expr.node {
        Add(ref a, ref b) => {
            let a_res = interpret_expr(env, ds, a)?;
            let b_res = interpret_expr(env, ds, b)?;
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
                DataType::List(l1) => match b_res {
                    DataType::List(l2) => {
                        let mut new_list = l1.clone();
                        new_list.append(&mut l2.clone());
                        DataType::List(new_list)
                    }
                    _ => {
                        return Err(QueryError::InvalidType(
                            "Cannot use + on something that is not a list with a list!".to_string(),
                        ))
                    }
                },
                DataType::String(s1) => match b_res {
                    DataType::String(s2) => {
                        let mut new_string = s1.clone();
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
        Sub(ref a, ref b) => {
            let a_res = interpret_expr(env, ds, a)?;
            let b_res = interpret_expr(env, ds, b)?;
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
        Mul(ref a, ref b) => {
            let a_res = interpret_expr(env, ds, a)?;
            let b_res = interpret_expr(env, ds, b)?;
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
        Div(ref a, ref b) => {
            let a_res = interpret_expr(env, ds, a)?;
            let b_res = interpret_expr(env, ds, b)?;
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
        Mod(ref a, ref b) => {
            let a_res = interpret_expr(env, ds, a)?;
            let b_res = interpret_expr(env, ds, b)?;
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
        Equal(ref lhs, ref rhs) => {
            let lhs_res = interpret_expr(env, ds, lhs)?;
            let rhs_res = interpret_expr(env, ds, rhs)?;
            Ok(DataType::Bool(lhs_res.query_eq(&rhs_res)?))
        }
        Assign(ref var, ref b) => {
            let val = interpret_expr(env, ds, b)?;
            // FIXME: avoid clone, it's slow
            env.insert(var, val.clone());
            Ok(val)
        }
        // FIXME: avoid clone, it's slow
        Var(ref var) => match env.get(&var[..]) {
            Some(v) => Ok(v.clone()),
            None => Err(QueryError::VariableNotDefined(var.to_string())),
        },
        Bool(lit) => Ok(DataType::Bool(lit)),
        Number(lit) => Ok(DataType::Number(lit)),
        String(ref litstr) => Ok(DataType::String(litstr.to_string())),
        Return(ref e) => {
            let val = interpret_expr(env, ds, e)?;
            Ok(val)
        }
        If(ref ifs) => {
            for (ref cond, ref block) in ifs {
                let c = interpret_expr(env, ds, cond)?;
                if c.query_eq(&DataType::Bool(true))? {
                    for expr in block {
                        interpret_expr(env, ds, expr)?;
                    }
                    break;
                }
            }
            Ok(DataType::None())
        }
        Function(ref fname, ref e) => {
            let args = match interpret_expr(env, ds, e)? {
                DataType::List(l) => l,
                _ => unreachable!(),
            };
            let var = match env.get(&fname[..]) {
                Some(v) => v,
                None => return Err(QueryError::VariableNotDefined(fname.clone())),
            };
            let (_name, fun) = match var {
                DataType::Function(name, fun) => (name, fun),
                _data => return Err(QueryError::InvalidType(fname.to_string())),
            };
            fun(args, env, ds)
        }
        List(ref list) => {
            let mut l = Vec::new();
            for entry in list {
                let res = interpret_expr(env, ds, entry)?;
                l.push(res);
            }
            Ok(DataType::List(l))
        }
        Dict(ref d) => {
            let mut dict = HashMap::new();
            for (key, val_uninterpreted) in d {
                let val = interpret_expr(env, ds, val_uninterpreted)?;
                dict.insert(key.clone(), val);
            }
            Ok(DataType::Dict(dict))
        }
    }
}
