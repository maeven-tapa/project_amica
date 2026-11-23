use std::{
    ffi::OsString,
    mem::size_of,
    os::windows::{ffi::OsStringExt, process::CommandExt},
    process::Command,
    ptr,
};

use windows_sys::Win32::{
    Foundation::RPC_E_CHANGED_MODE,
    System::{
        Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize},
        SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW},
        Threading::CREATE_NO_WINDOW,
    },
    UI::{
        Shell::{
            ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE, AssocQueryStringW, SEE_MASK_FLAG_NO_UI,
            SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
        },
        WindowsAndMessaging::SW_SHOWNORMAL,
    },
};

use super::PowerAction;

const LAUNCH_ERROR: &str = "Windows could not open this app. Try opening it from Start.";

pub(super) fn open(id: &str) -> Result<(), &'static str> {
    let _com = ComApartment::initialize()?;
    let launched = match id {
        "spotify" => {
            (has_handler("spotify") && open_uri("spotify:"))
                || open_uri("https://open.spotify.com/")
        }
        "youtube" => open_uri("https://www.youtube.com/"),
        "browser" => open_uri("https://www.google.com/"),
        "settings" => open_uri("ms-settings:"),
        "calculator" => open_system_app(GetSystemDirectoryW, "calc.exe"),
        "notepad" => open_system_app(GetSystemDirectoryW, "notepad.exe"),
        "paint" => open_system_app(GetSystemDirectoryW, "mspaint.exe"),
        "photos" => open_uri(r"shell:AppsFolder\Microsoft.Windows.Photos_8wekyb3d8bbwe!App"),
        "clock" => open_uri(r"shell:AppsFolder\Microsoft.WindowsAlarms_8wekyb3d8bbwe!App"),
        "snipping_tool" => open_uri(r"shell:AppsFolder\Microsoft.ScreenSketch_8wekyb3d8bbwe!App"),
        "edge" => open_uri("microsoft-edge:"),
        "microsoft_store" => open_uri("ms-windows-store:"),
        "file_explorer" => open_system_app(GetWindowsDirectoryW, "explorer.exe"),
        _ => false,
    };
    launched.then_some(()).ok_or(LAUNCH_ERROR)
}

pub(crate) struct ComApartment(bool);

impl ComApartment {
    pub(crate) fn initialize() -> Result<Self, &'static str> {
        let result = unsafe {
            CoInitializeEx(
                ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        if result >= 0 {
            Ok(Self(true))
        } else if result == RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else {
            Err(LAUNCH_ERROR)
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn has_handler(protocol: &str) -> bool {
    let protocol: Vec<u16> = protocol.encode_utf16().chain([0]).collect();
    let mut len = 0;
    let status = unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL,
            ASSOCSTR_EXECUTABLE,
            protocol.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            &mut len,
        )
    };
    status >= 0 && len > 1
}

fn open_uri(uri: &str) -> bool {
    let uri: Vec<u16> = uri.encode_utf16().chain([0]).collect();
    shell_open(&uri)
}

pub fn open_protocol(uri: &str) -> Result<(), &'static str> {
    let _com = ComApartment::initialize()?;
    open_uri(uri)
        .then_some(())
        .ok_or("Windows could not open that address.")
}

pub fn open_path(path: &std::path::Path) -> Result<(), &'static str> {
    use std::os::windows::ffi::OsStrExt;
    let _com = ComApartment::initialize()?;
    let path: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    shell_open(&path).then_some(()).ok_or(LAUNCH_ERROR)
}

pub fn open_in_notepad(path: &std::path::Path) -> Result<(), &'static str> {
    let mut executable = system_path(GetSystemDirectoryW, "notepad.exe").ok_or(LAUNCH_ERROR)?;
    executable.pop();
    Command::new(OsString::from_wide(&executable))
        .arg(path)
        .spawn()
        .map_err(|_| LAUNCH_ERROR)?;
    Ok(())
}

pub(super) fn open_page(url: &str) -> Result<(), &'static str> {
    let _com = ComApartment::initialize()?;
    open_uri(url).then_some(()).ok_or(LAUNCH_ERROR)
}

fn open_system_app(directory: unsafe extern "system" fn(*mut u16, u32) -> u32, file: &str) -> bool {
    system_path(directory, file).is_some_and(|path| shell_open(&path))
}

fn system_path(
    directory: unsafe extern "system" fn(*mut u16, u32) -> u32,
    file: &str,
) -> Option<Vec<u16>> {
    let mut path = vec![0; 260];
    let mut len = unsafe { directory(path.as_mut_ptr(), path.len() as u32) } as usize;
    if len >= path.len() {
        path.resize(len, 0);
        len = unsafe { directory(path.as_mut_ptr(), path.len() as u32) } as usize;
    }
    if len == 0 || len >= path.len() {
        return None;
    }
    path.truncate(len);
    path.extend("\\".encode_utf16().chain(file.encode_utf16()).chain([0]));
    Some(path)
}

pub(super) fn power(action: PowerAction) -> Result<(), &'static str> {
    const ERROR: &str = "Windows could not complete the PC power request.";
    let flag = match action {
        PowerAction::Shutdown => "/s",
        PowerAction::Restart => "/r",
        PowerAction::Sleep => {
            let slept =
                unsafe { windows_sys::Win32::System::Power::SetSuspendState(false, false, false) };
            return slept.then_some(()).ok_or(ERROR);
        }
    };
    let mut path = system_path(GetSystemDirectoryW, "shutdown.exe").ok_or(ERROR)?;
    path.pop();
    let status = Command::new(OsString::from_wide(&path))
        .args([flag, "/t", "0"])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|_| ERROR)?;
    status.success().then_some(()).ok_or(ERROR)
}

fn shell_open(target: &[u16]) -> bool {
    let operation = [b'o' as u16, b'p' as u16, b'e' as u16, b'n' as u16, 0];
    let mut launch = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpVerb: operation.as_ptr(),
        lpFile: target.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut launch) != 0 }
}
