
pub mod lexer {
    use plex::lexer;

    #[derive(Debug, Clone)]
    pub enum Token {
        Ident(String),

        Return,
        Print,

        Number(f64),
        String(String),
        Equals,
        Plus,
        Minus,
        Star,
        Slash,
        LParen,
        RParen,
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
        r#"print"# => (Token::Print, text),

		r#"\"[^\"]*\""# => (
			Token::String(text.to_owned()[1..text.len()-1].to_string()),
			text
		),
        r#"[0-9]+[\.]?[0-9]*"# => {
            (if let Ok(i) = text.parse() {
                Token::Number(i)
            } else {
                panic!("integer {} is out of range", text)
            }, text)
        }

        r#"[a-zA-Z_][a-zA-Z0-9_]*"# => (Token::Ident(text.to_owned()), text),

        r#"="# => (Token::Equals, text),
        r#"\+"# => (Token::Plus, text),
        r#"-"# => (Token::Minus, text),
        r#"\*"# => (Token::Star, text),
        r#"/"# => (Token::Slash, text),
        r#"\("# => (Token::LParen, text),
        r#"\)"# => (Token::RParen, text),
        r#";"# => (Token::Semi, text),

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

    #[derive(Debug)]
    pub struct Program {
        pub stmts: Vec<Expr>
    }

    #[derive(Debug)]
    pub struct Expr {
        pub span: Span,
        pub node: Expr_,
    }

    #[derive(Debug)]
    pub enum Expr_ {
/*
        Add(Box<Expr>, Box<Expr>),
        Sub(Box<Expr>, Box<Expr>),
        Mul(Box<Expr>, Box<Expr>),
        Div(Box<Expr>, Box<Expr>),
*/
        Var(String),
        Assign(String, Box<Expr>),
        Print(Box<Expr>),
        Return(Box<Expr>),
        Literal(f64),
        LiteralString(String),
    }
}

pub mod parser {
    use query::ast::*;
    use query::lexer::Token::*;
    use query::lexer::*;
    use plex::parser;
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
            statements[mut st] assign[e] Semi => {
                st.push(e);
                st
            }
        }

        assign: Expr {
            Print LParen assign[a] RParen => Expr {
                span: span!(),
                node: Expr_::Print(Box::new(a)),
            },
            Return assign[a] => Expr {
                span: span!(),
                node: Expr_::Return(Box::new(a)),
            },
            Ident(var) Equals assign[rhs] => Expr {
                span: span!(),
                node: Expr_::Assign(var, Box::new(rhs)),
            },
            term[t] => t,
        }

        term: Expr {
/*
            term[lhs] Plus fact[rhs] => Expr {
                span: span!(),
                node: Expr_::Add(Box::new(lhs), Box::new(rhs)),
            },
            term[lhs] Minus fact[rhs] => Expr {
                span: span!(),
                node: Expr_::Sub(Box::new(lhs), Box::new(rhs)),
            },
*/
            fact[x] => x
        }

        fact: Expr {
/*
            fact[lhs] Star atom[rhs] => Expr {
                span: span!(),
                node: Expr_::Mul(Box::new(lhs), Box::new(rhs)),
            },
            fact[lhs] Slash atom[rhs] => Expr {
                span: span!(),
                node: Expr_::Div(Box::new(lhs), Box::new(rhs)),
            },
*/
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
                node: Expr_::Literal(i),
            },
            String(s) => Expr {
                span: span!(),
                node: Expr_::LiteralString(s),
            },
            LParen assign[a] RParen => a
        }
    }

    pub fn parse<I: Iterator<Item=(Token, Span)>>(i: I) -> Result<Program, (Option<(Token, Span)>, &'static str)> {
        parse_(i)
    }
}

#[derive(Debug,Clone)]
pub enum DataType {
	Number(f64),
	String(String),
}

#[derive(Debug)]
pub enum QueryError {
	NoReturn
}

pub mod interp {
    use query::ast::*;
	use query::DataType;
	use query::QueryError;
    use std::collections::HashMap;

    pub fn interp<'a>(p: &'a Program) -> Result<DataType, QueryError> {
        let mut env = HashMap::new();
		let last_i = p.stmts.len()-1;
		let mut i = 0;
        for expr in &p.stmts {
            let ret = interp_expr(&mut env, expr);
			// FIXME: This is ugly
			if i == last_i {
				return Ok(ret);
			}
			i+=1;
        }
		Err(QueryError::NoReturn)
    }
    fn interp_expr<'a>(env: &mut HashMap<&'a str, DataType>, expr: &'a Expr) -> DataType {
        use query::ast::Expr_::*;
        match expr.node {
/*
            Add(ref a, ref b) => interp_expr(env, a) + interp_expr(env, b),
            Sub(ref a, ref b) => interp_expr(env, a) - interp_expr(env, b),
            Mul(ref a, ref b) => interp_expr(env, a) * interp_expr(env, b),
            Div(ref a, ref b) => interp_expr(env, a) / interp_expr(env, b),
*/
            Assign(ref var, ref b) => {
                let val = interp_expr(env, b);
				// FIXME: avoid clone, it's slow
                env.insert(var, val.clone());
                val
            }
			// FIXME: avoid clone, it's slow
            Var(ref var) => env.get(&var[..]).unwrap().clone(),
            Literal(lit) => DataType::Number(lit),
            LiteralString(ref litstr) => DataType::String(litstr.to_string()),
            Return(ref e) => {
                let val = interp_expr(env, e);
                println!("{:?}", val);
				val
            }
            Print(ref e) => {
                let val = interp_expr(env, e);
                println!("{:?}", val);
                val
            }
        }
    }
}
