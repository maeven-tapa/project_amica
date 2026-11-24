use std::{fs, path::Path};

use super::sample::{Cpu, Interfaces, Traffic};

pub(super) fn system_cpu() -> Option<Cpu> {
    let contents = fs::read_to_string("/proc/stat").ok()?;
    let aggregate = contents.lines().next()?.strip_prefix("cpu ")?;
    let mut total = 0;
    let mut idle = 0;
    for (index, count) in aggregate.split_whitespace().take(8).enumerate() {
        let count: u64 = count.parse().ok()?;
        total += count;
        if matches!(index, 3 | 4) {
            idle += count;
        }
    }
    Some(Cpu { total, idle })
}

pub(super) fn process_cpu() -> Option<u64> {
    let contents = fs::read_to_string("/proc/self/stat").ok()?;
    let (_, after_name) = contents.rsplit_once(')')?;
    let mut counts = after_name.split_whitespace().skip(11);
    let user: u64 = counts.next()?.parse().ok()?;
    let system: u64 = counts.next()?.parse().ok()?;
    Some(user + system)
}

pub(crate) fn physical_memory() -> Option<(u64, u64)> {
    let memory = fs::read_to_string("/proc/meminfo").ok()?;
    let total = memory_bytes(&memory, "MemTotal:")?;
    let available = memory_bytes(&memory, "MemAvailable:")?;
    Some((total, total.saturating_sub(available)))
}

pub(super) fn process_memory() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    memory_bytes(&status, "VmRSS:")
}

fn memory_bytes(contents: &str, key: &str) -> Option<u64> {
    let count: u64 = contents
        .lines()
        .find_map(|line| line.strip_prefix(key))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    count.checked_mul(1024)
}

pub(super) fn interfaces() -> Option<Interfaces> {
    fn number(path: &Path, file: &str) -> Option<u64> {
        fs::read_to_string(path.join(file))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    let mut physical = Interfaces::new();
    let mut virtual_devices = Interfaces::new();
    for entry in fs::read_dir("/sys/class/net").ok()?.filter_map(Result::ok) {
        let path = entry.path();
        if entry.file_name() == "lo"
            || !fs::read_to_string(path.join("operstate"))
                .is_ok_and(|state| matches!(state.trim(), "up" | "unknown"))
        {
            continue;
        }
        let Some(index) = number(&path, "ifindex").and_then(|value| value.try_into().ok()) else {
            continue;
        };
        let Some(received) = number(&path, "statistics/rx_bytes") else {
            continue;
        };
        let Some(sent) = number(&path, "statistics/tx_bytes") else {
            continue;
        };
        let devices = if path.join("device").exists() {
            &mut physical
        } else {
            &mut virtual_devices
        };
        devices.insert(index, Traffic { received, sent });
    }
    Some(if physical.is_empty() {
        virtual_devices
    } else {
        physical
    })
}
