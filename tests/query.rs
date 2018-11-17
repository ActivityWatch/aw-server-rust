extern crate aw_server;

extern crate plex;

#[cfg(test)]
mod query_tests {
    use aw_server::transform;
    use aw_server::query;
    use plex::{lexer, parser};

	#[test]
    fn test_number() {
        let s = String::from("1;1.;1.1;");
		let lexer = query::lexer::Lexer::new(&s)
			.inspect(|tok| eprintln!("tok: {:?}", tok));
		let program = query::parser::parse(lexer).unwrap();
		// TODO: Actually get return value
		query::interp::interp(&program);
    }

    #[test]
    fn test_return() {
        let s = String::from("return 1;");
		let lexer = query::lexer::Lexer::new(&s)
			.inspect(|tok| eprintln!("tok: {:?}", tok));
		let program = query::parser::parse(lexer).unwrap();
		// TODO: Actually get return value
		query::interp::interp(&program);
    }

    #[test]
	fn test_string() {
        let s = String::from("a=\"test\";");
		let lexer = query::lexer::Lexer::new(&s)
			.inspect(|tok| eprintln!("tok: {:?}", tok));
		let program = query::parser::parse(lexer).unwrap();
		let ret = query::interp::interp(&program).unwrap();
		match ret {
			query::DataType::String(s) => assert_eq!(s, "test"),
			_ => panic!("Wrong datatype")
		}
	}

	#[test]
	fn test_comment() {
        let s = String::from("1;# testing 123");
		let lexer = query::lexer::Lexer::new(&s)
			.inspect(|tok| eprintln!("tok: {:?}", tok));
		let program = query::parser::parse(lexer).unwrap();
		let ret = query::interp::interp(&program).unwrap();
	}
}
