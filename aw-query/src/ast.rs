use crate::lexer::Span;

use std::collections::HashMap;

#[derive(Debug)]
pub struct Program {
    pub stmts: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub span: Span,
    pub node: Expr_,
}

#[derive(Debug, Clone)]
pub enum Expr_ {
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
    Mod(Box<Expr>, Box<Expr>),

    Equal(Box<Expr>, Box<Expr>),

    Var(String),
    Assign(String, Box<Expr>),
    Function(String, Box<Expr>),
    If(Vec<(Box<Expr>, Vec<Expr>)>),
    Return(Box<Expr>),

    Bool(bool),
    Number(f64),
    String(String),
    List(Vec<Expr>),
    Dict(HashMap<String, Expr>),
}
