use jni::JNIEnv;
use jni::objects::{JObject, JString};
use jni::sys::{jlong, jstring};
use serde_json::json;

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_generalagent_mobile_runtime_NativeRuntime_nativeInitialize(
    mut env: JNIEnv,
    _receiver: JObject,
    request_json: JString,
) -> jstring {
    let output = java_string(&mut env, &request_json)
        .map(|request| crate::initialize_runtime_json(&request))
        .unwrap_or_else(jni_error_json);
    new_java_string(&mut env, output)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_generalagent_mobile_runtime_NativeRuntime_nativeInvoke(
    mut env: JNIEnv,
    _receiver: JObject,
    handle: jlong,
    request_json: JString,
) -> jstring {
    let output = java_string(&mut env, &request_json)
        .map(|request| crate::invoke_runtime_json(handle, &request))
        .unwrap_or_else(jni_error_json);
    new_java_string(&mut env, output)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_generalagent_mobile_runtime_NativeRuntime_nativeSendMessage(
    mut env: JNIEnv,
    _receiver: JObject,
    handle: jlong,
    request_json: JString,
    api_key: JString,
) -> jstring {
    let output = (|| {
        let request = java_string(&mut env, &request_json)?;
        let api_key = if api_key.is_null() {
            None
        } else {
            Some(java_string(&mut env, &api_key)?)
        };
        Ok(crate::send_message_json(handle, &request, api_key))
    })()
    .unwrap_or_else(jni_error_json);
    new_java_string(&mut env, output)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_generalagent_mobile_runtime_NativeRuntime_nativeClose(
    mut env: JNIEnv,
    _receiver: JObject,
    handle: jlong,
) -> jstring {
    new_java_string(&mut env, crate::close_runtime(handle))
}

fn java_string(env: &mut JNIEnv, value: &JString) -> anyhow::Result<String> {
    Ok(env.get_string(value)?.into())
}

fn new_java_string(env: &mut JNIEnv, value: String) -> jstring {
    env.new_string(value)
        .map(|value| value.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn jni_error_json(error: anyhow::Error) -> String {
    json!({
        "ok": false,
        "error": {
            "code": "jni_error",
            "message": error.to_string(),
        }
    })
    .to_string()
}
