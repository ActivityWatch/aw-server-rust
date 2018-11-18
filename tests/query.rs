extern crate aw_server;

extern crate plex;

#[cfg(test)]
mod query_tests {
    use aw_server::query;

	#[test]
    fn test_number() {
        let code = String::from("1;1.;1.1;");
		query::query(&code).unwrap();
    }

    #[test]
    fn test_return() {
        let code = String::from("return 1;");
		query::query(&code).unwrap();
    }

	#[test]
    fn test_function() {
        let code = String::from("print(1);");
		query::query(&code).unwrap();
    }

    #[test]
	fn test_string() {
        let code = String::from("a=\"test\";");
		let ret = query::query(&code).unwrap();
		match ret {
			query::DataType::String(s) => assert_eq!(s, "test"),
			_ => panic!("Wrong datatype")
		}
	}

	#[test]
	fn test_comment() {
        let code = String::from("1;# testing 123");
		query::query(&code).unwrap();
	}
}
