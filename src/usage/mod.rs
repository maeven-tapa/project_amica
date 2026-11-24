#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(any(windows, target_os = "linux"))]
mod sample;

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
    #[cfg(any(windows, target_os = "linux"))]
    sample::start();
}

pub fn read(scope: Scope) -> Result<Value, &'static str> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        sample::read(scope)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = scope;
        Err("Live system usage is unavailable on this platform.")
    }
}

#[cfg(target_os = "linux")]
pub(crate) use platform::physical_memory;

pub(crate) fn metric(name: &str) -> Result<String, String> {
    let (label, key, unit) = match name {
        "cpu usage" | "gamit ng cpu" => ("CPU usage", "cpu_percent_of_pc_capacity", "%"),
        "memory usage" | "ram usage" | "gamit ng memory" => {
            ("RAM usage", "memory_percent_of_pc_ram", "%")
        }
        "download rate" | "bilis ng download" => ("Download rate", "download_mbps", " Mbps"),
        "upload rate" | "bilis ng upload" => ("Upload rate", "upload_mbps", " Mbps"),
        _ => return Err("The requested metric is unavailable.".into()),
    };
    let report = read(Scope::Computer)?;
    let value = report[key]
        .as_f64()
        .ok_or("That measurement is unavailable.")?;
    Ok(format!("{label}: {value:.1}{unit}."))
}
