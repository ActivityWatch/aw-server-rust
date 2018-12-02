use query::DataType;
use query::QueryError;
use datastore::Datastore;
use models::TimeInterval;
use transform;

use std::collections::HashMap;

pub type QueryFn = fn(args: Vec<DataType>, env: &HashMap<&str, DataType>, ds: &Datastore) -> Result<DataType, QueryError>;

pub fn fill_env<'a>(env: &mut HashMap<&'a str, DataType>) {
    env.insert("print", DataType::Function("print".to_string(), -1, q_print));
    env.insert("query_bucket", DataType::Function("query_bucket".to_string(), 1, q_query_bucket));
    env.insert("sort_by_duration", DataType::Function("sort_by_duration".to_string(), 1, q_sort_by_duration));
    env.insert("sort_by_timestamp", DataType::Function("sort_by_timestamp".to_string(), 1, q_sort_by_timestamp));
    env.insert("sum_durations", DataType::Function("sum_durations".to_string(), 1, q_sum_durations));
    env.insert("limit_events", DataType::Function("limit_events".to_string(), 2, q_limit_events));
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

fn q_print(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
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

fn q_flood(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
    let mut tagged_events = match args[0] {
        DataType::List(ref l) => l.clone(),
        ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List, got {:?}", invalid_type)))
    };
    // Move events out of DataType container
    let mut events = Vec::new();
    for event in tagged_events.drain(..) {
        match event {
            DataType::Event(e) => events.push(e.clone()),
            ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List of Events, list contains {:?}", invalid_type)))
        }
    }
    // Run flood
    let mut flooded_events = transform::flood(events, chrono::Duration::seconds(5));
    // Put events back into DataType::Event container
    let mut tagged_flooded_events = Vec::new();
    for event in flooded_events.drain(..) {
        tagged_flooded_events.push(DataType::Event(event));
    }
    return Ok(DataType::List(tagged_flooded_events));
}

fn q_sort_by_duration(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
    let mut tagged_events = match args[0] {
        DataType::List(ref l) => l.clone(),
        ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List, got {:?}", invalid_type)))
    };
    // Move events out of DataType container
    let mut events = Vec::new();
    for event in tagged_events.drain(..) {
        match event {
            DataType::Event(e) => events.push(e.clone()),
            ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List of Events, list contains {:?}", invalid_type)))
        }
    }
    // Sort by duration
    let mut sorted_events = transform::sort_by_duration(events);
    // Put events back into DataType::Event container
    let mut tagged_sorted_events = Vec::new();
    for event in sorted_events.drain(..) {
        tagged_sorted_events.push(DataType::Event(event));
    }
    return Ok(DataType::List(tagged_sorted_events));
}

fn q_limit_events(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
    let mut tagged_events = match args[0] {
        DataType::List(ref l) => l.clone(),
        ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List, got {:?}", invalid_type)))
    };
    let mut limit = match args[1] {
        DataType::Number(ref n) => *n as usize,
        ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List, got {:?}", invalid_type)))
    };
    // Move events out of DataType container
    let mut events = Vec::new();
    for event in tagged_events.drain(..) {
        match event {
            DataType::Event(e) => events.push(e.clone()),
            ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List of Events, list contains {:?}", invalid_type)))
        }
    }
    if events.len() < limit { limit = events.len() }
    let mut limited_tagged_events = Vec::new();
    for event in events.drain(0..limit) {
        limited_tagged_events.push(DataType::Event(event));
    }
    return Ok(DataType::List(limited_tagged_events));
}

fn q_sort_by_timestamp(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
    let mut tagged_events = match args[0] {
        DataType::List(ref l) => l.clone(),
        ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List, got {:?}", invalid_type)))
    };
    // Move events out of DataType container
    let mut events = Vec::new();
    for event in tagged_events.drain(..) {
        match event {
            DataType::Event(e) => events.push(e.clone()),
            ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List of Events, list contains {:?}", invalid_type)))
        }
    }
    // Sort by duration
    let mut sorted_events = transform::sort_by_timestamp(events);
    // Put events back into DataType::Event container
    let mut tagged_sorted_events = Vec::new();
    for event in sorted_events.drain(..) {
        tagged_sorted_events.push(DataType::Event(event));
    }
    return Ok(DataType::List(tagged_sorted_events));
}

fn q_sum_durations(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
    let mut tagged_events = match args[0] {
        DataType::List(ref l) => l.clone(),
        ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List, got {:?}", invalid_type)))
    };
    // Move events out of DataType container
    let mut events = Vec::new();
    for event in tagged_events.drain(..) {
        match event {
            DataType::Event(e) => events.push(e.clone()),
            ref invalid_type => return Err(QueryError::InvalidFunctionParameters(format!("Expected parameter of type List of Events, list contains {:?}", invalid_type)))
        }
    }
    // Sort by duration
    let mut sum_durations = chrono::Duration::zero();
    for event in events.drain(..) {
        sum_durations = sum_durations + event.duration;
    }
    return Ok(DataType::Number((sum_durations.num_milliseconds() as f64)/1000.0));
}

fn q_merge_events_by_keys(args: Vec<DataType>, _env: &HashMap<&str, DataType>, _ds: &Datastore) -> Result<DataType, QueryError> {
    let mut tagged_events = match args[0] {
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
