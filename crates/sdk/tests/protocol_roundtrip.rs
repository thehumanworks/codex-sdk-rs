use codex_app_server_sdk::error::{IncomingClassified, classify_incoming};
use codex_app_server_sdk::events::{ServerNotification, parse_notification};
use codex_app_server_sdk::protocol::requests::{ClientInfo, InitializeParams};
use codex_app_server_sdk::protocol::shared::{JsonRpcRequest, RequestId};
use serde_json::json;

#[test]
fn request_envelope_has_no_jsonrpc_field() {
    let req = JsonRpcRequest {
        method: "initialize".to_string(),
        id: RequestId::Integer(1),
        params: InitializeParams::new(ClientInfo::new("n", "t", "v")),
    };

    let value = serde_json::to_value(req).expect("serialize request");
    let obj = value.as_object().expect("object");

    assert!(obj.get("jsonrpc").is_none());
    assert_eq!(
        obj.get("method").and_then(|v| v.as_str()),
        Some("initialize")
    );
}

#[test]
fn classify_response_result() {
    let input = json!({ "id": 10, "result": {"ok": true} });
    let classified = classify_incoming(input).expect("classify");

    match classified {
        IncomingClassified::Response { id, result } => {
            assert_eq!(id, RequestId::Integer(10));
            assert!(result.is_ok());
        }
        _ => panic!("unexpected variant"),
    }
}

#[test]
fn parse_unknown_notification_preserves_payload() {
    let params = json!({ "x": 1, "nested": {"y": true} });
    let event = parse_notification("custom/new-event".to_string(), params.clone()).expect("parse");

    match event {
        ServerNotification::Unknown {
            method,
            params: found,
        } => {
            assert_eq!(method, "custom/new-event");
            assert_eq!(found, params);
        }
        _ => panic!("expected unknown notification"),
    }
}

#[test]
fn parse_windows_sandbox_setup_completed_notification() {
    let params = json!({ "mode": "elevated", "started": true });
    let event = parse_notification("windowsSandbox/setupCompleted".to_string(), params.clone())
        .expect("parse");

    match event {
        ServerNotification::WindowsSandboxSetupCompleted(payload) => {
            assert_eq!(payload.extra.get("mode"), Some(&json!("elevated")));
            assert_eq!(payload.extra.get("started"), Some(&json!(true)));
        }
        _ => panic!("expected windows sandbox setup completed notification"),
    }
}

#[test]
fn parse_fuzzy_file_search_notifications() {
    let updated_params = json!({ "sessionId": "sess_1", "matches": [] });
    let updated = parse_notification(
        "fuzzyFileSearch/sessionUpdated".to_string(),
        updated_params.clone(),
    )
    .expect("parse updated");
    match updated {
        ServerNotification::FuzzyFileSearchSessionUpdated(payload) => {
            assert_eq!(payload.extra.get("sessionId"), Some(&json!("sess_1")));
            assert_eq!(payload.extra.get("matches"), Some(&json!([])));
        }
        _ => panic!("expected fuzzy session updated notification"),
    }

    let completed_params = json!({ "sessionId": "sess_1", "complete": true });
    let completed = parse_notification(
        "fuzzyFileSearch/sessionCompleted".to_string(),
        completed_params.clone(),
    )
    .expect("parse completed");
    match completed {
        ServerNotification::FuzzyFileSearchSessionCompleted(payload) => {
            assert_eq!(payload.extra.get("sessionId"), Some(&json!("sess_1")));
            assert_eq!(payload.extra.get("complete"), Some(&json!(true)));
        }
        _ => panic!("expected fuzzy session completed notification"),
    }
}
