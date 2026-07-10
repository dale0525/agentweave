use crate::{MobileInitConfig, MobileModelConfigDto, MobileRuntime};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);
static RUNTIMES: OnceLock<Mutex<HashMap<i64, Arc<Mutex<MobileRuntime>>>>> = OnceLock::new();

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum RuntimeRequest {
    Diagnostics,
    CreateSession { title: String },
    ListSessions,
    GetMessages { session_id: String },
    DeleteSession { session_id: String },
    SaveModelConfig { config: MobileModelConfigDto },
    LoadModelConfig,
}

#[derive(Debug, Deserialize)]
struct SendMessageRequest {
    session_id: String,
    content: String,
}

#[derive(Serialize)]
struct BridgeError {
    code: &'static str,
    message: String,
}

pub fn initialize_runtime_json(request_json: &str) -> String {
    let result = (|| {
        let config: MobileInitConfig = serde_json::from_str(request_json)?;
        let runtime = MobileRuntime::initialize(config)?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        runtimes()
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime registry is unavailable"))?
            .insert(handle, Arc::new(Mutex::new(runtime)));
        Ok(json!({"handle": handle}))
    })();
    bridge_result(result)
}

pub fn invoke_runtime_json(handle: i64, request_json: &str) -> String {
    let result = (|| {
        let request: RuntimeRequest = serde_json::from_str(request_json)?;
        let runtime = runtime(handle)?;
        let runtime = runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime handle is unavailable"))?;
        match request {
            RuntimeRequest::Diagnostics => serde_json::to_value(runtime.diagnostics()),
            RuntimeRequest::CreateSession { title } => {
                serde_json::to_value(runtime.create_session(&title)?)
            }
            RuntimeRequest::ListSessions => serde_json::to_value(runtime.list_sessions()?),
            RuntimeRequest::GetMessages { session_id } => {
                serde_json::to_value(runtime.get_messages(&session_id)?)
            }
            RuntimeRequest::DeleteSession { session_id } => {
                runtime.delete_session(&session_id)?;
                Ok(Value::Null)
            }
            RuntimeRequest::SaveModelConfig { config } => {
                runtime.save_model_config(config)?;
                Ok(Value::Null)
            }
            RuntimeRequest::LoadModelConfig => serde_json::to_value(runtime.load_model_config()?),
        }
        .map_err(Into::into)
    })();
    bridge_result(result)
}

pub fn send_message_json(handle: i64, request_json: &str, api_key: Option<String>) -> String {
    let result = (|| {
        let request: SendMessageRequest = serde_json::from_str(request_json)?;
        let runtime = runtime(handle)?;
        let runtime = runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime handle is unavailable"))?;
        serde_json::to_value(runtime.send_message(
            &request.session_id,
            &request.content,
            api_key,
        )?)
        .map_err(Into::into)
    })();
    bridge_result(result)
}

pub fn close_runtime(handle: i64) -> String {
    let result = (|| {
        let removed = runtimes()
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime registry is unavailable"))?
            .remove(&handle);
        if removed.is_none() {
            anyhow::bail!("runtime handle not found");
        }
        Ok(Value::Null)
    })();
    bridge_result(result)
}

fn runtime(handle: i64) -> anyhow::Result<Arc<Mutex<MobileRuntime>>> {
    runtimes()
        .lock()
        .map_err(|_| anyhow::anyhow!("runtime registry is unavailable"))?
        .get(&handle)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("runtime handle not found"))
}

fn runtimes() -> &'static Mutex<HashMap<i64, Arc<Mutex<MobileRuntime>>>> {
    RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn bridge_result(result: anyhow::Result<Value>) -> String {
    match result {
        Ok(data) => json!({"ok": true, "data": data}).to_string(),
        Err(error) => json!({
            "ok": false,
            "error": BridgeError {
                code: "runtime_error",
                message: error.to_string(),
            }
        })
        .to_string(),
    }
}
