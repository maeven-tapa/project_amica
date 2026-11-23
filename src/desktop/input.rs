use std::{mem::size_of, thread, time::Duration};

use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{
        ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    },
    UI::{
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::{
            GA_ROOT, GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow,
            GetSystemMetrics, IsIconic, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN, SW_RESTORE, SetCursorPos, SetForegroundWindow, ShowWindow,
            WindowFromPoint,
        },
    },
};

use crate::{
    actions::command::{Button, Direction, InputAction, Landmark},
    desktop::{TargetWindow, external},
};

const INPUT_ERROR: &str = "Windows blocked input. Check the target window and its privileges.";

pub fn activate(target: TargetWindow) -> Result<(), &'static str> {
    if !external::alive(target) {
        return Err("The selected window closed.");
    }
    let handle = target.handle as HWND;
    unsafe {
        if IsIconic(handle) != 0 {
            ShowWindow(handle, SW_RESTORE);
        }
        if GetForegroundWindow() != handle {
            SetForegroundWindow(handle);
        }
    }
    focused(target)
}

pub fn title(target: TargetWindow) -> Result<String, &'static str> {
    if !external::alive(target) {
        return Err("The selected window closed.");
    }
    let mut title = [0; 512];
    let length = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(
            target.handle as HWND,
            title.as_mut_ptr(),
            title.len() as i32,
        )
    }
    .max(0) as usize;
    let title = String::from_utf16_lossy(&title[..length]);
    if title.is_empty() {
        Err("The selected window has no visible title.")
    } else {
        Ok(title)
    }
}

pub fn focused(target: TargetWindow) -> Result<(), &'static str> {
    if external::alive(target)
        && unsafe { GetAncestor(GetForegroundWindow(), GA_ROOT) == target.handle as HWND }
    {
        Ok(())
    } else {
        Err("Windows did not focus the selected window. Bring it to the front and retry.")
    }
}

pub fn client(target: TargetWindow) -> Result<RECT, &'static str> {
    let handle = target.handle as HWND;
    let mut rect = RECT::default();
    let mut origin = POINT::default();
    if !external::alive(target)
        || unsafe { GetClientRect(handle, &mut rect) } == 0
        || unsafe { ClientToScreen(handle, &mut origin) } == 0
        || rect.right <= 0
        || rect.bottom <= 0
    {
        return Err("The target window has no visible client area.");
    }
    Ok(RECT {
        left: origin.x,
        top: origin.y,
        right: origin.x + rect.right,
        bottom: origin.y + rect.bottom,
    })
}

pub fn cursor() -> Result<POINT, &'static str> {
    let mut point = POINT::default();
    (unsafe { GetCursorPos(&mut point) } != 0)
        .then_some(point)
        .ok_or(INPUT_ERROR)
}

fn contains(rect: RECT, point: POINT) -> bool {
    (rect.left..rect.right).contains(&point.x) && (rect.top..rect.bottom).contains(&point.y)
}

fn at_target(target: TargetWindow, point: POINT) -> bool {
    unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) == target.handle as HWND }
}

fn position(target: TargetWindow, point: POINT) -> Result<(), &'static str> {
    focused(target)?;
    if !contains(client(target)?, point) || !at_target(target, point) {
        return Err("That point is outside the visible target window.");
    }
    (unsafe { SetCursorPos(point.x, point.y) } != 0)
        .then_some(())
        .ok_or(INPUT_ERROR)
}

pub fn cell(rect: RECT, columns: u8, rows: u8, number: u8) -> Option<POINT> {
    if columns == 0
        || rows == 0
        || number == 0
        || u16::from(number) > u16::from(columns) * u16::from(rows)
    {
        return None;
    }
    let index = i32::from(number - 1);
    let columns = i32::from(columns);
    let rows = i32::from(rows);
    Some(POINT {
        x: rect.left + (rect.right - rect.left) * (2 * (index % columns) + 1) / (2 * columns),
        y: rect.top + (rect.bottom - rect.top) * (2 * (index / columns) + 1) / (2 * rows),
    })
}

pub fn click_cell(
    target: TargetWindow,
    columns: u8,
    rows: u8,
    number: u8,
) -> Result<String, &'static str> {
    activate(target)?;
    let point = cell(client(target)?, columns, rows, number)
        .ok_or("Choose a numbered cell in the visible grid.")?;
    click(target, point, Button::Left)?;
    Ok(format!("Clicked grid cell {number}."))
}

fn landmark(rect: RECT, point: Landmark) -> POINT {
    let x = match point {
        Landmark::TopLeft | Landmark::Left | Landmark::BottomLeft => rect.left + 3,
        Landmark::TopRight | Landmark::Right | Landmark::BottomRight => rect.right - 4,
        _ => rect.left + (rect.right - rect.left) / 2,
    };
    let y = match point {
        Landmark::TopLeft | Landmark::Top | Landmark::TopRight => rect.top + 3,
        Landmark::BottomLeft | Landmark::Bottom | Landmark::BottomRight => rect.bottom - 4,
        _ => rect.top + (rect.bottom - rect.top) / 2,
    };
    POINT { x, y }
}

fn mouse(flags: u32, dx: i32, dy: i32, data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

fn key(code: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: code,
                wScan: scan,
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

fn send(target: TargetWindow, events: &[INPUT], release: &[INPUT]) -> Result<(), &'static str> {
    focused(target)?;
    let sent = unsafe {
        SendInput(
            events.len() as u32,
            events.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != events.len() as u32 {
        if sent > 0 && !release.is_empty() {
            unsafe {
                SendInput(
                    release.len() as u32,
                    release.as_ptr(),
                    size_of::<INPUT>() as i32,
                )
            };
        }
        return Err(INPUT_ERROR);
    }
    Ok(())
}

pub fn click(target: TargetWindow, point: POINT, button: Button) -> Result<(), &'static str> {
    position(target, point)?;
    let (down, up, vk) = match button {
        Button::Left | Button::Double => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, VK_LBUTTON),
        Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, VK_RBUTTON),
        Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, VK_MBUTTON),
    };
    if held(vk) {
        return Err("Release the mouse button and retry.");
    }
    let release = mouse(up, 0, 0, 0);
    let mut events = vec![mouse(down, 0, 0, 0), release];
    if button == Button::Double {
        events.extend_from_within(..);
    }
    send(target, &events, &[release])
}

fn held(key: u16) -> bool {
    (unsafe { GetAsyncKeyState(i32::from(key)) }) < 0
}

fn key_code(name: &str) -> Option<u16> {
    let name = name.trim().to_lowercase().replace([' ', '-'], "");
    Some(match name.as_str() {
        "tab" => VK_TAB,
        "return" | "enter" => VK_RETURN,
        "esc" | "escape" => VK_ESCAPE,
        "space" | "spacebar" => VK_SPACE,
        "home" => VK_HOME,
        "end" => VK_END,
        "pgup" | "pageup" => VK_PRIOR,
        "pgdn" | "pagedown" => VK_NEXT,
        "left" | "leftarrow" => VK_LEFT,
        "right" | "rightarrow" => VK_RIGHT,
        "up" | "uparrow" => VK_UP,
        "down" | "downarrow" => VK_DOWN,
        "shift" => VK_SHIFT,
        "ctrl" | "control" => VK_CONTROL,
        "alt" => VK_MENU,
        "back" | "backspace" => VK_BACK,
        "f3" => VK_F3,
        "f5" => VK_F5,
        "f6" => VK_F6,
        "f7" => VK_F7,
        "minus" => VK_OEM_MINUS,
        "equals" => VK_OEM_PLUS,
        letter if letter.len() == 1 && letter.as_bytes()[0].is_ascii_alphanumeric() => {
            u16::from(letter.as_bytes()[0].to_ascii_uppercase())
        }
        _ => return None,
    })
}

fn extended(vk: u16) -> u32 {
    if matches!(
        vk,
        VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_PRIOR | VK_NEXT
    ) {
        KEYEVENTF_EXTENDEDKEY
    } else {
        0
    }
}

pub fn keys(target: TargetWindow, chord: &str, repeats: u8) -> Result<(), &'static str> {
    let codes = chord
        .split('+')
        .map(key_code)
        .collect::<Option<Vec<_>>>()
        .ok_or("That key or shortcut is unavailable in Desktop Control.")?;
    let Some((&primary, modifiers)) = codes.split_last() else {
        return Err(INPUT_ERROR);
    };
    if codes.len() > 3
        || repeats == 0
        || repeats > 12
        || modifiers
            .iter()
            .any(|code| !matches!(*code, VK_SHIFT | VK_CONTROL | VK_MENU))
        || matches!(primary, VK_SHIFT | VK_CONTROL | VK_MENU)
    {
        return Err("Use one key and up to two modifiers, repeated at most 12 times.");
    }
    if modifiers.contains(&VK_MENU) && !matches!(primary, VK_LEFT | VK_RIGHT)
        || modifiers.contains(&VK_CONTROL) && matches!(primary, 0x53 | 0x58 | VK_RETURN | VK_BACK)
    {
        return Err("That shortcut requires the desktop app's own controls.");
    }
    if [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(held)
        || codes.iter().copied().any(held)
    {
        return Err("Release the keyboard modifiers and try again.");
    }
    let release: Vec<_> = codes
        .iter()
        .rev()
        .map(|&code| key(code, 0, KEYEVENTF_KEYUP | extended(code)))
        .collect();
    let mut events: Vec<_> = codes
        .iter()
        .map(|&code| key(code, 0, extended(code)))
        .collect();
    events.extend_from_slice(&release);
    for _ in 0..repeats {
        send(target, &events, &release)?;
    }
    Ok(())
}

fn unicode(target: TargetWindow, text: &str) -> Result<(), &'static str> {
    if text.is_empty()
        || text.encode_utf16().count() > 1_000
        || text.chars().any(|ch| ch.is_control() && ch != '\n')
    {
        return Err("Enter up to 1,000 characters of plain text.");
    }
    if [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(held)
    {
        return Err("Release the keyboard modifiers and try again.");
    }
    let release: Vec<_> = text
        .encode_utf16()
        .map(|unit| key(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP))
        .collect();
    let events: Vec<_> = text
        .encode_utf16()
        .zip(&release)
        .flat_map(|(unit, up)| [key(0, unit, KEYEVENTF_UNICODE), *up])
        .collect();
    send(target, &events, &release)
}

fn drag(target: TargetWindow, start: POINT, end: POINT) -> Result<(), &'static str> {
    if held(VK_LBUTTON) || !contains(client(target)?, end) || !at_target(target, end) {
        return Err("Both drag endpoints must be in the same visible window.");
    }
    position(target, start)?;
    let release = mouse(MOUSEEVENTF_LEFTUP, 0, 0, 0);
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if width < 2 || height < 2 {
        return Err(INPUT_ERROR);
    }
    let mut events = vec![mouse(MOUSEEVENTF_LEFTDOWN, 0, 0, 0)];
    for step in 1..=10 {
        let x = start.x + (end.x - start.x) * step / 10;
        let y = start.y + (end.y - start.y) * step / 10;
        if !at_target(target, POINT { x, y }) {
            return Err("Another window covers the requested drag path.");
        }
        let scale = |value, origin, span| {
            ((i64::from(value) - i64::from(origin)) * 65_536 / i64::from(span)).clamp(0, 65_535)
                as i32
        };
        events.push(mouse(
            MOUSEEVENTF_MOVE
                | MOUSEEVENTF_ABSOLUTE
                | MOUSEEVENTF_VIRTUALDESK
                | MOUSEEVENTF_MOVE_NOCOALESCE,
            scale(x, left, width),
            scale(y, top, height),
            0,
        ));
    }
    events.push(release);
    send(target, &events, &[release])
}

pub fn apply(target: TargetWindow, action: &InputAction) -> Result<String, String> {
    activate(target)?;
    Ok(match action {
        InputAction::Move(direction, pixels) | InputAction::Drag(direction, pixels) => {
            if !(1..=2_000).contains(pixels) {
                return Err("Use an offset between 1 and 2,000 pixels.".into());
            }
            let start = cursor()?;
            let (dx, dy) = direction.offset(*pixels);
            let end = POINT {
                x: start.x.saturating_add(dx),
                y: start.y.saturating_add(dy),
            };
            if matches!(action, InputAction::Drag(..)) {
                drag(target, start, end)?;
                "Dragged the pointer."
            } else {
                position(target, end)?;
                "Moved the pointer."
            }
        }
        InputAction::Point(point) => {
            let client = client(target)?;
            if *point == Landmark::TitleBar {
                let mut outer = RECT::default();
                let handle = target.handle as HWND;
                if unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(handle, &mut outer)
                } == 0
                    || client.top - outer.top < 8
                {
                    return Err("This window has no separate title bar.".into());
                }
                let point = POINT {
                    x: (outer.left + outer.right) / 2,
                    y: (outer.top + client.top) / 2,
                };
                focused(target)?;
                if !at_target(target, point) || unsafe { SetCursorPos(point.x, point.y) } == 0 {
                    return Err(INPUT_ERROR.into());
                }
            } else {
                position(target, landmark(client, *point))?;
            }
            "Moved the pointer to the requested point."
        }
        InputAction::Click(button, label) => {
            let point = match label {
                Some(label) => super::accessibility::point(target, label)?,
                None => cursor()?,
            };
            click(target, point, *button)?;
            "Clicked the target."
        }
        InputAction::Scroll(direction, notches) => {
            if !(1..=25).contains(notches) {
                return Err("Choose between 1 and 25 wheel notches.".into());
            }
            if let Some(applied) = super::accessibility::scroll(target, *direction, *notches) {
                return Ok(if applied? {
                    "Scrolled the selected window."
                } else {
                    "The selected window did not move further."
                }
                .into());
            }
            position(target, landmark(client(target)?, Landmark::Center))?;
            let (flag, sign) = match direction {
                Direction::Up => (MOUSEEVENTF_WHEEL, 1),
                Direction::Down => (MOUSEEVENTF_WHEEL, -1),
                Direction::Left => (MOUSEEVENTF_HWHEEL, -1),
                Direction::Right => (MOUSEEVENTF_HWHEEL, 1),
            };
            for _ in 0..*notches {
                send(target, &[mouse(flag, 0, 0, (sign * 120) as u32)], &[])?;
            }
            "Sent scroll input to the selected window."
        }
        InputAction::Keys(chord, repeats) => {
            keys(target, chord, *repeats)?;
            "Sent the requested key input."
        }
        InputAction::Text(text) => {
            super::accessibility::editable(target)?;
            unicode(target, text)?;
            "Inserted text in the focused field."
        }
        InputAction::Select(direction, times, by_word) => {
            let arrow = match direction {
                Direction::Up => "Up",
                Direction::Down => "Down",
                Direction::Left => "Left",
                Direction::Right => "Right",
            };
            let chord = format!("{}Shift+{arrow}", if *by_word { "Ctrl+" } else { "" });
            keys(target, &chord, *times)?;
            "Sent selection input."
        }
        InputAction::SelectLine(end) => {
            keys(target, if *end { "Shift+End" } else { "Shift+Home" }, 1)?;
            "Extended the selection to the requested line boundary."
        }
        InputAction::Find(text) => {
            keys(target, "Ctrl+F", 1)?;
            for _ in 0..8 {
                if super::accessibility::find_field(target).is_ok() {
                    unicode(target, text)?;
                    return Ok("Entered the search text in the find field.".into());
                }
                thread::sleep(Duration::from_millis(15));
            }
            return Err("The app did not expose an editable find field.".into());
        }
        InputAction::Paste => {
            super::accessibility::editable(target)?;
            keys(target, "Ctrl+V", 1)?;
            "Pasted into the selected window's focused field."
        }
        InputAction::Replace(text) => {
            super::accessibility::selection(target)?;
            unicode(target, text)?;
            "Replaced the selection."
        }
        InputAction::Case(mode) => {
            let selected = super::accessibility::selected_text(target)?;
            let result =
                match mode.to_lowercase().as_str() {
                    "uppercase" => selected.to_uppercase(),
                    "lowercase" => selected.to_lowercase(),
                    "title case" | "sentence case" | "toggle case" => {
                        change_case(&selected, &mode.to_lowercase())
                    }
                    _ => return Err(
                        "Choose uppercase, lowercase, title case, sentence case, or toggle case."
                            .into(),
                    ),
                };
            super::accessibility::selection(target)?;
            unicode(target, &result)?;
            "Changed the case of the selected text."
        }
        InputAction::Hover(name) => {
            position(target, super::accessibility::point(target, name)?)?;
            "Moved the pointer to the named control."
        }
        other => return super::accessibility::apply(target, other),
    }
    .into())
}

fn change_case(text: &str, mode: &str) -> String {
    let mut output = String::new();
    let mut start = true;
    for ch in text.chars() {
        if mode == "toggle case" {
            if ch.is_uppercase() {
                output.extend(ch.to_lowercase());
            } else {
                output.extend(ch.to_uppercase());
            }
        } else if ch.is_alphabetic() {
            if start {
                output.extend(ch.to_uppercase());
            } else {
                output.extend(ch.to_lowercase());
            }
            start = false;
        } else {
            output.push(ch);
            start |= if mode == "title case" {
                ch.is_whitespace() || ch == '-'
            } else {
                matches!(ch, '.' | '?' | '!')
            };
        }
    }
    output
}

pub fn work_area(target: TargetWindow) -> Result<RECT, &'static str> {
    let monitor = unsafe { MonitorFromWindow(target.handle as HWND, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0)
        .then_some(info.rcWork)
        .ok_or("Windows could not read the display work area.")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_maps_numbered_cells_inside_negative_origin_windows() {
        let bounds = RECT {
            left: -900,
            top: -300,
            right: 0,
            bottom: 300,
        };
        let point = cell(bounds, 3, 2, 5).unwrap();
        assert_eq!((point.x, point.y), (-450, 150));
        assert!(cell(bounds, 3, 2, 7).is_none());
    }
}
