use std::{
    collections::{BTreeMap, VecDeque},
    ffi::c_void,
    mem::size_of,
    ptr::{self, NonNull},
    slice,
    sync::{Mutex, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use windows_sys::Win32::{
    Foundation::FILETIME,
    NetworkManagement::{
        IpHelper::{
            FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_ROW2, MIB_IF_TABLE2,
        },
        Ndis::IfOperStatusUp,
    },
    System::{
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::{GetCurrentProcess, GetProcessTimes, GetSystemTimes},
    },
};

use super::{RECEIVED, SENT, Scope};

const SAMPLE_STEP: Duration = Duration::from_secs(1);
const SAMPLE_STEPS: usize = 3;
const MIB: f64 = 1_048_576.0;
const GIB: f64 = 1_073_741_824.0;
static LATEST: Mutex<Option<Sample>> = Mutex::new(None);

#[derive(Clone, Copy, Default)]
struct Traffic {
    received: u64,
    sent: u64,
}

impl Traffic {
    fn delta(self, baseline: Self) -> Option<Self> {
        Some(Self {
            received: self.received.checked_sub(baseline.received)?,
            sent: self.sent.checked_sub(baseline.sent)?,
        })
    }

    fn rates(self, seconds: f64) -> (f64, f64) {
        let to_mbps = |bytes| round(bytes as f64 * 8.0 / (seconds * 1_000_000.0), 1_000.0);
        (to_mbps(self.received), to_mbps(self.sent))
    }
}

#[derive(Clone, Copy)]
struct Cpu {
    total: u64,
    idle: u64,
}

type Interfaces = BTreeMap<u32, Traffic>;

struct Snapshot {
    at: Instant,
    system_cpu: Option<Cpu>,
    process_cpu: Option<u64>,
    physical_memory: Option<(u64, u64)>,
    process_memory: Option<u64>,
    adapters: Option<Interfaces>,
    gemini: Traffic,
}

impl Snapshot {
    fn take() -> Self {
        Self {
            at: Instant::now(),
            system_cpu: system_cpu(),
            process_cpu: process_cpu(),
            physical_memory: physical_memory(),
            process_memory: process_memory(),
            adapters: interfaces(),
            gemini: Traffic {
                received: RECEIVED.load(Ordering::Relaxed),
                sent: SENT.load(Ordering::Relaxed),
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Sample {
    at: Instant,
    seconds: f64,
    system_cpu: Option<f64>,
    process_cpu: Option<f64>,
    physical_memory: Option<(u64, u64)>,
    process_memory: Option<u64>,
    network: Option<(f64, f64)>,
    gemini: Option<(f64, f64)>,
}

impl Sample {
    fn measure(before: &Snapshot, after: &Snapshot) -> Self {
        let seconds = after.at.duration_since(before.at).as_secs_f64();
        let cpu = before
            .system_cpu
            .zip(after.system_cpu)
            .and_then(|(before, after)| {
                let total = after.total.checked_sub(before.total)?;
                let idle = after.idle.checked_sub(before.idle)?;
                Some((total, idle))
            });
        let system_cpu = cpu.and_then(|(total, idle)| percent(total.checked_sub(idle)?, total));
        let process_cpu = cpu.and_then(|(total, _)| {
            percent(after.process_cpu?.checked_sub(before.process_cpu?)?, total)
        });
        let network = before
            .adapters
            .as_ref()
            .zip(after.adapters.as_ref())
            .and_then(|(before, after)| adapter_delta(before, after))
            .map(|traffic| traffic.rates(seconds));
        let gemini = after
            .gemini
            .delta(before.gemini)
            .map(|traffic| traffic.rates(seconds));
        Self {
            at: after.at,
            seconds,
            system_cpu,
            process_cpu,
            physical_memory: after.physical_memory,
            process_memory: after.process_memory,
            network,
            gemini,
        }
    }

    fn report(self, scope: Scope) -> Result<Value, &'static str> {
        let memory_total = self.physical_memory.map(|(total, _)| total);
        let (name, cpu, memory, network, network_scope) = match scope {
            Scope::Computer => (
                "whole_pc",
                self.system_cpu,
                self.physical_memory.map(|(_, used)| used),
                self.network,
                "Traffic crossing the PC's active network adapters, including other apps and embedded media. It can include local network traffic.",
            ),
            Scope::Amica => (
                "amica_process",
                self.process_cpu,
                self.process_memory,
                self.gemini,
                "Amica's Gemini chat, Live voice, and Google Search request and response payloads. This estimate excludes embedded media traffic and protocol overhead.",
            ),
        };
        if cpu.is_none() && memory.is_none() && network.is_none() {
            return Err("Amica could not read usage counters from Windows.");
        }
        Ok(json!({
            "ok": true,
            "scope": name,
            "sample_seconds": round(self.seconds, 10.0),
            "cpu_percent_of_pc_capacity": cpu,
            "memory_percent_of_pc_ram": memory.zip(memory_total).and_then(|(used, total)| percent(used, total)),
            "memory_used_mib": memory.map(|bytes| round(bytes as f64 / MIB, 10.0)),
            "pc_ram_total_gib": memory_total.map(|bytes| round(bytes as f64 / GIB, 10.0)),
            "download_mbps": network.map(|(download, _)| download),
            "upload_mbps": network.map(|(_, upload)| upload),
            "total_mbps": network.map(|(download, upload)| round(download + upload, 1_000.0)),
            "network_scope": network_scope,
        }))
    }
}

pub(super) fn start() {
    let _ = thread::Builder::new().name("amica-usage".into()).spawn(|| {
        let mut history = VecDeque::from([Snapshot::take()]);
        loop {
            thread::sleep(SAMPLE_STEP);
            let current = Snapshot::take();
            let oldest = history.front().unwrap();
            if current.at.duration_since(oldest.at) > SAMPLE_STEP * 5 {
                history.clear();
            } else {
                let sample = Sample::measure(oldest, &current);
                *LATEST.lock().unwrap() = Some(sample);
                if history.len() >= SAMPLE_STEPS {
                    history.pop_front();
                }
            }
            history.push_back(current);
        }
    });
}

pub(super) fn read(scope: Scope) -> Result<Value, &'static str> {
    let cached = *LATEST.lock().unwrap();
    if let Some(sample) = cached
        && sample.at.elapsed() <= SAMPLE_STEP * 3
    {
        return sample.report(scope);
    }
    let baseline = Snapshot::take();
    thread::sleep(SAMPLE_STEP);
    Sample::measure(&baseline, &Snapshot::take()).report(scope)
}

fn round(number: f64, places: f64) -> f64 {
    (number * places).round() / places
}

fn percent(used: u64, total: u64) -> Option<f64> {
    (total != 0).then(|| round((used as f64 * 100.0 / total as f64).min(100.0), 100.0))
}

fn file_time(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

fn system_cpu() -> Option<Cpu> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    (unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0).then(|| Cpu {
        idle: file_time(idle),
        total: file_time(kernel) + file_time(user),
    })
}

fn process_cpu() -> Option<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    (unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } != 0)
        .then(|| file_time(kernel) + file_time(user))
}

fn physical_memory() -> Option<(u64, u64)> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then(|| {
        (
            status.ullTotalPhys,
            status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
        )
    })
}

fn process_memory() -> Option<u64> {
    let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let mut memory = PROCESS_MEMORY_COUNTERS {
        cb: size,
        ..Default::default()
    };
    (unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut memory, size) } != 0)
        .then_some(memory.WorkingSetSize as u64)
}

const HARDWARE_INTERFACE: u8 = 1;
const FILTER_INTERFACE: u8 = 2;

struct InterfaceTable(NonNull<MIB_IF_TABLE2>);

impl Drop for InterfaceTable {
    fn drop(&mut self) {
        unsafe { FreeMibTable(self.0.as_ptr().cast::<c_void>()) };
    }
}

fn interfaces() -> Option<Interfaces> {
    let mut table = ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) } != 0 {
        return None;
    }
    let table = InterfaceTable(NonNull::new(table)?);
    let table_ptr = table.0.as_ptr();
    let rows: &[MIB_IF_ROW2] = unsafe {
        let count = (*table_ptr).NumEntries as usize;
        slice::from_raw_parts(ptr::addr_of!((*table_ptr).Table).cast(), count)
    };
    let active = rows.iter().filter(|row| {
        row.OperStatus == IfOperStatusUp
            && row.Type != IF_TYPE_SOFTWARE_LOOPBACK
            && row.InterfaceAndOperStatusFlags._bitfield & FILTER_INTERFACE == 0
    });
    let physical = active
        .clone()
        .any(|row| row.InterfaceAndOperStatusFlags._bitfield & HARDWARE_INTERFACE != 0);
    Some(
        active
            .filter(|row| {
                !physical || row.InterfaceAndOperStatusFlags._bitfield & HARDWARE_INTERFACE != 0
            })
            .map(|row| {
                (
                    row.InterfaceIndex,
                    Traffic {
                        received: row.InOctets,
                        sent: row.OutOctets,
                    },
                )
            })
            .collect(),
    )
}

fn adapter_delta(before: &Interfaces, after: &Interfaces) -> Option<Traffic> {
    if before.is_empty() && after.is_empty() {
        return Some(Traffic::default());
    }
    let mut changes = after
        .iter()
        .filter_map(|(id, counter)| counter.delta(*before.get(id)?));
    let measured = changes.next()?;
    Some(changes.fold(measured, |total, change| Traffic {
        received: total.received.saturating_add(change.received),
        sent: total.sent.saturating_add(change.sent),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_preserve_upload_and_download_and_ignore_reset_adapters() {
        let old = Traffic {
            received: 150,
            sent: 250,
        };
        let new = Traffic {
            received: 3_000_150,
            sent: 250_250,
        };
        let before = Interfaces::from([(3, old), (5, new)]);
        let after = Interfaces::from([(1, new), (3, new), (5, old)]);
        assert_eq!(
            adapter_delta(&before, &after).unwrap().rates(2.0),
            (12.0, 1.0)
        );
        assert!(adapter_delta(&after, &Interfaces::new()).is_none());
    }

    #[test]
    fn windows_reports_cpu_ram_and_gemini_traffic_with_distinct_scopes() {
        let before = Snapshot::take();
        super::super::received(200_000);
        super::super::sent(50_000);
        thread::sleep(Duration::from_millis(150));
        let sample = Sample::measure(&before, &Snapshot::take());
        let pc = sample.report(Scope::Computer).unwrap();
        let amica = sample.report(Scope::Amica).unwrap();
        assert_eq!(pc["scope"], "whole_pc");
        assert_eq!(amica["scope"], "amica_process");
        assert!(pc["cpu_percent_of_pc_capacity"].is_number());
        assert!(pc["download_mbps"].is_number());
        assert!(amica["memory_used_mib"].as_f64().unwrap() > 0.0);
        assert!(amica["download_mbps"].as_f64().unwrap() > amica["upload_mbps"].as_f64().unwrap());
    }
}
