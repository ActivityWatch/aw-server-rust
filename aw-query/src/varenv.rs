use std::collections::HashMap;

use crate::datatype::DataType;
use crate::QueryError;

struct Var {
    pub refs: u32,
    pub val: Option<DataType>,
}

pub struct VarEnv {
    vars: HashMap<String, Var>,
}

impl VarEnv {
    pub fn new() -> Self {
        VarEnv {
            vars: HashMap::new(),
        }
    }

    pub fn declare(&mut self, name: String) -> () {
        if !self.vars.contains_key(&name) {
            let var = Var { refs: 0, val: None };
            println!("declare {}", name);
            self.vars.insert(name, var);
        }
    }

    pub fn declare_static(&mut self, name: String, val: DataType) -> () {
        let var = Var {
            refs: std::u32::MAX,
            val: Some(val),
        };
        self.vars.insert(name, var);
    }

    // TODO: rename assign?
    pub fn insert(&mut self, name: String, val: DataType) -> () {
        match self.vars.get_mut(&name) {
            Some(var) => var.val = Some(val),
            None => panic!(format!("fail, not declared {}", name)), // TODO: Properly handle this
        };
        // Return is a little special that it's always taken at the end of the interpretation
        if name == "RETURN" {
            self.add_ref("RETURN").unwrap();
        }
    }

    pub fn add_ref(&mut self, name: &str) -> Result<(), QueryError> {
        match self.vars.get_mut(name) {
            Some(var) => {
                if var.refs != std::u32::MAX {
                    println!("add ref {}, {}", name, var.refs);
                    var.refs += 1
                }
            }
            None => return Err(QueryError::VariableNotDefined(name.to_string())),
        };
        Ok(())
    }

    pub fn take(&mut self, name: &str) -> Option<DataType> {
        let clone: bool = match self.vars.get_mut(name) {
            Some(var) => {
                println!("take {}: {}", name, var.refs);
                var.refs -= 1;
                var.refs > 0
            }
            None => return None,
        };
        if clone {
            match self.vars.get(name) {
                Some(var) => {
                    match var.val {
                        Some(ref val) => Some(val.clone()),
                        None => return None,
                    }
                },
                None => return None,
            }
        } else {
            match self.vars.remove(name) {
                Some(var) => var.val,
                None => return None,
            }
        }
    }

    // TODO: Remove this completely, only needed for TIMEINTERVAL
    pub fn deprecated_get(&self, var: &str) -> Option<DataType> {
        match self.vars.get(var) {
            Some(var) => {
                match var.val {
                    Some(ref val) => Some(val.clone()),
                    None => None,
                }
            },
            None => None,
        }
    }
}
