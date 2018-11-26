#[derive(Debug)]
pub enum QueryError {
    // Lexing + Parsing
    LexingError, // FIXME: Lexing currently cannot fail without panic, unused
    ParsingError,

    // Execution
    EmptyQuery(),
    VariableNotDefined(String),
    MathError(String),
    InvalidType(String),
    InvalidFunctionParameters(String),
    TimeIntervalError(String),
    BucketQueryError(String),
}

mod lexer {
    use plex::lexer;

    #[derive(Debug, Clone)]
    pub enum Token {
        Ident(String),

        Return,

        Number(f64),
        String(String),
        Equals,
        Plus,
        Minus,
        Star,
        Slash,
        Percent,
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
        Comment,
    }

    lexer! {
        fn next_token(text: 'a) -> (Token, &'a str);

        r#"[ \t\r\n]+"# => (Token::Whitespace, text),
        // Python-style comments (# ...)
        r#"#[^\n]*"# => (Token::Comment, text),

        r#"return"# => (Token::Return, text),

        r#"\"[^\"]*\""# => (
            Token::String(text.to_owned()[1..text.len()-1].to_string()),
            text
        ),
        r#"[0-9]+[\.]?[0-9]*"# => {
            (if let Ok(i) = text.parse() {
                Token::Number(i)
            } else {
                // TODO: do not panic, send an error
                panic!("integer {} is out of range", text)
            }, text)
        }

        r#"[a-zA-Z_][a-zA-Z0-9_]*"# => (Token::Ident(text.to_owned()), text),

        r#"="# => (Token::Equals, text),
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

        // TODO: do not panic, send an error
        r#"."# => panic!("unexpected character: {}", text),
    }

    pub struct Lexer<'a> {
        original: &'a str,
        remaining: &'a str,
    }

    impl<'a> Lexer<'a> {
        pub fn new(s: &'a str) -> Lexer<'a> {
            Lexer { original: s, remaining: s }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Span {
        pub lo: usize,
        pub hi: usize,
    }

    fn span_in(s: &str, t: &str) -> Span {
        let lo = s.as_ptr() as usize - t.as_ptr() as usize;
        Span {
            lo: lo,
            hi: lo + s.len(),
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
                    (tok, span) => {
                        return Some((tok, span_in(span, self.original)));
                    }
                }
            }
        }
    }
}

mod ast {
    use query::lexer::Span;

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
        Var(String),
        Assign(String, Box<Expr>),
        // TODO: multi-argument functions
        Function(String, Box<Expr>),
        Return(Box<Expr>),
        Number(f64),
        String(String),
        List(Vec<Expr>),
        Dict(HashMap<String, Expr>),
    }
}

mod parser {
    use query::ast::*;
    use query::lexer::Token::*;
    use query::lexer::*;
    use plex::parser;

    use std::collections::HashMap;

    parser! {
        fn parse_(Token, Span);

        // combine two spans
        (a, b) {
            Span {
                lo: a.lo,
                hi: b.hi,
            }
        }

        program: Program {
            statements[s] => Program { stmts: s }
        }

        statements: Vec<Expr> {
            => vec![],
            statements[mut st] ret[r] Semi => {
                st.push(r);
                st
            }
        }

        ret: Expr {
            Return assign[a] => Expr {
                span: span!(),
                node: Expr_::Return(Box::new(a)),
            },
            assign[a] => a
        }

        assign: Expr {
            // Assign
            Ident(var) Equals assign[rhs] => Expr {
                span: span!(),
                node: Expr_::Assign(var, Box::new(rhs)),
            },
            // Function
            Ident(fname) LParen list[l] RParen => Expr {
                span: span!(),
                node: {
                    Expr_::Function(fname, Box::new(l))
                }
            },
            object[o] => o
        }

        object: Expr {
            LBracket list[l] RBracket => l,
            LBracket RBracket => Expr {
                span: span!(),
                node: {
                    Expr_::List(Vec::new())
                }
            },
            LBrace dict[d] RBrace => d,
            LBrace RBrace => Expr {
                span: span!(),
                node: {
                    Expr_::Dict(HashMap::new())
                }
            },
            term[o] => o,
        }

        list: Expr {
            assign[o] => Expr {
                span: span!(),
                node: {
                    let mut list = Vec::new();
                    list.push(o);
                    Expr_::List(list)
                }
            },
            list[l] Comma assign[o] => Expr {
                span: span!(),
                node: {
                    match l.node {
                        Expr_::List(mut l) => {
                            l.push(o);
                            // FIXME: this can be incredibly slow
                            Expr_::List(l.clone())
                        },
                        _ => panic!("a")
                    }
                }
            },
        }

        dict: Expr {
            String(k) Colon assign[v] => Expr {
                span: span!(),
                node: {
                    let mut dict = HashMap::new();
                    dict.insert(k, v);
                    Expr_::Dict(dict)
                }
            },
            dict[d] Comma String(k) Colon assign[v] => Expr {
                span: span!(),
                node: {
                    match d.node {
                        Expr_::Dict(mut d) => {
                            d.insert(k, v);
                            // FIXME: this can be incredibly slow
                            Expr_::Dict(d.clone())
                        },
                        _ => panic!("a")
                    }
                }
            },
        }

        term: Expr {
            term[lhs] Plus fact[rhs] => Expr {
                span: span!(),
                node: Expr_::Add(Box::new(lhs), Box::new(rhs)),
            },
            term[lhs] Minus fact[rhs] => Expr {
                span: span!(),
                node: Expr_::Sub(Box::new(lhs), Box::new(rhs)),
            },
            fact[x] => x
        }

        fact: Expr {
            fact[lhs] Star atom[rhs] => Expr {
                span: span!(),
                node: Expr_::Mul(Box::new(lhs), Box::new(rhs)),
            },
            fact[lhs] Slash atom[rhs] => Expr {
                span: span!(),
                node: Expr_::Div(Box::new(lhs), Box::new(rhs)),
            },
            fact[lhs] Percent atom[rhs] => Expr {
                span: span!(),
                node: Expr_::Mod(Box::new(lhs), Box::new(rhs)),
            },
            atom[x] => x
        }

        atom: Expr {
            // round brackets to destructure tokens
            Ident(v) => Expr {
                span: span!(),
                node: Expr_::Var(v),
            },
            Number(i) => Expr {
                span: span!(),
                node: Expr_::Number(i),
            },
            String(s) => Expr {
                span: span!(),
                node: Expr_::String(s),
            },
            LParen assign[a] RParen => a
        }
    }

    pub fn parse<I: Iterator<Item=(Token, Span)>>(i: I) -> Result<Program, (Option<(Token, Span)>, &'static str)> {
        parse_(i)
    }
}

use models::Event;
use serde::Serializer;
use serde::Serialize;

use std::collections::HashMap;

#[derive(Clone,Serialize)]
#[serde(untagged)]
pub enum DataType {
    None(),
    Number(f64),
    String(String),
    Event(Event),
    List(Vec<DataType>),
    Dict(HashMap<String, DataType>),
    // Name, argc (-1=unlimited), func
    #[serde(serialize_with = "serialize_function")]
    Function(String, i8, functions::QueryFn),
}

fn serialize_function<S>(element: &String, i: &i8, fun: &functions::QueryFn, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer
{
    panic!("Query function was unevaluated and was attempted to be serialized, panic!");
    //element.id.serialize(serializer)
}

use std::fmt;

// Needed because of a limitation in rust where you cannot derive(Debug) on a
// enum which has a fn with reference parameters which our QueryFn has
// https://stackoverflow.com/questions/53380040/function-pointer-with-a-reference-argument-cannot-derive-debug
impl fmt::Debug for DataType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DataType::None() => write!(f, "None()"),
            DataType::Number(n) => write!(f, "Number({})", n),
            DataType::String(s) => write!(f, "String({})", s),
            DataType::Event(e) => write!(f, "Event({:?})", e),
            DataType::List(l) => write!(f, "List({:?})", l),
            DataType::Dict(d) => write!(f, "Dict({:?})", d),
            DataType::Function(name, _argc, _fun) => write!(f, "Function({})", name),
        }
    }
}

impl PartialEq for DataType {
    fn eq(&self, other: &DataType) -> bool {
        match (self, other) {
            (DataType::None(), DataType::None()) => true,
            (DataType::Number(n1), DataType::Number(n2)) => n1 == n2,
            (DataType::String(s1), DataType::String(s2)) => s1 == s2,
            // TODO: Implement event comparison
            (DataType::Event(e1), DataType::String(e2)) => true, //e1 == e2,
            (DataType::List(l1), DataType::List(l2)) => l1 == l2,
            (DataType::Dict(d1), DataType::Dict(d2)) => d1 == d2,
            // We do not care about comparing functions
            _ => false
        }
    }
}

mod functions {
    use query::DataType;
    use query::QueryError;
    use datastore::Datastore;
    use models::TimeInterval;
    use models::Event;
    use transform;

    use std::collections::HashMap;

    pub type QueryFn = fn(args: Vec<DataType>, env: &HashMap<&str, DataType>, ds: &Datastore) -> Result<DataType, QueryError>;

    pub fn fill_env<'a>(env: &mut HashMap<&'a str, DataType>) {
        env.insert("print", DataType::Function("print".to_string(), -1, q_print));
        env.insert("query_bucket", DataType::Function("query_bucket".to_string(), 1, q_query_bucket));
        env.insert("flood", DataType::Function("flood".to_string(), 1, q_flood));
        env.insert("merge_events_by_keys", DataType::Function("merge_events_by_keys".to_string(), 2, q_merge_events_by_keys));
    }

    fn get_timeinterval (env: &HashMap<&str, DataType>) -> Result<TimeInterval, QueryError> {
        let interval_str = match env.get("TIMEINTERVAL") {
            Some(data_ti) => match data_ti {
                DataType::String(ti_str) => ti_str,
                _ => return Err(QueryError::TimeIntervalError("TIMEINTERVAL is not of type string!".to_string()))
            },
            None => return Err(QueryError::TimeIntervalError("TIMEINTERVAL not defined!".to_string()))
        };
        match TimeInterval::new_from_string(interval_str) {
            Ok(ti) => Ok(ti),
            Err(_e) => Err(QueryError::TimeIntervalError(format!("Failed to parse TIMEINTERVAL: {}", interval_str)))
        }
    }

    fn q_print(args: Vec<DataType>, env: &HashMap<&str, DataType>, ds: &Datastore) -> Result<DataType, QueryError> {
        for arg in args {
            println!("{:?}", arg);
        }
        return Ok(DataType::None());
    }

    fn q_query_bucket(args: Vec<DataType>, env: &HashMap<&str, DataType>, ds: &Datastore) -> Result<DataType, QueryError> {
        let bucket_id = match args[0] {
            DataType::String(ref s) => s,
            ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type String, got {:?}", invalid_type)))
        };
        let interval = get_timeinterval (env)?;
        let events = match ds.get_events(bucket_id, Some(interval.start().clone()), Some(interval.end().clone()), None) {
            Ok(events) => events,
            Err(e) => return Err(QueryError::BucketQueryError(format!("Failed to query bucket: {:?}", e)))
        };
        let mut ret = Vec::new();
        for event in events {
            ret.push(DataType::Event(event));
        };
        return Ok(DataType::List(ret));
    }

    fn q_flood(args: Vec<DataType>, env: &HashMap<&str, DataType>, ds: &Datastore) -> Result<DataType, QueryError> {
        let mut events = match args[0] {
            // TODO: sort by timestamp first
            DataType::List(ref l) => l.clone(),
            ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List, got {:?}", invalid_type)))
        };
        println!("{:?}", events);
        // Move events out of DataType container
        let mut new_events = Vec::new();
        for event in events.drain(..) {
            match event {
                DataType::Event(e) => new_events.push(e.clone()),
                ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List of Events, list contains {:?}", invalid_type)))
            }
        }
        // Run flood
        let mut flooded_events = transform::flood(new_events, chrono::Duration::seconds(5));
        // Put events back into DataType::Event container
        let mut new_flooded_events = Vec::new();
        for event in flooded_events.drain(..) {
            new_flooded_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(new_flooded_events));
    }

    fn q_merge_events_by_keys(args: Vec<DataType>, env: &HashMap<&str, DataType>, ds: &Datastore) -> Result<DataType, QueryError> {
        let mut tagged_events = match args[0] {
            // TODO: sort by timestamp first
            DataType::List(ref events) => events.clone(),
            ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List, got {:?}", invalid_type)))
        };
        let keys = match args[1] {
            DataType::List(ref keys) => keys,
            ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List, got {:?}", invalid_type)))
        };
        if keys.len() == 0 {
            return Ok(DataType::List(tagged_events));
        }
        let mut new_events = Vec::new();
        for event in tagged_events.drain(..) {
            match event {
                DataType::Event(e) => new_events.push(e),
                ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List of Events, list contains {:?}", invalid_type)))
            }
        }
        let mut new_keys = Vec::new();
        for key in keys {
            match key {
                DataType::String(s) => new_keys.push(s.clone()),
                ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List of Events, list contains {:?}", invalid_type)))
            }
        }
        let mut merged_events = transform::merge_events_by_keys(new_events, new_keys);
        let mut merged_tagged_events = Vec::new();
        for event in merged_events.drain(..) {
            merged_tagged_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(merged_tagged_events));
    }
}

mod interpret {
    use query;
    use query::ast::*;
    use query::DataType;
    use query::QueryError;
    use std::collections::HashMap;
    use datastore::Datastore;
    use models::TimeInterval;

    fn init_env<'a>(ti: &TimeInterval) -> HashMap<&'a str, DataType> {
        let mut env = HashMap::new();
        env.insert("TIMEINTERVAL", DataType::String(ti.to_string()));
        query::functions::fill_env(&mut env);
        return env;
    }

    pub fn interpret_prog<'a>(p: &'a Program, ti: &TimeInterval, ds: &Datastore) -> Result<DataType, QueryError> {
        if (p.stmts.len() == 0) {
            return Err(QueryError::EmptyQuery());
        }
        let last_i = p.stmts.len()-1;
        let mut env = init_env(ti);
        let mut i = 0;
        for expr in &p.stmts {
            let ret = interpret_expr(&mut env, ds, expr)?;
            // FIXME: This is ugly
            if i == last_i {
                return Ok(ret);
            }
            i+=1;
        }
        panic!("This should be unreachable!");
    }

    fn interpret_expr<'a>(env: &mut HashMap<&'a str, DataType>, ds: &Datastore, expr: &'a Expr) -> Result<DataType, QueryError> {
        use query::ast::Expr_::*;
        match expr.node {
            Add(ref a, ref b) => {
                let a_res = interpret_expr(env, ds, a)?;
                let b_res = interpret_expr(env, ds, b)?;
                let a_num = match a_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot add something that is not a number!".to_string()))
                };
                let b_num = match b_res {
                    DataType::Number(n) => n,
                    _ => return Err(QueryError::InvalidType("Cannot add something that is not a number!".to_string()))
                };
                Ok(DataType::Number(a_num+b_num))
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
            Number(lit) => Ok(DataType::Number(lit)),
            String(ref litstr) => Ok(DataType::String(litstr.to_string())),
            Return(ref e) => {
                let val = interpret_expr(env, ds, e)?;
                Ok(val)
            },
            Function(ref fname, ref e) => {
                let args = match interpret_expr(env, ds, e)? {
                    DataType::List(l) => l,
                    _ => panic!("This should not be possible")
                };
                let var = match env.get(&fname[..]) {
                    Some(v) => v,
                    None => return Err(QueryError::VariableNotDefined(fname.clone()))
                };
                let (name, argc, fun) = match var {
                    DataType::Function(name, argc, fun) => (name, argc, fun),
                    data => return Err(QueryError::InvalidType(fname.to_string()))
                };
                if (args.len() as i8) != *argc && *argc >= 0 {
                    return Err(QueryError::InvalidFunctionParameters(format!("Expected 1 argument, got {}", args.len())));
                }
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

use datastore::Datastore;
use models::TimeInterval;

pub fn query<'a>(code: &str, ti: &TimeInterval, ds: &Datastore) -> Result<DataType, QueryError> {
    let lexer = lexer::Lexer::new(code)
        .inspect(|tok| eprintln!("tok: {:?}", tok));
    let program = match parser::parse(lexer) {
        Ok(p) => p,
        Err(e) => {
            println!("{:?}", e);
            return Err(QueryError::ParsingError);
        }
    };
    interpret::interpret_prog(&program, ti, ds)
}
