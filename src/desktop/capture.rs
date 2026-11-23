use std::{
    fs::{self, OpenOptions},
    io::Write,
    mem::size_of,
    path::Path,
};

use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GetWindowDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SelectObject,
    },
    Storage::Xps::PrintWindow,
    UI::WindowsAndMessaging::{GetWindowRect, IsIconic},
};

use super::{TargetWindow, external};

struct Canvas {
    hwnd: HWND,
    source: HDC,
    image: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            if !self.previous.is_null() {
                SelectObject(self.image, self.previous);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.image.is_null() {
                DeleteDC(self.image);
            }
            if !self.source.is_null() {
                ReleaseDC(self.hwnd, self.source);
            }
        }
    }
}

pub fn save(target: TargetWindow, path: &Path) -> Result<String, String> {
    const ERROR: &str = "Windows could not capture this window.";
    let hwnd = target.handle as HWND;
    let mut rect = RECT::default();
    if !external::alive(target)
        || unsafe { IsIconic(hwnd) } != 0
        || unsafe { GetWindowRect(hwnd, &mut rect) } == 0
    {
        return Err("Restore the selected window before capturing it.".into());
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let pixels = width
        .checked_mul(height)
        .filter(|&size| (1..=16_777_216).contains(&size))
        .ok_or("The selected window is outside the supported capture size.")?
        as usize;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut canvas = Canvas {
        hwnd,
        source: unsafe { GetWindowDC(hwnd) },
        image: std::ptr::null_mut(),
        bitmap: std::ptr::null_mut(),
        previous: std::ptr::null_mut(),
    };
    if canvas.source.is_null() {
        return Err(ERROR.into());
    }
    canvas.image = unsafe { CreateCompatibleDC(canvas.source) };
    if canvas.image.is_null() {
        return Err(ERROR.into());
    }
    let mut bits = std::ptr::null_mut();
    canvas.bitmap = unsafe {
        CreateDIBSection(
            canvas.image,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        )
    };
    if canvas.bitmap.is_null() || bits.is_null() {
        return Err(ERROR.into());
    }
    canvas.previous = unsafe { SelectObject(canvas.image, canvas.bitmap) };
    if canvas.previous.is_null() || unsafe { PrintWindow(hwnd, canvas.image, 2) } == 0 {
        return Err(ERROR.into());
    }
    let pixels = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), pixels * 4) };
    if pixels.chunks_exact(4).all(|pixel| pixel[..3] == [0, 0, 0]) {
        return Err("Windows returned an empty capture for this window.".into());
    }
    let directory = path
        .parent()
        .ok_or("The destination folder is unavailable.")?;
    fs::create_dir_all(directory).map_err(|_| "Amica could not create the screenshot folder.")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "Amica could not create that screenshot file.")?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(b"BM")?;
        file.write_all(&(pixels.len() as u32 + 54).to_le_bytes())?;
        file.write_all(&0u32.to_le_bytes())?;
        file.write_all(&54u32.to_le_bytes())?;
        file.write_all(&40u32.to_le_bytes())?;
        file.write_all(&width.to_le_bytes())?;
        file.write_all(&(-height).to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&32u16.to_le_bytes())?;
        file.write_all(&[0; 24])?;
        file.write_all(pixels)?;
        file.flush()
    })();
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err("Amica could not write the screenshot to disk.".into());
    }
    Ok(format!("Saved the local capture at {}.", path.display()))
}
