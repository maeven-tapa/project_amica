#[cfg(windows)]
mod windows;

use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::blocking::{RequestBuilder, Response};
use serde_json::Value;

#[derive(Clone, Copy)]
pub enum Scope {
    Computer,
    Amica,
}

impl Scope {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "computer" => Some(Self::Computer),
            "amica" => Some(Self::Amica),
            _ => None,
        }
    }
}

static RECEIVED: AtomicU64 = AtomicU64::new(0);
static SENT: AtomicU64 = AtomicU64::new(0);

pub fn received(bytes: usize) {
    RECEIVED.fetch_add(bytes as u64, Ordering::Relaxed);
}

pub fn sent(bytes: usize) {
    SENT.fetch_add(bytes as u64, Ordering::Relaxed);
}

pub fn send_json(request: RequestBuilder, body: &Value) -> reqwest::Result<Response> {
    let payload = body.to_string();
    let bytes = payload.len();
    let response = request
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload)
        .send()?;
    sent(bytes);
    Ok(response)
}

pub fn read_json(response: Response) -> Option<Value> {
    let payload = response.bytes().ok()?;
    received(payload.len());
    serde_json::from_slice(&payload).ok()
}

pub fn start() {
    #[cfg(windows)]
    windows::start();
}

pub fn read(scope: Scope) -> Result<Value, &'static str> {
    #[cfg(windows)]
    {
        windows::read(scope)
    }
    #[cfg(not(windows))]
    {
        let _ = scope;
        Err("Live system usage is available on Windows.")
    }
}
