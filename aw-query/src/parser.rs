use crate::ast::*;
use crate::lexer::Token::*;
use crate::lexer::*;
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
        statements[st] Semi => st,
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

pub fn parse<I: Iterator<Item = (Token, Span)>>(
    i: I,
) -> Result<Program, (Option<(Token, Span)>, &'static str)> {
    parse_(i)
}
