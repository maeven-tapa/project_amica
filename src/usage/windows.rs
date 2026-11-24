use std::{
    ffi::c_void,
    mem::size_of,
    ptr::{self, NonNull},
    slice,
};

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

use super::sample::{Cpu, Interfaces, Traffic};

fn file_time(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

pub(super) fn system_cpu() -> Option<Cpu> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    (unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0).then(|| Cpu {
        idle: file_time(idle),
        total: file_time(kernel) + file_time(user),
    })
}

pub(super) fn process_cpu() -> Option<u64> {
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

pub(super) fn physical_memory() -> Option<(u64, u64)> {
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

pub(super) fn process_memory() -> Option<u64> {
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

pub(super) fn interfaces() -> Option<Interfaces> {
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
