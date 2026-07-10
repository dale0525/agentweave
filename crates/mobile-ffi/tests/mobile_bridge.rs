use mobile_ffi::{
    MobileInitConfig, MobileModelConfigDto, MobileRuntime, close_runtime, initialize_runtime_json,
    invoke_runtime_json, send_message_json,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn init_config(root: &std::path::Path) -> MobileInitConfig {
    let app_data_dir = root.join("files");
    MobileInitConfig {
        app_data_dir: app_data_dir.display().to_string(),
        cache_dir: root.join("cache").display().to_string(),
        database_path: app_data_dir.join("general-agent.db").display().to_string(),
        skills_dir: "skills".into(),
        platform: "android".into(),
        capabilities: vec![
            "network.http".into(),
            "filesystem.app_data".into(),
            "secure_storage".into(),
            "model.http_provider".into(),
        ],
    }
}

fn model_config(base_url: String) -> MobileModelConfigDto {
    MobileModelConfigDto {
        provider_id: "openai".into(),
        provider_name: "OpenAI".into(),
        endpoint_type: "responses".into(),
        base_url,
        model_name: "gpt-test".into(),
        secret_id: Some("model.openai.default".into()),
        headers: BTreeMap::new(),
    }
}

#[test]
fn runtime_persists_sessions_and_non_secret_model_config_across_restart() {
    let dir = tempdir().unwrap();
    let config = init_config(dir.path());
    let runtime = MobileRuntime::initialize(config.clone()).unwrap();

    let session = runtime.create_session("Android runtime").unwrap();
    runtime
        .save_model_config(model_config("https://api.openai.com/v1".into()))
        .unwrap();
    drop(runtime);

    let restarted = MobileRuntime::initialize(config).unwrap();
    assert_eq!(restarted.list_sessions().unwrap()[0].id, session.id);
    assert_eq!(
        restarted.load_model_config().unwrap().unwrap().secret_id,
        Some("model.openai.default".into()),
    );
    assert!(restarted.diagnostics().model_configured);
}

#[test]
fn json_bridge_uses_handles_for_session_operations() {
    let dir = tempdir().unwrap();
    let initialized: Value = serde_json::from_str(&initialize_runtime_json(
        &serde_json::to_string(&init_config(dir.path())).unwrap(),
    ))
    .unwrap();
    let handle = initialized["data"]["handle"].as_i64().unwrap();

    let created: Value = serde_json::from_str(&invoke_runtime_json(
        handle,
        &json!({"operation": "create_session", "title": "Bridge session"}).to_string(),
    ))
    .unwrap();
    let session_id = created["data"]["id"].as_str().unwrap();
    let listed: Value = serde_json::from_str(&invoke_runtime_json(
        handle,
        &json!({"operation": "list_sessions"}).to_string(),
    ))
    .unwrap();

    assert_eq!(listed["data"][0]["id"], session_id);
    let closed: Value = serde_json::from_str(&close_runtime(handle)).unwrap();
    assert_eq!(closed["ok"], true);
}

#[test]
fn real_http_turn_uses_transient_api_key_without_persisting_it() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.starts_with("POST /v1/responses "));
        assert!(request_text.contains("authorization: Bearer sk-transient"));
        let body = json!({
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "hello from mock"}]
            }]
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body,
        )
        .unwrap();
    });

    let dir = tempdir().unwrap();
    let config = init_config(dir.path());
    let database_path = config.database_path.clone();
    let runtime = MobileRuntime::initialize(config).unwrap();
    let session = runtime.create_session("HTTP turn").unwrap();
    runtime
        .save_model_config(model_config(format!("http://{address}/v1")))
        .unwrap();

    let turn = runtime
        .send_message(&session.id, "hello", Some("sk-transient".into()))
        .unwrap();
    server.join().unwrap();

    assert_eq!(turn.assistant_text, "hello from mock");
    assert_eq!(runtime.get_messages(&session.id).unwrap().len(), 2);
    assert!(
        !std::fs::read(database_path)
            .unwrap()
            .windows("sk-transient".len())
            .any(|window| window == b"sk-transient")
    );
}

#[test]
fn bridge_send_message_keeps_api_key_out_of_json_payloads() {
    let dir = tempdir().unwrap();
    let initialized: Value = serde_json::from_str(&initialize_runtime_json(
        &serde_json::to_string(&init_config(dir.path())).unwrap(),
    ))
    .unwrap();
    let handle = initialized["data"]["handle"].as_i64().unwrap();

    let response: Value = serde_json::from_str(&send_message_json(
        handle,
        &json!({"session_id": "missing", "content": "hello"}).to_string(),
        Some("sk-separate-argument".into()),
    ))
    .unwrap();

    assert_eq!(response["ok"], false);
    assert!(!response.to_string().contains("sk-separate-argument"));
    close_runtime(handle);
}

#[test]
fn missing_model_config_preserves_submitted_user_message() {
    let dir = tempdir().unwrap();
    let runtime = MobileRuntime::initialize(init_config(dir.path())).unwrap();
    let session = runtime.create_session("Unconfigured turn").unwrap();

    let error = runtime
        .send_message(&session.id, "keep before setup", None)
        .unwrap_err();
    let messages = runtime.get_messages(&session.id).unwrap();

    assert!(
        error
            .to_string()
            .contains("model configuration is required")
    );
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "keep before setup");
}

#[test]
fn failed_http_turn_preserves_submitted_user_message() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 4096];
        let _ = stream.read(&mut buffer).unwrap();
        let body = r#"{"error":{"message":"provider unavailable"}}"#;
        write!(
            stream,
            "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body,
        )
        .unwrap();
    });

    let dir = tempdir().unwrap();
    let runtime = MobileRuntime::initialize(init_config(dir.path())).unwrap();
    let session = runtime.create_session("Failed turn").unwrap();
    runtime
        .save_model_config(model_config(format!("http://{address}/v1")))
        .unwrap();

    let error = runtime
        .send_message(
            &session.id,
            "keep this message",
            Some("sk-transient".into()),
        )
        .unwrap_err();
    server.join().unwrap();
    let messages = runtime.get_messages(&session.id).unwrap();

    assert!(error.to_string().contains("503 Service Unavailable"));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "keep this message");
}

#[test]
fn closing_runtime_cancels_stalled_http_turn() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 4096];
        let _ = stream.read(&mut buffer).unwrap();
        accepted_tx.send(()).unwrap();
        thread::sleep(Duration::from_secs(3));
    });

    let dir = tempdir().unwrap();
    let init = init_config(dir.path());
    let initialized: Value = serde_json::from_str(&initialize_runtime_json(
        &serde_json::to_string(&init).unwrap(),
    ))
    .unwrap();
    let handle = initialized["data"]["handle"].as_i64().unwrap();
    let created: Value = serde_json::from_str(&invoke_runtime_json(
        handle,
        &json!({"operation": "create_session", "title": "Cancelled turn"}).to_string(),
    ))
    .unwrap();
    let session_id = created["data"]["id"].as_str().unwrap().to_string();
    invoke_runtime_json(
        handle,
        &json!({
            "operation": "save_model_config",
            "config": model_config(format!("http://{address}/v1")),
        })
        .to_string(),
    );
    let send_session_id = session_id.clone();
    let send = thread::spawn(move || {
        send_message_json(
            handle,
            &json!({"session_id": send_session_id, "content": "cancel me"}).to_string(),
            Some("sk-transient".into()),
        )
    });
    accepted_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let started = Instant::now();
    close_runtime(handle);
    let response: Value = serde_json::from_str(&send.join().unwrap()).unwrap();

    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(response["ok"], false);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("closed")
    );

    let restarted = MobileRuntime::initialize(init).unwrap();
    let messages = restarted.get_messages(&session_id).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    server.join().unwrap();
}
