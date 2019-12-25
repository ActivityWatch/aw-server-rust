use std::fmt;

use aw_models::TimeInterval;

use aw_datastore::Datastore;

pub mod datatype;

pub use crate::query::datatype::DataType;

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
    BucketQueryError(String),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

mod lexer {
    use plex::lexer;

    #[derive(Debug, Clone)]
    pub enum Token {
        Ident(String),

        If,
        ElseIf,
        Else,
        Return,

        Bool(bool),
        Number(f64),
        String(String),
        Plus,
        Minus,
        Star,
        Slash,
        Percent,
        Equals,
        Assign,
        LParen,
        RParen,
        LBracket,
        RBracket,
        LBrace,
        RBrace,
        Comma,
        Colon,
        Semi,

        Whitespace,
        Newline,
        Comment,
    }

    lexer! {
        fn next_token(text: 'a) -> (Token, &'a str);

        r#"[ \t\r]+"# => (Token::Whitespace, text),
        r#"\n"# => (Token::Newline, text),
        // Python-style comments (# ...)
        r#"#[^\n]*"# => (Token::Comment, text),

        r#"if"# => (Token::If, text),
        r#"elif"# => (Token::ElseIf, text),
        r#"else"# => (Token::Else, text),
        r#"return"# => (Token::Return, text),

        r#"true"# => (Token::Bool(true), text),
        r#"false"# => (Token::Bool(false), text),
        // TODO: Deprecate/Remove?
        r#"True"# => (Token::Bool(true), text),
        r#"False"# => (Token::Bool(false), text),

        r#"\"([^\"]|(\\\"))*\""# => (
            Token::String(text.to_owned()[1..text.len()-1].replace("\\\"", "\"").to_string()),
            text
        ),
        r#"[0-9]+[\.]?[0-9]*"# => {
            let tok = match text.parse() {
                Ok(n) => Token::Number(n),
                Err(e) => panic!("Integer {} is out of range: {}", text, e),
            };
            (tok, text)
        }

        r#"[a-zA-Z_][a-zA-Z0-9_]*"# => (Token::Ident(text.to_owned()), text),

        r#"=="# => (Token::Equals, text),
        r#"="# => (Token::Assign, text),
        r#"\+"# => (Token::Plus, text),
        r#"-"# => (Token::Minus, text),
        r#"\*"# => (Token::Star, text),
        r#"/"# => (Token::Slash, text),
        r#"%"# => (Token::Percent, text),
        r#"\("# => (Token::LParen, text),
        r#"\)"# => (Token::RParen, text),
        r#"\["# => (Token::LBracket, text),
        r#"\]"# => (Token::RBracket, text),
        r#"\{"# => (Token::LBrace, text),
        r#"\}"# => (Token::RBrace, text),
        r#","# => (Token::Comma, text),
        r#":"# => (Token::Colon, text),
        r#";"# => (Token::Semi, text),
    }

    pub struct Lexer<'a> {
        original: &'a str,
        remaining: &'a str,
        line: usize,
    }

    impl<'a> Lexer<'a> {
        pub fn new(s: &'a str) -> Lexer<'a> {
            Lexer { original: s, remaining: s , line: 1}
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Span {
        pub lo: usize,
        pub hi: usize,
        pub line: usize,
    }

    fn span_in(s: &str, t: &str, l: usize) -> Span {
        let lo = s.as_ptr() as usize - t.as_ptr() as usize;
        Span {
            lo: lo,
            hi: lo + s.len(),
            line: l,
        }
    }

    impl<'a> Iterator for Lexer<'a> {
        type Item = (Token, Span);
        fn next(&mut self) -> Option<(Token, Span)> {
            loop {
                let tok = if let Some((tok, new_remaining)) = next_token(self.remaining) {
                    self.remaining = new_remaining;
                    tok
                } else {
                    return None
                };
                match tok {
                    (Token::Whitespace, _) | (Token::Comment, _) => {
                        continue;
                    }
                    (Token::Newline, _) => {
                        self.line += 1;
                        continue;
                    }
                    (tok, span) => {
                        return Some((tok, span_in(span, self.original, self.line)));
                    }
                }
            }
        }
    }
}

mod ast {
    use crate::query::lexer::Span;

    use std::collections::HashMap;

    #[derive(Debug)]
    pub struct Program {
        pub stmts: Vec<Expr>
    }

    #[derive(Debug,Clone)]
    pub struct Expr {
        pub span: Span,
        pub node: Expr_,
    }

    #[derive(Debug,Clone)]
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
}

mod parser {
    use crate::query::ast::*;
    use crate::query::lexer::Token::*;
    use crate::query::lexer::*;
    use plex::parser;

    use std::collections::HashMap;

    fn merge_if_vecs(lhs: Expr_, rhs: Expr_) -> Expr_ {
        let mut ifs = match lhs {
            Expr_::If(_ifs) => _ifs.clone(),
            _ => unreachable!(),
        };
        match rhs {
            Expr_::If(_ifs) => ifs.append(&mut _ifs.clone()),
            _ => unreachable!(),
        };
        Expr_::If(ifs)
    }

    parser! {
        fn parse_(Token, Span);

        // combine two spans
        (a, b) {
            Span {
                lo: a.lo,
                hi: b.hi,
                line: a.line,
            }
        }

        program: Program {
            statements[s] => Program { stmts: s }
        }

        statements: Vec<Expr> {
            => vec![],
            statements[mut st] statement[x]  => {
                st.push(x);
                st
            },
        }

        statement: Expr {
            ifs[x] => x,
            ret[x] Semi => x,
        }

        ifs: Expr {
            _if[l_ifs] => l_ifs,
            _elif[l_ifs] => l_ifs,
            _else[l_ifs] => l_ifs,
        }

        _cond_block: Expr {
            binop[cond] LBrace statements[block] RBrace => Expr {
                span: span!(),
                node: {
                    let mut ifs = Vec::new();
                    ifs.push((Box::new(cond), block));
                    Expr_::If(ifs)
                }
            },
        }

        _if: Expr {
            If _cond_block[x] => x
        }

        _elif: Expr {
            // Else if
            _if[l_ifs] ElseIf _cond_block[l_preceding_ifs] => Expr {
                span: span!(),
                node: merge_if_vecs(l_ifs.node, l_preceding_ifs.node),
            },
            // Else if else if
            _elif[l_ifs] ElseIf _cond_block[l_preceding_ifs] => Expr {
                span: span!(),
                node: merge_if_vecs(l_ifs.node, l_preceding_ifs.node),
            },
        }

        _else: Expr {
            // if else
            _if[l_ifs] Else LBrace statements[l_else_block] RBrace => Expr {
                span: span!(),
                node: {
                    let mut l_new = match l_ifs.node {
                        Expr_::If(l_ifs) => l_ifs.clone(),
                        _ => unreachable!(),
                    };
                    let true_expr = Expr { span: span!(), node: Expr_::Bool(true) };
                    l_new.push((Box::new(true_expr), l_else_block));
                    Expr_::If(l_new)
                }
            },
            // else if else
            _elif[l_ifs] Else LBrace statements[l_else_block] RBrace => Expr {
                span: span!(),
                node: {
                    let mut l_new = match l_ifs.node {
                        Expr_::If(l_ifs) => l_ifs.clone(),
                        _ => unreachable!(),
                    };
                    let true_expr = Expr { span: span!(), node: Expr_::Bool(true) };
                    l_new.push((Box::new(true_expr), l_else_block));
                    Expr_::If(l_new)
                }
            },
        }

         ret: Expr {
            Return assign[a] => Expr {
                span: span!(),
                node: Expr_::Return(Box::new(a)),
            },
            assign[x] => x,
        }

        assign: Expr {
            Ident(var) Assign binop[rhs] => Expr {
                span: span!(),
                node: Expr_::Assign(var, Box::new(rhs)),
            },
            binop[x] => x
        }

        binop: Expr {
            binop[lhs] Plus func[rhs] => Expr {
                span: span!(),
                node: Expr_::Add(Box::new(lhs), Box::new(rhs)),
            },
            binop[lhs] Minus func[rhs] => Expr {
                span: span!(),
                node: Expr_::Sub(Box::new(lhs), Box::new(rhs)),
            },
            binop[lhs] Star func[rhs] => Expr {
                span: span!(),
                node: Expr_::Mul(Box::new(lhs), Box::new(rhs)),
            },
            binop[lhs] Slash func[rhs] => Expr {
                span: span!(),
                node: Expr_::Div(Box::new(lhs), Box::new(rhs)),
            },
            binop[lhs] Percent func[rhs] => Expr {
                span: span!(),
                node: Expr_::Mod(Box::new(lhs), Box::new(rhs)),
            },
            binop[lhs] Equals func[rhs] => Expr {
                span: span!(),
                node: Expr_::Equal(Box::new(lhs), Box::new(rhs)),
            },
            func[x] => x
        }

        func: Expr {
            Ident(fname) LParen _inner_list[l] RParen => Expr {
                span: span!(),
                node: Expr_::Function(fname, Box::new(l)),
            },
            Ident(fname) LParen RParen => Expr {
                span: span!(),
                node: {
                    let empty_expr_list = Expr {
                        span: span!(),
                        node: Expr_::List(Vec::new())
                    };
                    Expr_::Function(fname, Box::new(empty_expr_list))
                },
            },
            object[o] => o,
        }

        object: Expr {
            LBrace dict[d] RBrace => d,
            LBrace RBrace => Expr {
                span: span!(),
                node: {
                    Expr_::Dict(HashMap::new())
                }
            },
            list[l] => l
        }

        list: Expr {
            LBracket _inner_list[l] RBracket => l,
            LBracket RBracket => Expr {
                span: span!(),
                node: {
                    Expr_::List(Vec::new())
                }
            },
            atom[a] => a
        }

        _inner_list: Expr {
            binop[o] => Expr {
                span: span!(),
                node: {
                    let mut list = Vec::new();
                    list.push(o);
                    Expr_::List(list)
                }
            },
            _inner_list[l] Comma binop[o] => Expr {
                span: span!(),
                node: {
                    match l.node {
                        Expr_::List(mut l) => {
                            l.push(o);
                            // FIXME: this can be incredibly slow
                            Expr_::List(l.clone())
                        },
                        _ => unreachable!(),
                    }
                }
            },
        }

        dict: Expr {
            String(k) Colon binop[v] => Expr {
                span: span!(),
                node: {
                    let mut dict = HashMap::new();
                    dict.insert(k, v);
                    Expr_::Dict(dict)
                }
            },
            dict[d] Comma String(k) Colon binop[v] => Expr {
                span: span!(),
                node: {
                    match d.node {
                        Expr_::Dict(mut d) => {
                            d.insert(k, v);
                            // FIXME: this can be incredibly slow
                            Expr_::Dict(d.clone())
                        },
                        _ => unreachable!(),
                    }
                }
            },
        }

        atom: Expr {
            // round brackets to destructure tokens
            Ident(v) => Expr {
                span: span!(),
                node: Expr_::Var(v),
            },
            Bool(b) => Expr {
                span: span!(),
                node: Expr_::Bool(b),
            },
            Number(i) => Expr {
                span: span!(),
                node: Expr_::Number(i),
            },
            String(s) => Expr {
                span: span!(),
                node: Expr_::String(s),
            },
            LParen binop[x] RParen => x
        }
    }

    pub fn parse<I: Iterator<Item=(Token, Span)>>(i: I) -> Result<Program, (Option<(Token, Span)>, &'static str)> {
        parse_(i)
    }
}

mod functions;

mod interpret {
    use std::collections::HashMap;

    use aw_models::TimeInterval;
    use aw_datastore::Datastore;

    use crate::query;
    use crate::query::ast::*;
    use crate::query::DataType;
    use crate::query::QueryError;

    fn init_env<'a>(ti: &TimeInterval) -> HashMap<&'a str, DataType> {
        let mut env = HashMap::new();
        env.insert("TIMEINTERVAL", DataType::String(ti.to_string()));
        query::functions::fill_env(&mut env);
        return env;
    }

    pub fn interpret_prog<'a>(p: &'a Program, ti: &TimeInterval, ds: &Datastore) -> Result<DataType, QueryError> {
        let mut env = init_env(ti);
        let mut ret = None;
        for expr in &p.stmts {
            ret = Some(interpret_expr(&mut env, ds, expr)?)
        }
        match ret {
            Some(ret) => Ok(ret),
            None => Err(QueryError::EmptyQuery())
        }
    }

    fn interpret_expr<'a>(env: &mut HashMap<&'a str, DataType>, ds: &Datastore, expr: &'a Expr) -> Result<DataType, QueryError> {
        use crate::query::ast::Expr_::*;
        match expr.node {
            Add(ref a, ref b) => {
                let a_res = interpret_expr(env, ds, a)?;
                let b_res = interpret_expr(env, ds, b)?;
                let res = match a_res {
                    DataType::Number(n1) => match b_res {
                        DataType::Number(n2) => DataType::Number(n1+n2),
                        _ => return Err(QueryError::InvalidType("Cannot use + on something that is not a number with a number!".to_string()))
                    },
                    DataType::List(l1) => match b_res {
                        DataType::List(l2) => {
                            let mut new_list = l1.clone();
                            new_list.append(&mut l2.clone());
                            DataType::List(new_list)
                        },
                        _ => return Err(QueryError::InvalidType("Cannot use + on something that is not a list with a list!".to_string()))
                    }
                    DataType::String(s1) => match b_res {
                        DataType::String(s2) => {
                            let mut new_string = s1.clone();
                            new_string.push_str(&s2);
                            DataType::String(new_string)
                        },
                        _ => return Err(QueryError::InvalidType("Cannot use + on something that is not a list with a list!".to_string()))
                    }
                    _ => return Err(QueryError::InvalidType("Cannot use + on something that is not a number, list or string!".to_string()))
                };
                Ok(res)
            },
            Sub(ref a, ref b) => {
                let a_res = interpret_expr(env, ds, a)?;
                let b_res = interpret_expr(env, ds, b)?;
                let a_num = match a_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot sub something that is not a number!".to_string()))
                };
                let b_num = match b_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot sub something that is not a number!".to_string()))
                };
                Ok(DataType::Number(a_num-b_num))
            },
            Mul(ref a, ref b) => {
                let a_res = interpret_expr(env, ds, a)?;
                let b_res = interpret_expr(env, ds, b)?;
                let a_num = match a_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot sub something that is not a number!".to_string()))
                };
                let b_num = match b_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot sub something that is not a number!".to_string()))
                };
                Ok(DataType::Number(a_num*b_num))
            },
            Div(ref a, ref b) => {
                let a_res = interpret_expr(env, ds, a)?;
                let b_res = interpret_expr(env, ds, b)?;
                let a_num = match a_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot sub something that is not a number!".to_string()))
                };
                let b_num = match b_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot sub something that is not a number!".to_string()))
                };
                if b_num == 0.0 {
                    return Err(QueryError::MathError("Tried to divide by zero!".to_string()));
                }
                Ok(DataType::Number(a_num/b_num))
            },
            Mod(ref a, ref b) => {
                let a_res = interpret_expr(env, ds, a)?;
                let b_res = interpret_expr(env, ds, b)?;
                let a_num = match a_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot sub something that is not a number!".to_string()))
                };
                let b_num = match b_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot sub something that is not a number!".to_string()))
                };
                Ok(DataType::Number(a_num%b_num))
            },
            Equal(ref lhs, ref rhs) => {
                let lhs_res = interpret_expr(env, ds, lhs)?;
                let rhs_res = interpret_expr(env, ds, rhs)?;
                Ok(DataType::Bool(lhs_res.query_eq(&rhs_res)?))
            },
            Assign(ref var, ref b) => {
                let val = interpret_expr(env, ds, b)?;
                // FIXME: avoid clone, it's slow
                env.insert(var, val.clone());
                Ok(val)
            }
            // FIXME: avoid clone, it's slow
            Var(ref var) => {
                match env.get(&var[..]) {
                    Some(v) => Ok(v.clone()),
                    None => Err(QueryError::VariableNotDefined(var.to_string()))
                }
            },
            Bool(lit) => Ok(DataType::Bool(lit)),
            Number(lit) => Ok(DataType::Number(lit)),
            String(ref litstr) => Ok(DataType::String(litstr.to_string())),
            Return(ref e) => {
                let val = interpret_expr(env, ds, e)?;
                Ok(val)
            },
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
            },
            Function(ref fname, ref e) => {
                let args = match interpret_expr(env, ds, e)? {
                    DataType::List(l) => l,
                    _ => unreachable!(),
                };
                let var = match env.get(&fname[..]) {
                    Some(v) => v,
                    None => return Err(QueryError::VariableNotDefined(fname.clone()))
                };
                let (_name, fun) = match var {
                    DataType::Function(name, fun) => (name, fun),
                    _data => return Err(QueryError::InvalidType(fname.to_string()))
                };
                fun(args, env, ds)
            },
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
}

pub fn query<'a>(code: &str, ti: &TimeInterval, ds: &Datastore) -> Result<DataType, QueryError> {
    let lexer = lexer::Lexer::new(code);
    let program = match parser::parse(lexer) {
        Ok(p) => p,
        Err(e) => {
            // TODO: Improve parsing error message
            warn!("ParsingError: {:?}", e);
            return Err(QueryError::ParsingError(format!("{:?}", e)));
        }
    };
    interpret::interpret_prog(&program, ti, ds)
}
