use std::mem::size_of;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, RECT},
    Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            GetWindowRect, IsZoomed, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
            ShowWindow,
        },
    },
};

use crate::actions::command::{Landmark, WindowControl};

use super::{TargetWindow, external, input};

pub fn is_browser(target: TargetWindow) -> bool {
    matches!(
        external::process_name(target.process).as_str(),
        "msedge" | "chrome" | "firefox" | "brave" | "opera" | "vivaldi"
    )
}

pub fn is_chromium(target: TargetWindow) -> bool {
    matches!(
        external::process_name(target.process).as_str(),
        "msedge" | "chrome" | "brave" | "opera" | "vivaldi"
    )
}

pub fn apply(target: TargetWindow, action: WindowControl) -> Result<String, String> {
    let hwnd = target.handle as HWND;
    let mut rect = RECT::default();
    if !external::alive(target) || unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err("The target window is no longer available.".into());
    }
    let area = input::work_area(target)?;
    let mut result = rect;
    match action {
        WindowControl::Resize(width, height) => {
            if !(200..=4000).contains(&width) || !(120..=3000).contains(&height) {
                return Err("Choose a supported window size.".into());
            }
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
            result.right = rect.left + (u32::from(width) * dpi / 96) as i32;
            result.bottom = rect.top + (u32::from(height) * dpi / 96) as i32;
        }
        WindowControl::Display(number) => {
            let displays = monitors();
            let index = usize::from(number.checked_sub(1).ok_or("Display numbers start at 1.")?);
            let display = displays.get(index).ok_or("That display is unavailable.")?;
            let target_width = (rect.right - rect.left).min(display.right - display.left);
            let target_height = (rect.bottom - rect.top).min(display.bottom - display.top);
            result.left = display.left
                + (rect.left - area.left).clamp(0, display.right - display.left - target_width);
            result.top = display.top
                + (rect.top - area.top).clamp(0, display.bottom - display.top - target_height);
            result.right = result.left + target_width;
            result.bottom = result.top + target_height;
        }
        WindowControl::Snap(region) => {
            let x = (area.left + area.right) / 2;
            let y = (area.top + area.bottom) / 2;
            result = area;
            let third = (area.right - area.left) / 3;
            match region {
                Landmark::LeftThird => result.right = area.left + third,
                Landmark::MiddleThird => {
                    result.left = area.left + third;
                    result.right = area.right - third;
                }
                Landmark::RightThird => result.left = area.right - third,
                Landmark::LeftTwoThirds => result.right = area.right - third,
                _ => (),
            }
            if matches!(
                region,
                Landmark::Left | Landmark::TopLeft | Landmark::BottomLeft
            ) {
                result.right = x;
            }
            if matches!(
                region,
                Landmark::Right | Landmark::TopRight | Landmark::BottomRight
            ) {
                result.left = x;
            }
            if matches!(
                region,
                Landmark::Top | Landmark::TopLeft | Landmark::TopRight
            ) {
                result.bottom = y;
            }
            if matches!(
                region,
                Landmark::Bottom | Landmark::BottomLeft | Landmark::BottomRight
            ) {
                result.top = y;
            }
        }
        _ => return Err("No placement operation was requested.".into()),
    }
    place(
        target,
        result.left,
        result.top,
        result.right - result.left,
        result.bottom - result.top,
    )?;
    Ok("Updated the selected window placement.".into())
}

pub fn place(
    target: TargetWindow,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), &'static str> {
    if !external::alive(target) {
        return Err("The selected window closed.");
    }
    let hwnd = target.handle as HWND;
    unsafe {
        if IsZoomed(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        (SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        ) != 0)
            .then_some(())
            .ok_or("Windows did not accept that placement.")
    }
}

pub(super) fn monitors() -> Vec<RECT> {
    let mut screens: Vec<(bool, RECT)> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut screens as *mut Vec<(bool, RECT)> as LPARAM,
        )
    };
    screens.sort_by_key(|(primary, rect)| (!primary, rect.left, rect.top));
    screens.into_iter().map(|(_, rect)| rect).collect()
}

unsafe extern "system" fn collect(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    parameter: LPARAM,
) -> i32 {
    let screens = unsafe { &mut *(parameter as *mut Vec<(bool, RECT)>) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        screens.push((info.dwFlags & 1 != 0, info.rcWork));
    }
    1
}

pub fn snapshot() -> Vec<crate::routines::Window> {
    ["notepad", "calculator", "file explorer", "paint"]
        .into_iter()
        .filter_map(|app| {
            let target = TargetWindow::find(app).ok()?;
            let mut rect = RECT::default();
            (unsafe { GetWindowRect(target.handle as HWND, &mut rect) } != 0).then(|| {
                crate::routines::Window {
                    app: app.into(),
                    bounds: [
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                    ],
                }
            })
        })
        .collect()
}

pub fn restore_saved(saved: &crate::routines::Window) -> Result<(), &'static str> {
    let target = match TargetWindow::find(&saved.app) {
        Ok(target) => target,
        Err(_) => {
            let app =
                crate::actions::catalog::app(&saved.app).ok_or("A saved app is unavailable.")?;
            super::open_app(app)?;
            return Err(
                "Opened the app. Once its window is ready, run the layout again to place it.",
            );
        }
    };
    let [mut x, mut y, width, height] = saved.bounds;
    if !monitors()
        .iter()
        .any(|area| (area.left..area.right).contains(&x) && (area.top..area.bottom).contains(&y))
    {
        let area = input::work_area(target)?;
        (x, y) = (area.left, area.top);
    }
    place(target, x, y, width, height)
}
