use crate::DataType;
use crate::QueryError;
use crate::VarEnv;
use aw_datastore::Datastore;

pub type QueryFn =
    fn(args: Vec<DataType>, env: &VarEnv, ds: &Datastore) -> Result<DataType, QueryError>;

pub fn fill_env(env: &mut VarEnv) {
    env.insert(
        "print".to_string(),
        DataType::Function("print".to_string(), qfunctions::print),
    );
    env.insert(
        "query_bucket".to_string(),
        DataType::Function("query_bucket".to_string(), qfunctions::query_bucket),
    );
    env.insert(
        "query_bucket_names".to_string(),
        DataType::Function(
            "query_bucket_names".to_string(),
            qfunctions::query_bucket_names,
        ),
    );
    env.insert(
        "sort_by_duration".to_string(),
        DataType::Function("sort_by_duration".to_string(), qfunctions::sort_by_duration),
    );
    env.insert(
        "sort_by_timestamp".to_string(),
        DataType::Function(
            "sort_by_timestamp".to_string(),
            qfunctions::sort_by_timestamp,
        ),
    );
    env.insert(
        "sum_durations".to_string(),
        DataType::Function("sum_durations".to_string(), qfunctions::sum_durations),
    );
    env.insert(
        "limit_events".to_string(),
        DataType::Function("limit_events".to_string(), qfunctions::limit_events),
    );
    env.insert(
        "contains".to_string(),
        DataType::Function("contains".to_string(), qfunctions::contains),
    );
    env.insert(
        "flood".to_string(),
        DataType::Function("flood".to_string(), qfunctions::flood),
    );
    env.insert(
        "find_bucket".to_string(),
        DataType::Function("find_bucket".to_string(), qfunctions::find_bucket),
    );
    env.insert(
        "merge_events_by_keys".to_string(),
        DataType::Function(
            "merge_events_by_keys".to_string(),
            qfunctions::merge_events_by_keys,
        ),
    );
    env.insert(
        "chunk_events_by_key".to_string(),
        DataType::Function(
            "chunk_events_by_key".to_string(),
            qfunctions::chunk_events_by_key,
        ),
    );
    env.insert(
        "exclude_keyvals".to_string(),
        DataType::Function("exclude_keyvals".to_string(), qfunctions::exclude_keyvals),
    );
    env.insert(
        "filter_keyvals".to_string(),
        DataType::Function("filter_keyvals".to_string(), qfunctions::filter_keyvals),
    );
    env.insert(
        "filter_keyvals_regex".to_string(),
        DataType::Function(
            "filter_keyvals_regex".to_string(),
            qfunctions::filter_keyvals_regex,
        ),
    );
    env.insert(
        "filter_period_intersect".to_string(),
        DataType::Function(
            "filter_period_intersect".to_string(),
            qfunctions::filter_period_intersect,
        ),
    );
    env.insert(
        "split_url_events".to_string(),
        DataType::Function("split_url_events".to_string(), qfunctions::split_url_events),
    );
    env.insert(
        "concat".to_string(),
        DataType::Function("concat".to_string(), qfunctions::concat),
    );
    env.insert(
        "categorize".to_string(),
        DataType::Function("categorize".into(), qfunctions::categorize),
    );
    env.insert(
        "tag".to_string(),
        DataType::Function("tag".into(), qfunctions::tag),
    );
    env.insert(
        "period_union".to_string(),
        DataType::Function("period_union".into(), qfunctions::period_union),
    );
    env.insert(
        "union_no_overlap".to_string(),
        DataType::Function("union_no_overlap".into(), qfunctions::union_no_overlap),
    );
}

mod qfunctions {
    use aw_datastore::Datastore;
    use aw_models::Event;
    use aw_transform::classify::Rule;

    use super::validate;
    use crate::DataType;
    use crate::QueryError;
    use crate::VarEnv;

    pub fn print(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        for arg in args {
            info!("{:?}", arg);
        }
        Ok(DataType::None())
    }

    pub fn query_bucket(
        args: Vec<DataType>,
        env: &VarEnv,
        ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // Typecheck
        validate::args_length(&args, 1)?;

        let bucket_id: String = (&args[0]).try_into()?;
        let interval = validate::get_timeinterval(env)?;

        let events = match ds.get_events(
            bucket_id.as_str(),
            Some(*interval.start()),
            Some(*interval.end()),
            None,
        ) {
            Ok(events) => events,
            Err(e) => {
                return Err(QueryError::BucketQueryError(format!(
                    "Failed to query bucket: {e:?}"
                )))
            }
        };
        let mut ret = Vec::new();
        for event in events {
            ret.push(DataType::Event(event));
        }
        Ok(DataType::List(ret))
    }

    pub fn query_bucket_names(
        args: Vec<DataType>,
        _env: &VarEnv,
        ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        validate::args_length(&args, 0)?;
        let mut bucketnames: Vec<DataType> = Vec::new();
        let buckets = match ds.get_buckets() {
            Ok(buckets) => buckets,
            Err(e) => {
                return Err(QueryError::BucketQueryError(format!(
                    "Failed to query bucket names: {e:?}"
                )))
            }
        };
        for bucketname in buckets.keys() {
            bucketnames.push(DataType::String(bucketname.to_string()));
        }
        Ok(DataType::List(bucketnames))
    }

    pub fn find_bucket(
        args: Vec<DataType>,
        _env: &VarEnv,
        ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        validate::args_length(&args, 1).or_else(|_| validate::args_length(&args, 2))?;

        let bucket_filter: String = (&args[0]).try_into()?;
        let hostname_filter: Option<String> = match args.len() {
            2 => Some((&args[1]).try_into()?),
            _ => None,
        };

        let buckets = match ds.get_buckets() {
            Ok(buckets) => buckets,
            Err(e) => {
                return Err(QueryError::BucketQueryError(format!(
                    "Failed to query bucket names: {e:?}"
                )))
            }
        };
        let bucketname = match aw_transform::find_bucket(
            &bucket_filter,
            &hostname_filter,
            buckets.values(),
        ) {
            Some(bucketname) => bucketname,
            None => {
                return Err(QueryError::BucketQueryError(match hostname_filter {
                        None => {
                            format!("Failed to find bucket matching filter '{bucket_filter}'")
                        }
                        Some(hostname_filter) => format!(
                            "Failed to find bucket matching filter '{bucket_filter}' and hostname '{hostname_filter}'"
                        ),
                    }));
            }
        };
        Ok(DataType::String(bucketname))
    }

    pub fn contains(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        match args.get(0).unwrap() {
            DataType::List(ref list) => Ok(DataType::Bool(list.contains(&args[1]))),
            DataType::Dict(ref dict) => {
                let s = match &args[1] {
                    DataType::String(s) => s.to_string(),
                    _ => {
                        return Err(QueryError::InvalidFunctionParameters(format!(
                            "function contains got second argument {:?}, expected type String",
                            args[0]
                        )))
                    }
                };
                Ok(DataType::Bool(dict.contains_key(&s)))
            }
            _ => Err(QueryError::InvalidFunctionParameters(format!(
                "function contains got first argument {:?}, expected type List or Dict",
                args[0]
            ))),
        }
    }

    pub fn flood(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let events: Vec<Event> = (&args[0]).try_into()?;
        // Run flood
        let mut flooded_events = aw_transform::flood(events, chrono::Duration::seconds(5));
        // Put events back into DataType::Event container
        let mut tagged_flooded_events = Vec::new();
        for event in flooded_events.drain(..) {
            tagged_flooded_events.push(DataType::Event(event));
        }
        Ok(DataType::List(tagged_flooded_events))
    }

    pub fn categorize(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events: Vec<Event> = Vec::try_from(&args[0])?;
        let rules: Vec<(Vec<String>, Rule)> = Vec::try_from(&args[1])?;
        // Run categorize
        let mut flooded_events = aw_transform::classify::categorize(events, &rules);
        // Put events back into DataType::Event container
        let mut tagged_flooded_events = Vec::new();
        for event in flooded_events.drain(..) {
            tagged_flooded_events.push(DataType::Event(event));
        }
        Ok(DataType::List(tagged_flooded_events))
    }

    pub fn tag(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events: Vec<Event> = Vec::try_from(&args[0])?;
        let rules: Vec<(String, Rule)> = Vec::try_from(&args[1])?;
        // Run categorize
        let mut flooded_events = aw_transform::classify::tag(events, &rules);
        // Put events back into DataType::Event container
        let mut tagged_flooded_events = Vec::new();
        for event in flooded_events.drain(..) {
            tagged_flooded_events.push(DataType::Event(event));
        }
        Ok(DataType::List(tagged_flooded_events))
    }

    pub fn sort_by_duration(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let events: Vec<Event> = (&args[0]).try_into()?;

        // Sort by duration
        let mut sorted_events = aw_transform::sort_by_duration(events);
        // Put events back into DataType::Event container
        let mut tagged_sorted_events = Vec::new();
        for event in sorted_events.drain(..) {
            tagged_sorted_events.push(DataType::Event(event));
        }
        Ok(DataType::List(tagged_sorted_events))
    }

    pub fn limit_events(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let mut events: Vec<Event> = (&args[0]).try_into()?;
        let mut limit: usize = (&args[1]).try_into()?;

        if events.len() < limit {
            limit = events.len()
        }
        let mut limited_tagged_events = Vec::new();
        for event in events.drain(0..limit) {
            limited_tagged_events.push(DataType::Event(event));
        }
        Ok(DataType::List(limited_tagged_events))
    }

    pub fn sort_by_timestamp(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let events: Vec<Event> = (&args[0]).try_into()?;

        // Sort by duration
        let mut sorted_events = aw_transform::sort_by_timestamp(events);
        // Put events back into DataType::Event container
        let mut tagged_sorted_events = Vec::new();
        for event in sorted_events.drain(..) {
            tagged_sorted_events.push(DataType::Event(event));
        }
        Ok(DataType::List(tagged_sorted_events))
    }

    pub fn sum_durations(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let mut events: Vec<Event> = (&args[0]).try_into()?;

        // Sort by duration
        let mut sum_durations = chrono::Duration::zero();
        for event in events.drain(..) {
            sum_durations = sum_durations + event.duration;
        }
        Ok(DataType::Number(
            (sum_durations.num_milliseconds() as f64) / 1000.0,
        ))
    }

    pub fn merge_events_by_keys(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events: Vec<Event> = (&args[0]).try_into()?;
        let keys: Vec<String> = (&args[1]).try_into()?;

        let mut merged_events = aw_transform::merge_events_by_keys(events, keys);
        let mut merged_tagged_events = Vec::new();
        for event in merged_events.drain(..) {
            merged_tagged_events.push(DataType::Event(event));
        }
        Ok(DataType::List(merged_tagged_events))
    }

    pub fn chunk_events_by_key(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events: Vec<Event> = (&args[0]).try_into()?;
        let key: String = (&args[1]).try_into()?;

        let mut merged_events = aw_transform::chunk_events_by_key(events, &key);
        let mut merged_tagged_events = Vec::new();
        for event in merged_events.drain(..) {
            merged_tagged_events.push(DataType::Event(event));
        }
        Ok(DataType::List(merged_tagged_events))
    }

    pub fn filter_keyvals(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 3)?;
        let events = (&args[0]).try_into()?;
        let key: String = (&args[1]).try_into()?;
        let vals: Vec<_> = (&args[2]).try_into()?;

        let mut filtered_events = aw_transform::filter_keyvals(events, &key, &vals);
        let mut filtered_tagged_events = Vec::new();
        for event in filtered_events.drain(..) {
            filtered_tagged_events.push(DataType::Event(event));
        }
        Ok(DataType::List(filtered_tagged_events))
    }

    use fancy_regex::RegexBuilder;

    pub fn filter_keyvals_regex(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 3)?;
        let events = (&args[0]).try_into()?;
        let key: String = (&args[1]).try_into()?;
        let regex_str: String = (&args[2]).try_into()?;
        let regex = match RegexBuilder::new(&regex_str).build() {
            Ok(regex) => regex,
            Err(e) => {
                return Err(QueryError::RegexCompileError(format!(
                    "Failed to compile regex string '{regex_str}': {e}"
                )))
            }
        };

        let mut filtered_events = aw_transform::filter_keyvals_regex(events, &key, &regex);
        let mut filtered_tagged_events = Vec::new();
        for event in filtered_events.drain(..) {
            filtered_tagged_events.push(DataType::Event(event));
        }
        Ok(DataType::List(filtered_tagged_events))
    }

    pub fn exclude_keyvals(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 3)?;
        let events = (&args[0]).try_into()?;
        let key: String = (&args[1]).try_into()?;
        let vals: Vec<_> = (&args[2]).try_into()?;

        let mut filtered_events = aw_transform::exclude_keyvals(events, &key, &vals);
        let mut filtered_tagged_events = Vec::new();
        for event in filtered_events.drain(..) {
            filtered_tagged_events.push(DataType::Event(event));
        }
        Ok(DataType::List(filtered_tagged_events))
    }

    pub fn filter_period_intersect(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events: Vec<Event> = (&args[0]).try_into()?;
        let filter_events: Vec<Event> = (&args[1]).try_into()?;

        let mut filtered_events = aw_transform::filter_period_intersect(events, filter_events);
        let mut filtered_tagged_events = Vec::new();
        for event in filtered_events.drain(..) {
            filtered_tagged_events.push(DataType::Event(event));
        }
        Ok(DataType::List(filtered_tagged_events))
    }

    pub fn split_url_events(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 1)?;
        let mut events: Vec<Event> = (&args[0]).try_into()?;

        let mut tagged_split_url_events = Vec::new();
        for mut event in events.drain(..) {
            aw_transform::split_url_event(&mut event);
            tagged_split_url_events.push(DataType::Event(event));
        }
        Ok(DataType::List(tagged_split_url_events))
    }

    pub fn concat(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        let mut event_list = Vec::new();
        for arg in args {
            let mut events: Vec<Event> = (&arg).try_into()?;
            for event in events.drain(..) {
                event_list.push(DataType::Event(event));
            }
        }
        Ok(DataType::List(event_list))
    }

    pub fn period_union(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events1: Vec<Event> = (&args[0]).try_into()?;
        let events2: Vec<Event> = (&args[1]).try_into()?;

        let mut result = aw_transform::period_union(&events1, &events2);
        let mut result_tagged = Vec::new();
        for event in result.drain(..) {
            result_tagged.push(DataType::Event(event));
        }
        Ok(DataType::List(result_tagged))
    }

    pub fn union_no_overlap(
        args: Vec<DataType>,
        _env: &VarEnv,
        _ds: &Datastore,
    ) -> Result<DataType, QueryError> {
        // typecheck
        validate::args_length(&args, 2)?;
        let events1: Vec<Event> = (&args[0]).try_into()?;
        let events2: Vec<Event> = (&args[1]).try_into()?;

        let mut result = aw_transform::union_no_overlap(events1, events2);
        let mut result_tagged = Vec::new();
        for event in result.drain(..) {
            result_tagged.push(DataType::Event(event));
        }
        Ok(DataType::List(result_tagged))
    }
}

mod validate {
    use crate::{DataType, QueryError, VarEnv};
    use aw_models::TimeInterval;

    pub fn args_length(args: &[DataType], len: usize) -> Result<(), QueryError> {
        if args.len() != len {
            return Err(QueryError::InvalidFunctionParameters(format!(
                "Expected {} parameters in function, got {}",
                len,
                args.len()
            )));
        }
        Ok(())
    }

    pub fn get_timeinterval(env: &VarEnv) -> Result<TimeInterval, QueryError> {
        let interval_str = match env.get("TIMEINTERVAL") {
            Some(data_ti) => match data_ti {
                DataType::String(ti_str) => ti_str,
                _ => {
                    return Err(QueryError::TimeIntervalError(
                        "TIMEINTERVAL is not of type string!".to_string(),
                    ))
                }
            },
            None => {
                return Err(QueryError::TimeIntervalError(
                    "TIMEINTERVAL not defined!".to_string(),
                ))
            }
        };
        match TimeInterval::new_from_string(interval_str) {
            Ok(ti) => Ok(ti),
            Err(_e) => Err(QueryError::TimeIntervalError(format!(
                "Failed to parse TIMEINTERVAL: {interval_str}"
            ))),
        }
    }
}
