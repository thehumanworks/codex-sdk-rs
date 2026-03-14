use codex_app_server_sdk::OpenAiSerializable;
use serde_json::Value;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    codex_app_server_sdk::JsonSchema,
    codex_app_server_sdk::OpenAiSerializable,
)]
struct ReexportedSchema {
    answer: String,
}

#[test]
fn downstream_can_derive_json_schema_without_direct_schemars_dependency() {
    let schema = ReexportedSchema::openai_output_schema();
    assert_eq!(
        schema.get("type"),
        Some(&Value::String("object".to_string()))
    );
    assert_eq!(
        schema.get("additionalProperties"),
        Some(&Value::Bool(false))
    );
}

#[codex_app_server_sdk::openai_type]
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenAiTypeSchema {
    #[serde(rename = "final_answer")]
    answer: String,
}

#[test]
fn openai_type_attribute_adds_required_derives_and_crate_paths() {
    let schema = OpenAiTypeSchema::openai_output_schema();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("schema should include properties object");
    assert!(properties.contains_key("final_answer"));

    let parsed = OpenAiTypeSchema::from_openai_value(serde_json::json!({
        "final_answer": "ok"
    }))
    .expect("deserialize should succeed");
    assert_eq!(
        parsed,
        OpenAiTypeSchema {
            answer: "ok".to_string()
        }
    );
}
