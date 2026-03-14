use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub trait OpenAiSerializable: Sized {
    fn openai_output_schema() -> Value;

    fn to_openai_value(&self) -> serde_json::Result<Value>
    where
        Self: Serialize,
    {
        serialize_openai_value(self)
    }

    fn from_openai_value(value: Value) -> serde_json::Result<Self>
    where
        Self: DeserializeOwned,
    {
        deserialize_openai_value(value)
    }
}

pub fn openai_json_schema_for<T>() -> Value
where
    T: JsonSchema,
{
    let schema = SchemaSettings::default()
        .with(|settings| settings.meta_schema = None)
        .into_generator()
        .into_root_schema_for::<T>();
    let mut schema =
        serde_json::to_value(schema).expect("serializing generated schema should not fail");
    enforce_openai_object_constraints(&mut schema);
    schema
}

pub fn serialize_openai_value<T>(value: &T) -> serde_json::Result<Value>
where
    T: Serialize,
{
    serde_json::to_value(value)
}

pub fn deserialize_openai_value<T>(value: Value) -> serde_json::Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value)
}

fn enforce_openai_object_constraints(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for nested in map.values_mut() {
                enforce_openai_object_constraints(nested);
            }

            let is_object_schema = map.get("type").and_then(Value::as_str) == Some("object");
            if is_object_schema && !map.contains_key("additionalProperties") {
                map.insert("additionalProperties".to_string(), Value::Bool(false));
            }
        }
        Value::Array(values) => {
            for nested in values {
                enforce_openai_object_constraints(nested);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        Serialize,
        Deserialize,
        JsonSchema,
        codex_app_server_sdk::OpenAiSerializable,
    )]
    struct SummarySchema {
        answer: String,
    }

    #[test]
    fn derives_openai_schema_without_meta_schema() {
        let schema = SummarySchema::openai_output_schema();
        assert!(schema.get("$schema").is_none());
        assert_eq!(
            schema.get("type"),
            Some(&Value::String("object".to_string()))
        );
        assert_eq!(
            schema.get("additionalProperties"),
            Some(&Value::Bool(false))
        );
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .expect("schema should include properties object");
        assert!(properties.contains_key("answer"));
    }

    #[test]
    fn helper_serializes_and_deserializes_struct_values() {
        let expected = SummarySchema {
            answer: "ok".to_string(),
        };
        let value = expected
            .to_openai_value()
            .expect("serialize should succeed");
        let parsed = SummarySchema::from_openai_value(value).expect("deserialize should succeed");
        assert_eq!(parsed, expected);
    }

    #[test]
    fn schema_helper_accepts_json_schema_types_directly() {
        let schema = openai_json_schema_for::<SummarySchema>();
        assert_eq!(
            schema.get("type"),
            Some(&Value::String("object".to_string()))
        );
    }
}
