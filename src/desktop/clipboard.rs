use std::{mem::size_of, ptr};

use windows_sys::Win32::{
    Foundation::GlobalFree,
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
};

pub fn set(owner: windows_sys::Win32::Foundation::HWND, text: &str) -> Result<(), &'static str> {
    const ERROR: &str = "Windows could not update the clipboard.";
    if text.contains('\0') {
        return Err("Clipboard text cannot contain NUL characters.");
    }
    let text: Vec<_> = text.encode_utf16().chain([0]).collect();
    unsafe {
        let data = GlobalAlloc(GMEM_MOVEABLE, text.len() * size_of::<u16>());
        if data.is_null() {
            return Err(ERROR);
        }
        let memory = GlobalLock(data).cast::<u16>();
        if memory.is_null() {
            GlobalFree(data);
            return Err(ERROR);
        }
        ptr::copy_nonoverlapping(text.as_ptr(), memory, text.len());
        GlobalUnlock(data);
        if owner.is_null() || OpenClipboard(owner) == 0 {
            GlobalFree(data);
            return Err(ERROR);
        }
        let transferred =
            EmptyClipboard() != 0 && !SetClipboardData(u32::from(CF_UNICODETEXT), data).is_null();
        CloseClipboard();
        if !transferred {
            GlobalFree(data);
            return Err(ERROR);
        }
    }
    Ok(())
}
