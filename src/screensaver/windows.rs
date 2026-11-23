use std::mem::size_of;

use windows_sys::Win32::{
    System::{
        Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
        SystemInformation::GetTickCount,
    },
    UI::{
        Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
        Shell::{
            QUNS_BUSY, QUNS_NOT_PRESENT, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
            SHQueryUserNotificationState,
        },
        WindowsAndMessaging::SetForegroundWindow,
    },
};

pub(super) fn raise(window: &crate::AmicaWindow) {
    if let Some(hwnd) = crate::desktop::window_handle(window) {
        unsafe { SetForegroundWindow(hwnd) };
    }
}

pub fn last_input() -> Option<(u32, u32)> {
    let mut input = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    (unsafe { GetLastInputInfo(&mut input) } != 0).then(|| {
        (
            input.dwTime,
            unsafe { GetTickCount() }.wrapping_sub(input.dwTime),
        )
    })
}

pub fn blocked() -> bool {
    let mut state = 0;
    (unsafe { SHQueryUserNotificationState(&mut state) }) < 0
        || matches!(
            state,
            QUNS_BUSY | QUNS_NOT_PRESENT | QUNS_PRESENTATION_MODE | QUNS_RUNNING_D3D_FULL_SCREEN
        )
}

pub fn on_battery() -> bool {
    let mut power = SYSTEM_POWER_STATUS::default();
    (unsafe { GetSystemPowerStatus(&mut power) }) != 0 && power.ACLineStatus == 0
}
