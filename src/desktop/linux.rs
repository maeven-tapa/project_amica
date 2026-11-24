use std::{
    env, fs, io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use super::PowerAction;

pub(crate) fn supports(id: &str) -> bool {
    matches!(
        id,
        "spotify"
            | "youtube"
            | "browser"
            | "file_explorer"
            | "calculator"
            | "notepad"
            | "photos"
            | "settings"
    )
}

pub(super) fn open(id: &str) -> Result<(), &'static str> {
    match id {
        "spotify" => open_page("https://open.spotify.com/"),
        "youtube" => open_page("https://www.youtube.com/"),
        "browser" => open_page("https://www.google.com/"),
        "file_explorer" => open_folder("home"),
        "calculator" => launch_app(&["galculator", "gnome-calculator", "kcalc"]),
        "notepad" => launch_app(&["mousepad", "leafpad", "gedit", "kate"]),
        "photos" => launch_app(&["ristretto", "gpicview", "eog"]),
        "settings" => launch_app(&["rc_gui", "lxappearance", "gnome-control-center"]),
        _ => Err("This app shortcut is unavailable on Linux."),
    }
}

fn launch_app(apps: &[&str]) -> Result<(), &'static str> {
    for app in apps {
        match Command::new(app)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err("Linux could not launch the selected app."),
        }
    }
    Err("Install a desktop app for this shortcut and try again.")
}

pub(super) fn open_page(url: &str) -> Result<(), &'static str> {
    open_with_default_app(Path::new(url)).map_err(|_| "Could not open the default browser.")
}

pub(super) fn open_folder(name: &str) -> Result<(), &'static str> {
    open_with_default_app(&known_folder(name)?)
        .map_err(|_| "Could not open the folder in the file manager.")
}

fn open_with_default_app(path: &Path) -> Result<(), ()> {
    Command::new("gio")
        .arg("open")
        .arg(path)
        .output()
        .is_ok_and(|output| output.status.success())
        .then_some(())
        .ok_or(())
}

fn known_folder(name: &str) -> Result<PathBuf, &'static str> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("Linux did not report a home directory.")?;
    let (variable, fallback) = match name.trim().to_ascii_lowercase().as_str() {
        "home" | "profile" => return Ok(home),
        "downloads" => ("DOWNLOAD", "Downloads"),
        "documents" => ("DOCUMENTS", "Documents"),
        "desktop" => ("DESKTOP", "Desktop"),
        "pictures" => ("PICTURES", "Pictures"),
        "music" => ("MUSIC", "Music"),
        "videos" => ("VIDEOS", "Videos"),
        _ => return Err("Choose Downloads, Documents, Desktop, Pictures, Music, Videos, or Home."),
    };
    let path = Command::new("xdg-user-dir")
        .arg(variable)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let bytes = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
            PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
        })
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(fallback));
    Ok(path)
}

pub(super) fn power(action: PowerAction) -> Result<(), &'static str> {
    let action = match action {
        PowerAction::Shutdown => "poweroff",
        PowerAction::Restart => "reboot",
        PowerAction::Sleep => "suspend",
    };
    Command::new("systemctl")
        .arg(action)
        .output()
        .is_ok_and(|output| output.status.success())
        .then_some(())
        .ok_or("Linux denied the request or this power state is unavailable.")
}

pub(crate) fn set_volume(amount: i32, relative: bool) -> Result<String, &'static str> {
    if amount.unsigned_abs() > 100 || (!relative && amount < 0) {
        return Err("Choose a volume adjustment within 100 percentage points.");
    }
    let mut value = format!("{}%", amount.unsigned_abs());
    if relative {
        value.push(if amount < 0 { '-' } else { '+' });
    }
    Command::new("wpctl")
        .args([
            "set-volume",
            "--limit",
            "1.0",
            "@DEFAULT_AUDIO_SINK@",
            &value,
        ])
        .output()
        .is_ok_and(|output| output.status.success())
        .then(|| "Updated the default audio output volume.".into())
        .ok_or("Could not set the default PipeWire output volume.")
}

pub(crate) fn batteries() -> impl Iterator<Item = PathBuf> {
    fs::read_dir("/sys/class/power_supply")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            fs::read_to_string(path.join("type")).is_ok_and(|kind| kind.trim() == "Battery")
        })
}

pub(crate) fn on_battery() -> bool {
    batteries().any(|path| {
        fs::read_to_string(path.join("status")).is_ok_and(|status| status.trim() == "Discharging")
    })
}

pub(crate) fn metric(name: &str) -> Result<String, String> {
    match name {
        "battery percentage" => {
            let percent = batteries()
                .find_map(|path| {
                    fs::read_to_string(path.join("capacity"))
                        .ok()?
                        .trim()
                        .parse::<u8>()
                        .ok()
                        .filter(|&value| value <= 100)
                })
                .ok_or("Linux did not report a battery percentage.")?;
            Ok(format!("Battery is {percent}%."))
        }
        "kasalukuyang pinagmumulan ng kuryente" => {
            if batteries().next().is_none() {
                return Ok("Linux did not report a battery.".into());
            }
            Ok(if on_battery() {
                "This computer is running on battery."
            } else {
                "The battery is not discharging."
            }
            .into())
        }
        "default na audio output" => {
            use cpal::traits::HostTrait;
            let device = cpal::default_host()
                .default_output_device()
                .ok_or("Linux did not report a default audio output.")?;
            Ok(format!(
                "Default audio output: {}.",
                crate::settings::name(&device)
            ))
        }
        "free disk space" => {
            let stats = rustix::fs::statvfs("/")
                .map_err(|_| "Linux did not report the free system disk space.")?;
            let available = stats.f_bavail as f64 * stats.f_frsize as f64;
            Ok(format!(
                "System disk free space: {:.1} GiB.",
                available / 1_073_741_824.0
            ))
        }
        name => crate::usage::metric(name),
    }
}
