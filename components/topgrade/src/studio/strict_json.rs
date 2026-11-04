use color_eyre::eyre::{Context, Result};
use serde::{
    Deserialize, Deserializer,
    de::{Error, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::fmt;

struct Unique(Value);

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(input: D) -> std::result::Result<Self, D::Error> {
        struct Values;
        impl<'de> Visitor<'de> for Values {
            type Value = Unique;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON without duplicate keys")
            }

            fn visit_unit<E: Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }

            fn visit_bool<E: Error>(self, value: bool) -> Result<Unique, E> {
                Ok(Unique(Value::Bool(value)))
            }

            fn visit_i64<E: Error>(self, value: i64) -> Result<Unique, E> {
                Ok(Unique(Value::from(value)))
            }

            fn visit_u64<E: Error>(self, value: u64) -> Result<Unique, E> {
                Ok(Unique(Value::from(value)))
            }

            fn visit_f64<E: Error>(self, value: f64) -> Result<Unique, E> {
                Ok(Unique(Value::from(value)))
            }

            fn visit_str<E: Error>(self, value: &str) -> Result<Unique, E> {
                Ok(Unique(Value::from(value)))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Unique, A::Error> {
                let mut items = Vec::new();
                while let Some(Unique(value)) = sequence.next_element()? {
                    items.push(value);
                }
                Ok(Unique(Value::Array(items)))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
                let mut items = Map::new();
                while let Some((key, Unique(value))) = map.next_entry()? {
                    if items.insert(key, value).is_some() {
                        return Err(A::Error::custom("duplicate JSON field"));
                    }
                }
                Ok(Unique(Value::Object(items)))
            }
        }
        input.deserialize_any(Values)
    }
}

pub(super) fn parse(input: &[u8]) -> Result<Value> {
    let value: Unique = serde_json::from_slice(input).wrap_err("Invalid or ambiguous JSON")?;
    Ok(value.0)
}
