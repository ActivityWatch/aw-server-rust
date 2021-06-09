use aw_datastore::Datastore;

use crate::ast::*;
use crate::QueryError;
use crate::VarEnv;

pub fn preprocess_prog(p: &Program, env: &mut VarEnv, ds: &Datastore) -> Result<(), QueryError> {
    for expr in &p.stmts {
        preprocess_expr(env, ds, expr)?;
    }
    Ok(())
}

fn preprocess_expr(env: &mut VarEnv, ds: &Datastore, expr: &Expr) -> Result<(), QueryError> {
    use crate::ast::Expr_::*;
    match &expr.node {
        Var(ref var) => env.add_ref(var)?,
        Add(ref a, ref b) => {
            preprocess_expr(env, ds, a)?;
            preprocess_expr(env, ds, b)?;
        }
        Sub(ref a, ref b) => {
            preprocess_expr(env, ds, a)?;
            preprocess_expr(env, ds, b)?;
        }
        Mul(ref a, ref b) => {
            preprocess_expr(env, ds, a)?;
            preprocess_expr(env, ds, b)?;
        }
        Div(ref a, ref b) => {
            preprocess_expr(env, ds, a)?;
            preprocess_expr(env, ds, b)?;
        }
        Mod(ref a, ref b) => {
            preprocess_expr(env, ds, a)?;
            preprocess_expr(env, ds, b)?;
        }
        Equal(ref a, ref b) => {
            preprocess_expr(env, ds, a)?;
            preprocess_expr(env, ds, b)?;
        }
        Assign(ref var, ref b) => {
            preprocess_expr(env, ds, b)?;
            env.declare(var.to_string());
        }
        Function(ref fname, ref args) => {
            env.add_ref(fname)?;
            preprocess_expr(env, ds, args)?;
        }
        If(ref ifs) => {
            for (cond, block) in ifs {
                // TODO: could be optimized?
                preprocess_expr(env, ds, cond)?;
                for expr in block {
                    preprocess_expr(env, ds, expr)?;
                }
            }
        }
        List(list) => {
            for entry in list {
                preprocess_expr(env, ds, entry)?;
            }
        }
        Dict(d) => {
            for (_key, val_uninterpreted) in d {
                preprocess_expr(env, ds, val_uninterpreted)?;
            }
        }
        Return(e) => {
            preprocess_expr(env, ds, e)?;
        }
        Bool(_lit) => (),
        Number(_lit) => (),
        String(_lit) => (),
    };
    Ok(())
}
