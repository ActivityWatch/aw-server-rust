#[macro_use]
extern crate serde_json;

#[macro_use]
extern crate aw_server;

#[cfg(test)]
mod macros_tests {
    #[test]
    fn test_json_map() {
        json_map! {};
        json_map! {"a": json!(1)};
        json_map! {"a": json!(1), "b": json!(2)};
    }
}
