use core::marker::PhantomData;
use schemars::{gen::SchemaGenerator, schema::Schema, JsonSchema};
use serde::de::{DeserializeOwned, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;
use std::fmt::Debug;

#[derive(Debug, Clone)]
pub enum TryParse<T: JsonSchema> {
    Parsed(T),
    Unparsed(Value),
    NotPresent,
}

impl<T: JsonSchema> JsonSchema for TryParse<T> {
    fn schema_name() -> String {
        format!("Try<{}>", std::any::type_name::<T>())
    }

    fn json_schema(gen: &mut SchemaGenerator) -> Schema {
        gen.subschema_for::<T>()
    }
}

impl<'de, T: DeserializeOwned + JsonSchema> Deserialize<'de> for TryParse<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Option::<Value>::deserialize(deserializer)? {
            None => Ok(TryParse::NotPresent),
            Some(value) => match T::deserialize(&value) {
                Ok(t) => Ok(TryParse::Parsed(t)),
                Err(_) => Ok(TryParse::Unparsed(value)),
            },
        }
    }
}

#[derive(Debug, Clone, JsonSchema)]
#[serde(transparent)]
pub struct TryVec<T: JsonSchema> {
    inner: Vec<TryParse<T>>,
}

impl<T: JsonSchema> TryVec<T> {
    pub fn new(mut vec: Vec<T>) -> Self {
        let mut vec_marked: Vec<TryParse<T>> = Vec::new();
        for item in vec.drain(..) {
            vec_marked.push(TryParse::Parsed(item));
        }
        TryVec { inner: vec_marked }
    }

    pub fn new_empty() -> Self {
        TryVec { inner: Vec::new() }
    }

    pub fn take_inner(self) -> Vec<T> {
        let mut vec: Vec<T> = Vec::new();
        for item in self.inner {
            match item {
                TryParse::Parsed(i) => vec.push(i),
                _ => continue,
            };
        }
        vec
    }
}

impl<T: JsonSchema> Serialize for TryVec<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.inner.len()))?;
        for element in &self.inner {
            match element {
                TryParse::Parsed(t) => seq.serialize_element(t)?,
                _ => continue,
            };
        }
        seq.end()
    }
}

struct TryVecVisitor<T: JsonSchema> {
    marker: PhantomData<fn() -> TryVec<T>>,
}

impl<T: JsonSchema> TryVecVisitor<T> {
    fn new() -> Self {
        TryVecVisitor {
            marker: PhantomData,
        }
    }
}

impl<'de, T> Visitor<'de> for TryVecVisitor<T>
where
    T: DeserializeOwned + JsonSchema,
{
    type Value = TryVec<T>;

    // Format a message stating what data this Visitor expects to receive.
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a sequence")
    }

    fn visit_seq<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: SeqAccess<'de>,
    {
        let mut vec = Vec::new();

        loop {
            let res = match access.next_element() {
                Ok(val) => val,
                Err(err) => {
                    println!("Failed to parse event because '{err}', the event will be discarded");
                    continue;
                }
            };
            match res {
                Some(item) => vec.push(item),
                None => break,
            };
        }

        Ok(TryVec { inner: vec })
    }
}

impl<'de, T: JsonSchema> Deserialize<'de> for TryVec<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(TryVecVisitor::new())
    }
}

#[cfg(test)]
mod test {
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    use super::TryVec;

    #[derive(Deserialize, Serialize, JsonSchema, Debug)]
    struct TestEvent {
        data: String,
    }

    fn assert_serialized_deserialized_eq(data: &str, eq: &str) {
        let deserialized = serde_json::from_str::<TryVec<TestEvent>>(data).unwrap();
        let serialized = serde_json::to_string(&deserialized).unwrap();
        assert_eq!(serialized, eq);
    }

    #[test]
    fn test_serialize_deserialize() {
        println!("test empty array");
        assert_serialized_deserialized_eq(r#"[]"#, r#"[]"#);

        println!("test one valid event");
        assert_serialized_deserialized_eq(r#"[{"data":"test"}]"#, r#"[{"data":"test"}]"#);

        println!("test invalid type int, skip event");
        assert_serialized_deserialized_eq(r#"[{ "data": 1 }]"#, r#"[]"#);

        println!("test invalid type dict, skip event");
        assert_serialized_deserialized_eq(r#"[{"data":{}}]"#, r#"[]"#);

        println!("test invalid type arr, skip event");
        assert_serialized_deserialized_eq(r#"[{"data":[]}]"#, r#"[]"#);

        println!("test multiple valid events");
        assert_serialized_deserialized_eq(
            r#"[{"data":"test"},{"data":"test2"},{"data":"test3"}]"#,
            r#"[{"data":"test"},{"data":"test2"},{"data":"test3"}]"#,
        );

        println!("test invalid event in middle of sequence, skip one event");
        assert_serialized_deserialized_eq(
            r#"[{"data":"test"},{"data":2},{"data":"test3"}]"#,
            r#"[{"data":"test"},{"data":"test3"}]"#,
        );

        println!("test utf-16 character");
        assert_serialized_deserialized_eq(r#"[{"data":"\ud835\udc47"}]"#, r#"[{"data":"𝑇"}]"#);

        println!("test invalid utf-8/16, skip event");
        assert_serialized_deserialized_eq(r#"[{"data":"\ud835"}]"#, r#"[]"#);
    }

    #[test]
    fn test_methods() {
        let tryvec = TryVec::<TestEvent>::new_empty();
        assert_eq!(tryvec.take_inner().len(), Vec::<TestEvent>::new().len());
    }
}
