use std::{
    ffi::OsStr,
    fs,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use ::windows::{
    Win32::{
        Storage::FileSystem::{CopyFileW, MoveFileW},
        System::{
            Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree},
            Variant::VARIANT,
        },
        UI::Shell::*,
    },
    core::{BSTR, Interface, PCWSTR},
};

use super::{
    TargetWindow,
    windows::{self, ComApartment},
};
use crate::actions::command::FileAction;

const EXPLORER: &str = "Choose the Explorer window showing a regular local folder.";

pub struct Change {
    pub target: TargetWindow,
    pub from: PathBuf,
    pub to: Option<PathBuf>,
    pub copy: bool,
}

impl Change {
    pub fn description(&self) -> String {
        if let Some(to) = &self.to {
            let verb = if self.copy { "Copy" } else { "Move or rename" };
            format!("{verb} {}\nTo {}", self.from.display(), to.display())
        } else {
            format!("Move to the Recycle Bin\n{}", self.from.display())
        }
    }

    pub fn run(self) -> Result<String, String> {
        let _apartment = ComApartment::initialize()?;
        let view = view(self.target)?;
        let current = folder(&view)?;
        if self.from.parent() != Some(current.as_path()) || !self.from.is_file() {
            return Err("The source folder or file changed. Request a new preview.".into());
        }
        let from = wide(&self.from);
        match self.to {
            Some(to) => {
                let target = wide(&to);
                unsafe {
                    if self.copy {
                        CopyFileW(PCWSTR(from.as_ptr()), PCWSTR(target.as_ptr()), true)
                    } else {
                        MoveFileW(PCWSTR(from.as_ptr()), PCWSTR(target.as_ptr()))
                    }
                }
                .map_err(|err| format!("Windows could not complete the operation. {err}"))?;
                Ok(if self.copy {
                    "Copied the file."
                } else {
                    "Moved or renamed the file."
                }
                .into())
            }
            None => {
                unsafe {
                    let operation: IFileOperation =
                        CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
                            .map_err(|_| "The Recycle Bin is unavailable.")?;
                    operation
                        .SetOperationFlags(
                            FOFX_ADDUNDORECORD
                                | FOFX_RECYCLEONDELETE
                                | FOF_WANTNUKEWARNING
                                | FOF_NOERRORUI
                                | FOFX_EARLYFAILURE,
                        )
                        .map_err(|_| "Windows did not enable recycling.")?;
                    let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(from.as_ptr()), None)
                        .map_err(|_| "The file is unavailable.")?;
                    operation
                        .DeleteItem(&item, None)
                        .map_err(|_| "Windows would not queue the file for recycling.")?;
                    operation
                        .PerformOperations()
                        .map_err(|_| "Windows could not recycle the file.")?;
                    if operation
                        .GetAnyOperationsAborted()
                        .map_or(true, |aborted| aborted.as_bool())
                        || self.from.exists()
                    {
                        return Err("Windows did not recycle the file.".into());
                    }
                }
                Ok("Moved the file to the Recycle Bin.".into())
            }
        }
    }
}

pub fn known_folder(name: &str) -> Result<PathBuf, &'static str> {
    let id = match name.trim().to_lowercase().as_str() {
        "downloads" => FOLDERID_Downloads,
        "documents" => FOLDERID_Documents,
        "desktop" => FOLDERID_Desktop,
        "pictures" => FOLDERID_Pictures,
        "music" => FOLDERID_Music,
        "videos" => FOLDERID_Videos,
        "home" | "profile" => FOLDERID_Profile,
        _ => return Err("Choose Downloads, Documents, Desktop, Pictures, Music, Videos, or Home."),
    };
    unsafe {
        let folder = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None)
            .map_err(|_| "Windows did not resolve that folder.")?;
        let result = folder
            .to_string()
            .map(PathBuf::from)
            .map_err(|_| "Windows returned an unreadable folder path.");
        CoTaskMemFree(Some(folder.0.cast()));
        result
    }
}

fn view(target: TargetWindow) -> Result<IShellFolderViewDual, &'static str> {
    if !super::external::alive(target) {
        return Err(EXPLORER);
    }
    unsafe {
        let shell: IShellWindows =
            CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).map_err(|_| EXPLORER)?;
        for index in 0..shell.Count().map_err(|_| EXPLORER)? {
            let Ok(browser) = shell
                .Item(&VARIANT::from(index))
                .and_then(|item| item.cast::<IWebBrowserApp>())
            else {
                continue;
            };
            if browser
                .HWND()
                .ok()
                .is_some_and(|handle| handle.0 as usize == target.handle)
            {
                return browser
                    .Document()
                    .and_then(|document| document.cast())
                    .map_err(|_| EXPLORER);
            }
        }
    }
    Err(EXPLORER)
}

fn folder(view: &IShellFolderViewDual) -> Result<PathBuf, &'static str> {
    let path = unsafe {
        view.Folder()
            .and_then(|folder| folder.cast::<Folder2>())
            .and_then(|folder| folder.Self_())
            .and_then(|item| item.Path())
    }
    .map_err(|_| EXPLORER)?;
    let path = PathBuf::from(path.to_string());
    (path.is_dir() && path.is_absolute() && !path.as_os_str().to_string_lossy().starts_with("\\\\"))
        .then_some(path)
        .ok_or(EXPLORER)
}

fn selected(view: &IShellFolderViewDual) -> Result<PathBuf, &'static str> {
    unsafe {
        let items = view.SelectedItems().map_err(|_| EXPLORER)?;
        if items.Count().ok() != Some(1) {
            return Err("Select one file in Explorer first.");
        }
        let path = items
            .Item(&VARIANT::from(0))
            .and_then(|item| item.Path())
            .map_err(|_| "Windows could not read the selected file.")?;
        let path = PathBuf::from(path.to_string());
        path.is_file()
            .then_some(path)
            .ok_or("Select one regular file in Explorer first.")
    }
}

pub fn child(base: &Path, value: &str) -> Result<PathBuf, &'static str> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 240
        || value.ends_with(['.', ' '])
        || value.chars().any(|ch| {
            ch.is_control() || matches!(ch, '/' | '\\' | ':' | '<' | '>' | '|' | '?' | '*' | '"')
        })
        || matches!(value, "." | "..")
    {
        return Err("Enter a single valid file or folder name.");
    }
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4)
    {
        return Err("Windows reserves that file name.");
    }
    Ok(base.join(value))
}

pub fn save_copy(target: TargetWindow, destination: &Path) -> Result<String, String> {
    use std::io::Write;
    super::input::activate(target)?;
    let text = super::accessibility::document_text(target)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| "Choose a new filename in a writable folder.")?;
    if file
        .write_all(text.as_bytes())
        .and_then(|()| file.flush())
        .is_err()
    {
        drop(file);
        let _ = fs::remove_file(destination);
        return Err("Amica could not write the text copy.".into());
    }
    Ok(format!("Saved a copy to {}.", destination.display()))
}

pub fn preview(target: TargetWindow, action: &FileAction) -> Result<Change, String> {
    let _apartment = ComApartment::initialize()?;
    let view = view(target)?;
    let base = folder(&view)?;
    let from = match action {
        FileAction::Recycle(name) => child(&base, name)?,
        _ => selected(&view)?,
    };
    if !from.is_file()
        || from.parent() != Some(base.as_path())
        || fs::symlink_metadata(&from).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err("Choose a regular file inside the visible folder.".into());
    }
    let to = match action {
        FileAction::Copy(name) | FileAction::Move(name) => Some(
            known_folder(name)?.join(
                from.file_name()
                    .ok_or("The source filename is unavailable.")?,
            ),
        ),
        FileAction::Rename(name) => {
            let mut name = child(&base, name)?
                .file_name()
                .ok_or("Enter a new file name.")?
                .to_os_string();
            if let Some(extension) = from.extension() {
                name.push(".");
                name.push(extension);
            }
            Some(base.join(name))
        }
        FileAction::Recycle(_) => None,
        _ => return Err("That file operation has no preview.".into()),
    };
    if to.as_ref().is_some_and(|path| path.exists()) {
        return Err(
            "A file already exists at the destination. Use a different name or folder.".into(),
        );
    }
    Ok(Change {
        target,
        from,
        to,
        copy: matches!(action, FileAction::Copy(_)),
    })
}

pub fn run(target: Option<TargetWindow>, action: &FileAction) -> Result<String, String> {
    let _apartment = ComApartment::initialize()?;
    if let FileAction::OpenFolder(name) = action {
        let path = known_folder(name)?;
        windows::open_path(&path)?;
        return Ok(format!("Opened {name}."));
    }
    let view = view(target.ok_or(EXPLORER)?)?;
    let base = folder(&view)?;
    match action {
        FileAction::Create(name) => {
            fs::create_dir(child(&base, name)?)
                .map_err(|err| format!("Windows could not create the folder. {err}"))?;
            Ok(format!("Created {name}."))
        }
        FileAction::Find(query) | FileAction::Filter(query) => {
            let query = query.trim().to_lowercase();
            let paths = fs::read_dir(&base)
                .map_err(|_| EXPLORER)?
                .filter_map(Result::ok)
                .filter(|entry| {
                    if matches!(action, FileAction::Filter(_)) {
                        entry
                            .path()
                            .extension()
                            .and_then(OsStr::to_str)
                            .is_some_and(|ext| ext.eq_ignore_ascii_case(&query))
                    } else {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&query)
                    }
                })
                .take(25)
                .map(|entry| entry.path().display().to_string())
                .collect::<Vec<_>>();
            Ok(if paths.is_empty() {
                "No matching items in the selected folder.".into()
            } else {
                paths.join("\n")
            })
        }
        FileAction::Sort(order) => {
            let order = order.to_lowercase();
            let column = if order.contains("date") || order.contains("petsa") {
                "System.DateModified"
            } else if order.contains("size") || order.contains("laki") {
                "System.Size"
            } else if order.contains("type") || order.contains("uri") {
                "System.ItemTypeText"
            } else {
                "System.ItemNameDisplay"
            };
            let descending = ["desc", "newest", "largest", "z hanggang a", "pinakabago"]
                .iter()
                .any(|word| order.contains(word));
            let sort = BSTR::from(format!(
                "prop:{}{column};",
                if descending { "-" } else { "+" }
            ));
            let advanced: IShellFolderViewDual3 = view.cast().map_err(|_| EXPLORER)?;
            unsafe { advanced.SetSortColumns(&sort) }
                .map_err(|_| "Explorer rejected this sort order.")?;
            Ok("Changed the Explorer sort order.".into())
        }
        FileAction::View(mode) => {
            let mode = match mode.to_lowercase().as_str() {
                "small icons" => FVM_SMALLICON,
                "list" => FVM_LIST,
                "details" => FVM_DETAILS,
                "tiles" => FVM_TILE,
                "content" => FVM_CONTENT,
                "large icons" | "medium icons" | "extra large icons" => FVM_ICON,
                _ => return Err("This Explorer view or spacing mode is unavailable.".into()),
            };
            let advanced: IShellFolderViewDual2 = view.cast().map_err(|_| EXPLORER)?;
            unsafe { advanced.SetCurrentViewMode(mode.0 as u32) }
                .map_err(|_| "Explorer rejected this view mode.")?;
            Ok("Changed the Explorer view.".into())
        }
        FileAction::Edit(name) | FileAction::Play(name) => {
            let path = child(&base, name)?;
            if !path.is_file() {
                return Err("That file could not be found in the selected folder.".into());
            }
            if matches!(action, FileAction::Edit(_)) {
                if !path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
                    ["txt", "md", "csv", "log", "json"]
                        .iter()
                        .any(|item| item.eq_ignore_ascii_case(ext))
                }) {
                    return Err("Choose a supported text file for Notepad.".into());
                }
                windows::open_in_notepad(&path)?;
            } else {
                if !path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
                    [
                        "mp3", "wav", "ogg", "m4a", "flac", "mp4", "webm", "mkv", "avi",
                    ]
                    .iter()
                    .any(|item| item.eq_ignore_ascii_case(ext))
                }) {
                    return Err("Choose a supported local audio or video file.".into());
                }
                windows::open_path(&path)?;
            }
            Ok(format!("Opened {name}."))
        }
        FileAction::NewText(name) => {
            let label = if name.to_lowercase().ends_with(".txt") {
                name.clone()
            } else {
                format!("{name}.txt")
            };
            let path = child(&base, &label)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|_| "That draft could not be created. Check the name and destination.")?;
            windows::open_in_notepad(&path)?;
            Ok(format!("Opened a new draft at {}.", path.display()))
        }
        _ => Err("This operation requires a preview or the editor’s own Save As control.".into()),
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}
