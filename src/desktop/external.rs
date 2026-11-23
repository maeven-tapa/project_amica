use std::{cell::RefCell, collections::HashMap, mem::size_of, time::Duration};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Timer};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, SetLastError},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GA_ROOT, GW_OWNER, GWL_STYLE, GetAncestor, GetClassNameW, GetDesktopWindow,
        GetForegroundWindow, GetShellWindow, GetWindow, GetWindowLongPtrW, GetWindowPlacement,
        GetWindowTextW, GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOPMOST, IsWindow,
        IsWindowVisible, PostMessageW, SW_MINIMIZE, SW_RESTORE, SWP_ASYNCWINDOWPOS,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SetForegroundWindow, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, ShowWindowAsync,
        WINDOWPLACEMENT, WM_CLOSE, WS_OVERLAPPEDWINDOW,
    },
};

use crate::AmicaWindow;

use super::{ExternalAction, TargetWindow};

const WINDOW_ERROR: &str = "Windows did not allow Amica to change that window.";

#[derive(Clone, Copy)]
struct WindowFrame {
    style: isize,
    placement: WINDOWPLACEMENT,
}

thread_local! {
    static FULLSCREEN: RefCell<HashMap<TargetWindow, WindowFrame>> = RefCell::default();
}

pub(crate) fn window_handle(window: &AmicaWindow) -> Option<HWND> {
    let handle = window.window().window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(raw) => Some(raw.hwnd.get() as HWND),
        _ => None,
    }
}

pub(super) fn show_self(window: &AmicaWindow) {
    let Some(hwnd) = window_handle(window) else {
        return;
    };
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        SetForegroundWindow(hwnd);
    }

    let weak = window.as_weak();
    Timer::single_shot(Duration::from_millis(1200), move || {
        if let Some(window) = weak.upgrade()
            && let Some(hwnd) = window_handle(&window)
        {
            unsafe {
                SetWindowPos(
                    hwnd,
                    HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            };
        }
    });
}

pub(super) fn find(name: &str) -> Result<TargetWindow, &'static str> {
    let foreground = unsafe { GetAncestor(GetForegroundWindow(), GA_ROOT) };
    let name = name.trim().to_lowercase();
    if name == "current" {
        return target(foreground).ok_or("Switch to the app you want to control first.");
    }
    if name.is_empty() {
        return Err("Name the app or say current app.");
    }

    let mut results = Search {
        name: &name,
        matches: Vec::new(),
    };
    unsafe { EnumWindows(Some(find_window), &mut results as *mut Search<'_> as LPARAM) };
    if let Some(current) = results
        .matches
        .iter()
        .find(|entry| entry.handle == foreground as usize)
    {
        return Ok(*current);
    }
    match results.matches.as_slice() {
        [window] => Ok(*window),
        [] => Err("Amica could not find an open window for that app."),
        _ => {
            Err("More than one window matches. Switch to the one you want or use its window title.")
        }
    }
}

struct Search<'a> {
    name: &'a str,
    matches: Vec<TargetWindow>,
}

unsafe extern "system" fn find_window(hwnd: HWND, context: LPARAM) -> i32 {
    let search = unsafe { &mut *(context as *mut Search<'_>) };
    if let Some(window) = target(hwnd) {
        let mut title = [0u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
        if len > 0 {
            let title = String::from_utf16_lossy(&title[..len as usize]).to_lowercase();
            let executable = process_name(window.process);
            if title.contains(search.name)
                || executable == search.name
                || (search.name == "file explorer" && executable == "explorer")
                || (search.name == "edge" && executable == "msedge")
            {
                search.matches.push(window);
            }
        }
    }
    1
}

fn target(hwnd: HWND) -> Option<TargetWindow> {
    unsafe {
        if hwnd.is_null()
            || hwnd == GetDesktopWindow()
            || hwnd == GetShellWindow()
            || IsWindowVisible(hwnd) == 0
            || !GetWindow(hwnd, GW_OWNER).is_null()
        {
            return None;
        }
    }
    let mut class = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    if len <= 0
        || matches!(
            String::from_utf16_lossy(&class[..len as usize]).as_str(),
            "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW"
        )
    {
        return None;
    }
    let process = window_process(hwnd);
    (process != 0 && process != std::process::id()).then_some(TargetWindow {
        handle: hwnd as usize,
        process,
    })
}

fn window_process(hwnd: HWND) -> u32 {
    let mut process = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process) };
    process
}

pub(super) fn process_name(pid: u32) -> String {
    process_path(pid)
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_lowercase())
        })
        .unwrap_or_default()
}

pub(super) fn process_path(pid: u32) -> Option<std::path::PathBuf> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut text = [0u16; 512];
    let mut len = text.len() as u32;
    let result = unsafe { QueryFullProcessImageNameW(handle, 0, text.as_mut_ptr(), &mut len) };
    unsafe { CloseHandle(handle) };
    if result == 0 {
        return None;
    }
    use std::os::windows::ffi::OsStringExt;
    Some(std::ffi::OsString::from_wide(&text[..len as usize]).into())
}

pub(super) fn alive(window: TargetWindow) -> bool {
    let hwnd = window.handle as HWND;
    (unsafe { IsWindow(hwnd) != 0 }) && window.process == window_process(hwnd)
}

pub(super) fn apply(window: TargetWindow, action: ExternalAction) -> Result<(), &'static str> {
    if !alive(window) {
        return Err("That app window has already closed.");
    }
    let hwnd = window.handle as HWND;
    match action {
        ExternalAction::Fullscreen => fullscreen(window),
        ExternalAction::Restore => match restore(window)? {
            true => Ok(()),
            false => show_async(hwnd, SW_RESTORE),
        },
        ExternalAction::Minimize => show_async(hwnd, SW_MINIMIZE),
        ExternalAction::Maximize => show_async(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::SW_MAXIMIZE,
        ),
        ExternalAction::Close => {
            restore(window)?;
            (unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } != 0)
                .then_some(())
                .ok_or(WINDOW_ERROR)
        }
    }
}

fn show_async(hwnd: HWND, command: i32) -> Result<(), &'static str> {
    (unsafe { ShowWindowAsync(hwnd, command) } != 0)
        .then_some(())
        .ok_or(WINDOW_ERROR)
}

fn fullscreen(window: TargetWindow) -> Result<(), &'static str> {
    FULLSCREEN.with_borrow_mut(|frames| {
        frames.retain(|window, _| alive(*window));
        let hwnd = window.handle as HWND;
        if frames.contains_key(&window) {
            return show_async(hwnd, SW_RESTORE);
        }
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if style == 0
            || unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0
            || unsafe {
                GetMonitorInfoW(
                    MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                )
            } == 0
        {
            return Err(WINDOW_ERROR);
        }
        let bounds = monitor.rcMonitor;
        let frame = WindowFrame { style, placement };
        show_async(hwnd, SW_RESTORE)?;
        if !set_style(hwnd, style & !(WS_OVERLAPPEDWINDOW as isize)) {
            return Err(WINDOW_ERROR);
        }
        if unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        } == 0
        {
            let _ = restore_frame(hwnd, frame);
            return Err(WINDOW_ERROR);
        }
        frames.insert(window, frame);
        Ok(())
    })
}

fn set_style(hwnd: HWND, style: isize) -> bool {
    unsafe {
        SetLastError(0);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style) != 0
            || windows_sys::Win32::Foundation::GetLastError() == 0
    }
}

fn restore(window: TargetWindow) -> Result<bool, &'static str> {
    FULLSCREEN.with_borrow_mut(|frames| {
        let Some(&frame) = frames.get(&window) else {
            return Ok(false);
        };
        restore_frame(window.handle as HWND, frame)?;
        frames.remove(&window);
        Ok(true)
    })
}

fn restore_frame(hwnd: HWND, frame: WindowFrame) -> Result<(), &'static str> {
    if !set_style(hwnd, frame.style)
        || unsafe { SetWindowPlacement(hwnd, &frame.placement) } == 0
        || unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED
                    | SWP_NOSIZE
                    | SWP_NOMOVE
                    | SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | SWP_ASYNCWINDOWPOS,
            )
        } == 0
    {
        return Err(WINDOW_ERROR);
    }
    Ok(())
}

pub(super) fn restore_all() {
    FULLSCREEN.with_borrow_mut(|frames| {
        for (window, frame) in frames.drain() {
            if alive(window) {
                let _ = restore_frame(window.handle as HWND, frame);
            }
        }
    });
}
