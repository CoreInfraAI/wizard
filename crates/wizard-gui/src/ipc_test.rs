//! Exercise Rust command argument decoding and responses through Tauri's IPC dispatcher.
//! The test runtime creates no native windows and never starts application services.

use serde_json::Value;
use tauri::test::MockRuntime;

pub(crate) fn window(app: &tauri::App<MockRuntime>) -> tauri::WebviewWindow<MockRuntime> {
    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
        .build()
        .expect("test webview")
}

pub(crate) fn invoke(
    window: &tauri::WebviewWindow<MockRuntime>,
    command: &str,
    arguments: Value,
) -> Result<Value, Value> {
    tauri::test::get_ipc_response(
        window,
        tauri::webview::InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .expect("local test origin"),
            body: tauri::ipc::InvokeBody::Json(arguments),
            headers: tauri::http::HeaderMap::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        },
    )
    .map(|body| body.deserialize().expect("JSON command response"))
}
