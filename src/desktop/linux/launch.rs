use std::{
    env, io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const SETTINGS: &[&[&str]] = &[
    &["rpcc"],
    &["rc_gui"],
    &["gnome-control-center"],
    &["xfce4-settings-manager"],
];
const SOFTWARE: &[&[&str]] = &[
    &["rp-prefapps"],
    &["pi-package"],
    &["gnome-software"],
    &["synaptic"],
];

enum Target {
    Page(&'static str),
    Home,
    Apps(&'static [&'static [&'static str]]),
}

fn target(id: &str) -> Option<Target> {
    Some(match id {
        "spotify" => Target::Page("https://open.spotify.com/"),
        "youtube" => Target::Page("https://www.youtube.com/"),
        "browser" => Target::Page("https://www.google.com/"),
        "file_explorer" => Target::Home,
        "calculator" => Target::Apps(&[&["galculator"], &["gnome-calculator"], &["kcalc"]]),
        "notepad" => Target::Apps(&[&["mousepad"], &["leafpad"], &["gedit"], &["kate"]]),
        "paint" => Target::Apps(&[&["mtpaint"], &["pinta"], &["kolourpaint"]]),
        "photos" => Target::Apps(&[&["gpicview"], &["ristretto"], &["eog"]]),
        "clock" => Target::Apps(&[&["gnome-clocks"], &["kclock"]]),
        "snipping_tool" => Target::Apps(&[
            &["gui-screenshot"],
            &["xfce4-screenshooter"],
            &["gnome-screenshot", "--interactive"],
        ]),
        "edge" => Target::Apps(&[
            &["chromium"],
            &["chromium-browser"],
            &["microsoft-edge"],
            &["microsoft-edge-stable"],
        ]),
        "microsoft_store" => Target::Apps(SOFTWARE),
        "settings" => Target::Apps(SETTINGS),
        _ => return None,
    })
}

pub(in crate::desktop) fn open(id: &str) -> Result<(), &'static str> {
    match target(id).ok_or("Choose an app from the Apps panel.")? {
        Target::Page(url) => open_page(url),
        Target::Home => open_folder("home"),
        Target::Apps(apps) => launch_apps(apps),
    }
}

fn launch_apps(apps: &[&[&str]]) -> Result<(), &'static str> {
    for command in apps {
        let (app, args) = command.split_first().unwrap();
        match Command::new(app)
            .args(args)
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
    Err("This shortcut needs a desktop app. See the Raspberry Pi setup guide for packages.")
}

pub(in crate::desktop) fn open_page(url: &str) -> Result<(), &'static str> {
    open_with_default_app(Path::new(url)).map_err(|_| "Could not open the default browser.")
}

pub(in crate::desktop) fn open_folder(name: &str) -> Result<(), &'static str> {
    open_with_default_app(&known_folder(name)?)
        .map_err(|_| "Could not open the folder in the file manager.")
}

fn open_with_default_app(path: &Path) -> Result<(), ()> {
    Command::new("gio")
        .args(["open", "--"])
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

pub(super) fn settings(page: &str) -> Result<String, String> {
    let (apps, fallback): (&[&[&str]], &str) = match page.trim().to_ascii_lowercase().as_str() {
        "wi-fi" | "wifi" | "network" => (&[&["nm-connection-editor"], &["gnome-control-center", "wifi"]], "Use the network menu to manage Wi-Fi."),
        "bluetooth" => (&[&["blueman-manager"], &["gnome-control-center", "bluetooth"]], "Use the Bluetooth menu to manage devices."),
        "display" | "screen" | "resolution" | "display scaling" | "scaling" | "orientation" | "brightness" | "night light" => (
            &[&["raindrop"], &["wdisplays"], &["arandr"], &["gnome-control-center", "display"]],
            "Look for Screens. Display options depend on the monitor and desktop.",
        ),
        "sound" | "volume" | "audio" | "volume mixer" | "mixer" => (
            &[&["pavucontrol"], &["gnome-control-center", "sound"]], "Use the sound menu or install pavucontrol for audio settings.",
        ),
        "accessibility" | "magnifier" => (
            &[&["gnome-control-center", "universal-access"], &["xfce4-accessibility-settings"]], "Look for accessibility options in the desktop settings.",
        ),
        "keyboard" => (
            &[&["rasputin"], &["gnome-control-center", "keyboard"], &["lxinput"]],
            "Look for Mouse and Keyboard.",
        ),
        "mouse" | "touchpad" => (
            &[&["rasputin"], &["gnome-control-center", "mouse"], &["lxinput"]],
            "Look for Mouse and Keyboard.",
        ),
        "power" | "power & battery" | "battery" => (
            &[&["xfce4-power-manager-settings"], &["mate-power-preferences"], &["gnome-control-center", "power"]], "Look for power and display options. Battery settings require a detectable battery.",
        ),
        "notifications" => (&[&["gnome-control-center", "notifications"], &["xfce4-notifyd-config"]], "Notification settings depend on the running desktop.") ,
        "storage" | "disks" => (&[&["gnome-disks"]], "Install gnome-disk-utility for a graphical storage view."),
        "date and time" | "date & time" | "time" | "time zone" => (&[&["rpcc", "localisation"], &["gnome-control-center", "datetime"], &["system-config-date"]], "Look for Localisation to change your time zone."),
        "windows update" | "software update" | "software updates" | "updates" => (
            SOFTWARE, "Use the desktop's software updater to review updates.",
        ),
        _ => return Err("Choose network, Bluetooth, display, audio, accessibility, keyboard, mouse, power, notifications, storage, date and time, or software updates.".into()),
    };
    if launch_apps(apps).is_ok() {
        return Ok(format!("Opened Linux settings for {page}."));
    }
    launch_apps(SETTINGS)?;
    Ok(format!("Opened system settings. {fallback}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_registered_app_has_a_linux_target() {
        for id in crate::desktop::APP_IDS {
            assert!(
                super::target(id).is_some(),
                "missing Linux launcher for {id}"
            );
        }
    }
}
