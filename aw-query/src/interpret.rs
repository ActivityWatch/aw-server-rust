use std::collections::HashMap;

use aw_datastore::Datastore;

use crate::ast::*;
use crate::DataType;
use crate::QueryError;
use crate::VarEnv;

pub fn interpret_prog(
    p: Program,
    env: &mut VarEnv,
    ds: &Datastore,
) -> Result<DataType, QueryError> {
    for expr in p.stmts {
        interpret_expr(env, ds, expr)?;
    }
    match env.take("RETURN") {
        Some(ret) => Ok(ret),
        None => Err(QueryError::EmptyQuery()),
    }
}

fn interpret_expr(env: &mut VarEnv, ds: &Datastore, expr: Expr) -> Result<DataType, QueryError> {
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
        Var(var) => match env.take(&var) {
            Some(v) => Ok(v),
            None => Err(QueryError::VariableNotDefined(var.to_string())),
        },
        Bool(lit) => Ok(DataType::Bool(lit)),
        Number(lit) => Ok(DataType::Number(lit)),
        String(litstr) => Ok(DataType::String(litstr)),
        Return(e) => {
            let val = interpret_expr(env, ds, *e)?;
            // TODO: Once RETURN is deprecated we can fix this
            env.declare("RETURN".to_string());
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
            let args = match interpret_expr(env, ds, *e)? {
                DataType::List(l) => l,
                _ => unreachable!(),
            };
            let var = match env.take(&fname[..]) {
                Some(v) => v,
                None => return Err(QueryError::VariableNotDefined(fname.clone())),
            };
            let (_name, fun) = match var {
                DataType::Function(name, fun) => (name, fun),
                _data => return Err(QueryError::InvalidType(fname.to_string())),
            };
            fun(args, env, ds)
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
                dict.insert(key.clone(), val);
            }
            Ok(DataType::Dict(dict))
        }
    }
}
