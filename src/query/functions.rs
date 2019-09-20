use std::collections::HashMap;
use crate::query::DataType;
use crate::query::QueryError;
use crate::datastore::Datastore;

pub type QueryFn = fn(args: Vec<DataType>, env: &HashMap<&str, DataType>, ds: &Datastore) -> Result<DataType, QueryError>;

pub fn fill_env<'a>(env: &mut HashMap<&'a str, DataType>) {
    env.insert("print", DataType::Function("print".to_string(), qfunctions::print));
    env.insert("query_bucket", DataType::Function("query_bucket".to_string(), qfunctions::query_bucket));
    env.insert("query_bucket_names", DataType::Function("query_bucket_names".to_string(), qfunctions::query_bucket_names));
    env.insert("sort_by_duration", DataType::Function("sort_by_duration".to_string(), qfunctions::sort_by_duration));
    env.insert("sort_by_timestamp", DataType::Function("sort_by_timestamp".to_string(), qfunctions::sort_by_timestamp));
    env.insert("sum_durations", DataType::Function("sum_durations".to_string(), qfunctions::sum_durations));
    env.insert("limit_events", DataType::Function("limit_events".to_string(), qfunctions::limit_events));
    env.insert("contains", DataType::Function("contains".to_string(), qfunctions::contains));
    env.insert("flood", DataType::Function("flood".to_string(), qfunctions::flood));
    env.insert("merge_events_by_keys", DataType::Function("merge_events_by_keys".to_string(), qfunctions::merge_events_by_keys));
    env.insert("chunk_events_by_key", DataType::Function("chunk_events_by_key".to_string(), qfunctions::chunk_events_by_key));
    env.insert("filter_keyvals", DataType::Function("filter_keyvals".to_string(), qfunctions::filter_keyvals));
    env.insert("filter_period_intersect", DataType::Function("filter_period_intersect".to_string(), qfunctions::filter_period_intersect));
    env.insert("split_url_events", DataType::Function("split_url_events".to_string(), qfunctions::split_url_events));
    env.insert("concat", DataType::Function("concat".to_string(), qfunctions::concat));
    env.insert("classify", DataType::Function("classify".into(), qfunctions::classify));
}

mod qfunctions {
    use std::collections::HashMap;
    use regex::Regex;

    use crate::query::DataType;
    use crate::query::QueryError;
    use crate::datastore::Datastore;
    use crate::transform;
    use super::validate;


    pub fn print(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        for arg in args {
            info!("{:?}", arg);
        }
        return Ok(DataType::None());
    }

    pub fn query_bucket(args: Vec<DataType>, env: &HashMap<&str, DataType>, ds: &Datastore) -> Result<DataType, QueryError> {
        // Typecheck
        validate::args_length(&args, 1)?;
        let bucket_id = validate::arg_type_string(&args[0])?;
        let interval = validate::get_timeinterval (env)?;

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

    pub fn query_bucket_names(args: Vec<DataType>, _env: &HashMap<&str, DataType>, ds: &Datastore) -> Result<DataType, QueryError> {
        validate::args_length(&args, 0)?;
        let mut bucketnames : Vec<DataType> = Vec::new();
        let buckets = match ds.get_buckets() {
            Ok(buckets) => buckets,
            Err(e) => return Err(QueryError::BucketQueryError(format!("Failed to query bucket names: {:?}", e))),
        };
        for bucketname in buckets.keys() {
            bucketnames.push(DataType::String(bucketname.to_string()));
        }
        return Ok(DataType::List(bucketnames));
    }

    pub fn contains(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        match args[0] {
            DataType::List(ref list) => {
                return Ok(DataType::Bool(list.contains(&args[1])));
            },
            DataType::Dict(ref dict) => {
                let s = match &args[1] {
                    DataType::String(s) => s.to_string(),
                    _ => return Err(QueryError::InvalidFunctionParameters(format!("function contains got first argument {:?}, expected type List or Dict", args[0]))),
                };
                return Ok(DataType::Bool(dict.contains_key(&s)));
            },
            _ => return Err(QueryError::InvalidFunctionParameters(format!("function contains got first argument {:?}, expected type List or Dict", args[0]))),
        }
    }

    pub fn flood(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let events = validate::arg_type_event_list(&args[0])?.clone();
        // Run flood
        let mut flooded_events = transform::flood(events, chrono::Duration::seconds(5));
        // Put events back into DataType::Event container
        let mut tagged_flooded_events = Vec::new();
        for event in flooded_events.drain(..) {
            tagged_flooded_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(tagged_flooded_events));
    }

    pub fn classify(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events = validate::arg_type_event_list(&args[0])?.clone();
        let classes = validate::arg_type_list_of_list_of_strings(&args[1])?.clone();
        // Run classify
        let classes_tuples: Vec<(Regex, String)> = classes
            .iter()
            .map(|l| (Regex::new(l.get(0).unwrap()).unwrap(), l.get(1).unwrap().to_string()))
            .collect();
        let mut flooded_events = transform::classify::classify(events, classes_tuples);
        // Put events back into DataType::Event container
        let mut tagged_flooded_events = Vec::new();
        for event in flooded_events.drain(..) {
            tagged_flooded_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(tagged_flooded_events));
    }

    pub fn sort_by_duration(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let events = validate::arg_type_event_list(&args[0])?;

        // Sort by duration
        let mut sorted_events = transform::sort_by_duration(events);
        // Put events back into DataType::Event container
        let mut tagged_sorted_events = Vec::new();
        for event in sorted_events.drain(..) {
            tagged_sorted_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(tagged_sorted_events));
    }

    pub fn limit_events(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let mut events = validate::arg_type_event_list(&args[0])?.clone();
        let mut limit = validate::arg_type_number(&args[1])? as usize;

        if events.len() < limit { limit = events.len() }
        let mut limited_tagged_events = Vec::new();
        for event in events.drain(0..limit) {
            limited_tagged_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(limited_tagged_events));
    }

    pub fn sort_by_timestamp(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let events = validate::arg_type_event_list(&args[0])?;

        // Sort by duration
        let mut sorted_events = transform::sort_by_timestamp(events);
        // Put events back into DataType::Event container
        let mut tagged_sorted_events = Vec::new();
        for event in sorted_events.drain(..) {
            tagged_sorted_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(tagged_sorted_events));
    }

    pub fn sum_durations(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let mut events = validate::arg_type_event_list(&args[0])?.clone();

        // Sort by duration
        let mut sum_durations = chrono::Duration::zero();
        for event in events.drain(..) {
            sum_durations = sum_durations + event.duration;
        }
        return Ok(DataType::Number((sum_durations.num_milliseconds() as f64)/1000.0));
    }

    pub fn merge_events_by_keys(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events = validate::arg_type_event_list(&args[0])?;
        let keys  = validate::arg_type_string_list(&args[1])?;

        let mut merged_events = transform::merge_events_by_keys(events, keys);
        let mut merged_tagged_events = Vec::new();
        for event in merged_events.drain(..) {
            merged_tagged_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(merged_tagged_events));
    }

    pub fn chunk_events_by_key(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events = validate::arg_type_event_list(&args[0])?;
        let key  = validate::arg_type_string(&args[1])?;

        let mut merged_events = transform::chunk_events_by_key(events, &key);
        let mut merged_tagged_events = Vec::new();
        for event in merged_events.drain(..) {
            merged_tagged_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(merged_tagged_events));
    }

    pub fn filter_keyvals(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 3)?;
        let events = validate::arg_type_event_list(&args[0])?;
        let key  = validate::arg_type_string(&args[1])?;
        let vals = validate::arg_type_value_list(&args[2])?;

        let mut filtered_events = transform::filter_keyvals(events, &key, &vals);
        let mut filtered_tagged_events = Vec::new();
        for event in filtered_events.drain(..) {
            filtered_tagged_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(filtered_tagged_events));
    }

    pub fn filter_period_intersect(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events = validate::arg_type_event_list(&args[0])?;
        let filter_events = validate::arg_type_event_list(&args[1])?;

        let mut filtered_events = transform::filter_period_intersect(&events, &filter_events);
        let mut filtered_tagged_events = Vec::new();
        for event in filtered_events.drain(..) {
            filtered_tagged_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(filtered_tagged_events));
    }

    pub fn split_url_events(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let mut events = validate::arg_type_event_list(&args[0])?;

        let mut tagged_split_url_events = Vec::new();
        for mut event in events.drain(..) {
            transform::split_url_event(&mut event);
            tagged_split_url_events.push(DataType::Event(event));
        }
        return Ok(DataType::List(tagged_split_url_events));
    }

    pub fn concat(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
        let mut event_list = Vec::new();
        for arg in args {
            let mut events = validate::arg_type_event_list(&arg)?;
            for event in events.drain(..) {
                event_list.push(DataType::Event(event));
            }
        }
        return Ok(DataType::List(event_list));
    }
}

mod validate {
    use crate::query::{QueryError, DataType};
    use crate::models::Event;
    use crate::models::TimeInterval;
    use std::collections::HashMap;

    pub fn args_length(args: &Vec<DataType>, len: usize) -> Result<(), QueryError> {
        if args.len() != len {
            return Err(QueryError::InvalidFunctionParameters(
                format!("Expected {} parameters in function, got {}", len, args.len())
            ));
        }
        return Ok(());
    }

    pub fn arg_type_string (arg: &DataType) -> Result<&String, QueryError> {
        match arg {
            DataType::String(ref s) => Ok(s),
            ref invalid_type => Err(QueryError::InvalidFunctionParameters(
                format!("Expected function parameter of type String, got {:?}", invalid_type)
            ))
        }
    }

    pub fn arg_type_number (arg: &DataType) -> Result<f64, QueryError> {
        match arg {
            DataType::Number(f) => Ok(*f),
            ref invalid_type => Err(QueryError::InvalidFunctionParameters(
                format!("Expected function parameter of type Number, got {:?}", invalid_type)
            ))
        }
    }

    pub fn arg_type_list (arg: &DataType) -> Result<&Vec<DataType>, QueryError> {
        match arg {
            DataType::List(ref s) => Ok(s),
            ref invalid_type => Err(QueryError::InvalidFunctionParameters(
                format!("Expected function parameter of type List, got {:?}", invalid_type)
            ))
        }
    }

    pub fn arg_type_list_of_list_of_strings (arg: &DataType) -> Result<Vec<Vec<String>>, QueryError> {
        let mut tagged_lists = arg_type_list(arg)?.clone();
        let mut lists: Vec<Vec<String>> = Vec::new();
        for list in tagged_lists.drain(..) {
            match list {
                DataType::List(_) => lists.push(arg_type_string_list(&list)?.clone()),
                ref invalid_type => return Err(QueryError::InvalidFunctionParameters(
                    format!("Expected function parameter of type list of tuples of strings, list contains {:?}", invalid_type)
                ))
            }
        }
        return Ok(lists);
    }

    pub fn arg_type_event_list (arg: &DataType) -> Result<Vec<Event>, QueryError> {
        let mut tagged_events = arg_type_list(arg)?.clone();
        let mut events = Vec::new();
        for event in tagged_events.drain(..) {
            match event {
                DataType::Event(e) => events.push(e.clone()),
                ref invalid_type => return Err(QueryError::InvalidFunctionParameters(
                    format!("Expected function parameter of type List of Events, list contains {:?}", invalid_type)
                ))
            }
        }
        return Ok(events);
    }

    pub fn arg_type_string_list (arg: &DataType) -> Result<Vec<String>, QueryError> {
        let mut tagged_strings = arg_type_list(arg)?.clone();
        let mut strings = Vec::new();
        for string in tagged_strings.drain(..) {
            match string {
                DataType::String(s) => strings.push(s.clone()),
                ref invalid_type => return Err(QueryError::InvalidFunctionParameters(
                    format!("Expected function parameter of type List of Strings, list contains {:?}", invalid_type)
                ))
            }
        }
        return Ok(strings);
    }

    use serde_json::value::Value;
    use serde_json::Number;
    pub fn arg_type_value_list (arg: &DataType) -> Result<Vec<Value>, QueryError> {
        let mut tagged_strings = arg_type_list(arg)?.clone();
        let mut strings = Vec::new();
        for string in tagged_strings.drain(..) {
            match string {
                DataType::String(s) => strings.push(Value::String(s)),
                DataType::Number(n) => strings.push(Value::Number(Number::from_f64(n).unwrap())),
                //DataType::Bool(b) => strings.push(json!(b)),
                DataType::None() => strings.push(Value::Null),
                ref invalid_type => return Err(QueryError::InvalidFunctionParameters(
                    format!("Query2 support for parsing values is limited and only supports strings, numbers and null, list contains {:?}", invalid_type)
                ))
            }
        }
        return Ok(strings);
    }

    pub fn get_timeinterval (env: &HashMap<&str, DataType>) -> Result<TimeInterval, QueryError> {
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
}
