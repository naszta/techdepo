//! Thin wrappers over browser APIs: `localStorage`, `crypto.randomUUID()` and the document
//! element. Every function degrades to a no-op when the API is unavailable (private mode,
//! tests), so callers never need to care.

use serde::Serialize;
use serde::de::DeserializeOwned;

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The raw string stored under `key`.
pub fn load_string(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

/// Stores `value` under `key`.
pub fn save_string(key: &str, value: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(key, value);
    }
}

/// Removes `key`.
pub fn remove(key: &str) {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(key);
    }
}

/// The JSON value stored under `key`; `None` when absent or unreadable.
pub fn load_json<T: DeserializeOwned>(key: &str) -> Option<T> {
    serde_json::from_str(&load_string(key)?).ok()
}

/// Stores `value` as JSON under `key`.
pub fn save_json<T: Serialize>(key: &str, value: &T) {
    if let Ok(json) = serde_json::to_string(value) {
        save_string(key, &json);
    }
}

/// A random UUID for idempotency keys (one per form load).
pub fn uuid() -> String {
    web_sys::window()
        .and_then(|w| w.crypto().ok())
        .map(|c| c.random_uuid())
        .unwrap_or_else(|| {
            // Fallback for non-secure contexts: 32 hex digits from Math.random().
            (0..4)
                .map(|_| {
                    format!(
                        "{:08x}",
                        (js_sys::Math::random() * f64::from(u32::MAX)) as u32
                    )
                })
                .collect()
        })
}

/// Sets (or with `None`, removes) an attribute on `<html>`.
pub fn set_root_attribute(name: &str, value: Option<&str>) {
    let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    else {
        return;
    };
    let _ = match value {
        Some(value) => root.set_attribute(name, value),
        None => root.remove_attribute(name),
    };
}

/// Whether the OS prefers a dark colour scheme.
pub fn prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Scrolls the window to the top (after a route change that keeps the layout).
pub fn scroll_to_top() {
    if let Some(window) = web_sys::window() {
        window.scroll_to_with_x_and_y(0.0, 0.0);
    }
}

/// Waits `ms` milliseconds (polling after a mutation whose effects land asynchronously).
pub async fn sleep(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}
