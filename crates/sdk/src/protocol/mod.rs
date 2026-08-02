macro_rules! opaque_struct {
    ($name:ident) => {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            #[serde(flatten)]
            pub extra: serde_json::Map<String, serde_json::Value>,
        }
    };
}

pub mod methods;
pub mod notifications;
pub mod requests;
pub mod responses;
pub mod server_requests;
pub mod shared;
