mod audio;
mod display;
mod launch;
#[cfg(test)]
mod tests;

use std::{fs, path::PathBuf, process::Command};

use crate::actions::command::SystemAction;

use super::PowerAction;

pub(super) use launch::{open, open_folder, open_page};

fn command_text(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().into())
}

pub(crate) fn execute(action: &SystemAction) -> Result<String, String> {
    match action {
        SystemAction::Volume(amount, relative) => audio::set_volume(*amount, *relative),
        SystemAction::AppVolume(app, percent) => audio::app_volume(app, *percent),
        SystemAction::Output(name, percent) => audio::select_output(name, *percent),
        SystemAction::Mute(app) => audio::mute(app),
        SystemAction::Brightness(percent) => display::brightness(*percent),
        SystemAction::Settings(page) => launch::settings(page),
        SystemAction::Magnifier(percent) => display::magnifier(*percent),
        SystemAction::Metric(name) => metric(name),
    }
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

fn batteries() -> impl Iterator<Item = PathBuf> {
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

fn metric(name: &str) -> Result<String, String> {
    match name.trim().to_ascii_lowercase().as_str() {
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
        "kasalukuyang pinagmumulan ng kuryente" | "power source" => {
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
        "default na audio output" | "default audio output" => audio::default_output(),
        "display resolution" => display::resolution(),
        "number of connected monitors" => display::monitor_count(),
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
