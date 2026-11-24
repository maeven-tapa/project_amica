use std::{env, fs, thread};

use super::Spec;

pub(super) fn scan() -> Result<Vec<Spec>, &'static str> {
    let board = fs::read_to_string("/proc/device-tree/model")
        .or_else(|_| fs::read_to_string("/sys/class/dmi/id/product_name"))
        .ok();
    let distro = fs::read_to_string("/etc/os-release").ok();
    let os = distro
        .as_deref()
        .and_then(|release| {
            release
                .lines()
                .find_map(|line| line.strip_prefix("PRETTY_NAME="))
                .map(|name| name.trim_matches('"'))
        })
        .unwrap_or("Linux");
    let arch = env::consts::ARCH;
    let cpu = thread::available_parallelism().map_or_else(
        |_| arch.into(),
        |count| format!("{arch} · {count} logical CPUs"),
    );
    let mut specs = Vec::new();
    if let Some(board) = board {
        let model = board.trim_end_matches('\0').trim();
        if !model.is_empty() {
            specs.push(Spec {
                label: "Computer",
                value: model.into(),
            });
        }
    }
    specs.push(Spec {
        label: "Operating system",
        value: os.into(),
    });
    specs.push(Spec {
        label: "Processor",
        value: cpu,
    });
    if let Some((total, _)) = crate::usage::physical_memory() {
        specs.push(Spec {
            label: "RAM",
            value: format!("{:.1} GiB", total as f64 / 1_073_741_824.0),
        });
    }
    Ok(specs)
}
