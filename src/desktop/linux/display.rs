use std::{env, fs};

use serde::Deserialize;

use super::command_text;

const UNAVAILABLE: &str = "Could not read display information from this desktop session.";
const BACKLIGHT_ERROR: &str = "Linux could not adjust the display backlight. Check brightnessctl and your device permissions.";
const DDC_ERROR: &str = "The display did not accept DDC/CI brightness control. Enable DDC/CI in the monitor settings and check ddcutil and I²C access.";

struct Monitor {
    name: String,
    enabled: bool,
    mode: Option<(u32, u32)>,
}

#[derive(Deserialize)]
struct Output {
    name: String,
    enabled: bool,
    modes: Vec<Mode>,
}

#[derive(Deserialize)]
struct Mode {
    width: u32,
    height: u32,
    current: bool,
}

pub(super) fn resolution() -> Result<String, String> {
    let displays = monitors()?;
    let modes = displays
        .iter()
        .filter(|monitor| monitor.enabled)
        .filter_map(|monitor| {
            let (width, height) = monitor.mode?;
            Some(format!("{} is {width} × {height} pixels", monitor.name))
        })
        .collect::<Vec<_>>();
    if modes.is_empty() {
        return Err("No active display reported its resolution.".into());
    }
    Ok(format!("{}.", modes.join(", ")))
}

pub(super) fn monitor_count() -> Result<String, String> {
    let count = monitors()?.len();
    Ok(match count {
        0 => "No connected displays reported.".into(),
        1 => "One display is connected.".into(),
        _ => format!("{count} displays are connected."),
    })
}

fn wayland() -> bool {
    env::var_os("WAYLAND_DISPLAY").is_some() || env::var_os("WAYLAND_SOCKET").is_some()
}

fn monitors() -> Result<Vec<Monitor>, &'static str> {
    if wayland() {
        if let Some(json) = command_text("wlr-randr", &["--json"]) {
            return parse_outputs(&json).ok_or(UNAVAILABLE);
        }
        return command_text("wlr-randr", &[])
            .map(|text| parse_wayland_text(&text))
            .ok_or(UNAVAILABLE);
    }
    command_text("xrandr", &["--query"])
        .map(|text| parse_xrandr(&text))
        .ok_or(UNAVAILABLE)
}

fn parse_outputs(json: &str) -> Option<Vec<Monitor>> {
    let outputs: Vec<Output> = serde_json::from_str(json).ok()?;
    Some(
        outputs
            .into_iter()
            .map(|output| Monitor {
                name: output.name,
                enabled: output.enabled,
                mode: output
                    .modes
                    .iter()
                    .find(|mode| mode.current)
                    .map(|mode| (mode.width, mode.height)),
            })
            .collect(),
    )
}

fn parse_wayland_text(text: &str) -> Vec<Monitor> {
    let mut monitors: Vec<Monitor> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if !line.starts_with(char::is_whitespace) {
            if let Some(name) = line.split_whitespace().next() {
                monitors.push(Monitor {
                    name: name.into(),
                    enabled: false,
                    mode: None,
                });
            }
        } else if let Some(monitor) = monitors.last_mut() {
            let line = line.trim();
            if line == "Enabled: yes" {
                monitor.enabled = true;
            } else if line.ends_with("current)") {
                monitor.mode = line.split_whitespace().next().and_then(dimensions);
            }
        }
    }
    monitors
}

fn parse_xrandr(text: &str) -> Vec<Monitor> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?;
            if words.next()? != "connected" {
                return None;
            }
            let mode = words.find_map(|word| {
                let (size, _) = word.split_once(['+', '-'])?;
                dimensions(size)
            });
            Some(Monitor {
                name: name.into(),
                enabled: mode.is_some(),
                mode,
            })
        })
        .collect()
}

fn dimensions(text: &str) -> Option<(u32, u32)> {
    let (width, height) = text.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn single_active(monitors: &[Monitor]) -> Result<&Monitor, &'static str> {
    let mut enabled = monitors.iter().filter(|monitor| monitor.enabled);
    let first = enabled.next().ok_or("No active display was found.")?;
    if enabled.next().is_some() {
        return Err(
            "Several displays are active. Choose the intended display in your desktop settings.",
        );
    }
    Ok(first)
}

pub(super) fn brightness(percent: u8) -> Result<String, String> {
    if percent > 100 {
        return Err("Choose a brightness from 0 to 100 percent.".into());
    }
    let monitors = monitors().ok();
    let active = monitors.as_deref().map(single_active).transpose()?;
    if active
        .is_none_or(|monitor| !monitor.name.starts_with("HDMI") && !monitor.name.starts_with("DP-"))
    {
        let backlights: Vec<_> = fs::read_dir("/sys/class/backlight")
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .collect();
        if let [device] = backlights.as_slice() {
            let name = device.file_name();
            let name = name.to_str().ok_or(BACKLIGHT_ERROR)?;
            command_text(
                "brightnessctl",
                &[
                    "--class=backlight",
                    "--device",
                    name,
                    "set",
                    &format!("{percent}%"),
                ],
            )
            .ok_or(BACKLIGHT_ERROR)?;
            return Ok(format!("Set the display backlight to {percent}%."));
        }
        if backlights.len() > 1 {
            return Err("Several backlights are available. Choose the intended display in your desktop settings.".into());
        }
    }

    let monitor = active.ok_or(
        "Open Amica in a desktop session with an active display to set monitor brightness.",
    )?;
    set_ddc_brightness(&monitor.name, percent)
}

fn set_ddc_brightness(connector: &str, percent: u8) -> Result<String, String> {
    let bus = drm_bus(connector).map(|bus| format!("--bus={bus}"));
    let selector = match bus {
        Some(bus) => bus,
        None => {
            let detected = command_text("ddcutil", &["detect", "--brief"]).ok_or(DDC_ERROR)?;
            let display = parse_ddc_display(&detected)?;
            format!("--display={display}")
        }
    };
    let read = || {
        let value = command_text("ddcutil", &[&selector, "--terse", "getvcp", "10"])?;
        parse_ddc_brightness(&value)
    };
    let (_, maximum) = read().ok_or(DDC_ERROR)?;
    let requested = u32::from(maximum) * u32::from(percent) / 100;
    command_text(
        "ddcutil",
        &[&selector, "setvcp", "10", &requested.to_string()],
    )
    .ok_or(DDC_ERROR)?;
    let actual = read().map_or(f64::from(percent), |(value, max)| {
        f64::from(value) * 100.0 / f64::from(max)
    });
    Ok(format!("Display brightness is {actual:.0}%."))
}

fn drm_bus(connector: &str) -> Option<u32> {
    fs::read_dir("/sys/class/drm")
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let name = entry.file_name();
            let (_, name) = name.to_str()?.split_once('-')?;
            if name != connector {
                return None;
            }
            if fs::read_to_string(entry.path().join("status")).ok()?.trim() != "connected" {
                return None;
            }
            let ddc = fs::read_link(entry.path().join("ddc")).ok()?;
            ddc.file_name()?
                .to_str()?
                .strip_prefix("i2c-")?
                .parse()
                .ok()
        })
}

fn parse_ddc_display(text: &str) -> Result<u32, &'static str> {
    let mut displays = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Display ")?.parse().ok());
    let first = displays.next().ok_or(DDC_ERROR)?;
    if displays.next().is_some() {
        return Err(
            "Several DDC/CI monitors were found. Use the intended monitor's brightness controls.",
        );
    }
    Ok(first)
}

fn parse_ddc_brightness(text: &str) -> Option<(u16, u16)> {
    let line = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("VCP 10 C "))?;
    let mut values = line.split_whitespace();
    let value = values.next()?.parse().ok()?;
    let max = values.next()?.parse().ok()?;
    (max > 0 && value <= max).then_some((value, max))
}

pub(super) fn magnifier(percent: u16) -> Result<String, String> {
    if !(100..=600).contains(&percent) {
        return Err("Choose a magnification from 100 to 600 percent.".into());
    }
    let factor = format!("{:.2}", f64::from(percent) / 100.0);
    if env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
        desktop
            .split(':')
            .any(|name| name.eq_ignore_ascii_case("gnome"))
    }) {
        let enabled = if percent == 100 { "false" } else { "true" };
        let error = "The GNOME desktop did not accept the requested magnification.";
        if percent > 100 {
            command_text(
                "gsettings",
                &[
                    "set",
                    "org.gnome.desktop.a11y.magnifier",
                    "mag-factor",
                    &factor,
                ],
            )
            .ok_or(error)?;
        }
        command_text(
            "gsettings",
            &[
                "set",
                "org.gnome.desktop.a11y.applications",
                "screen-magnifier-enabled",
                enabled,
            ],
        )
        .ok_or(error)?;
        return Ok(format!("Screen magnification is {percent}%."));
    }
    if !wayland() {
        return Err(
            "This desktop exposes magnification through its accessibility settings.".into(),
        );
    }
    let monitors = monitors()?;
    let monitor = single_active(&monitors)?;
    command_text(
        "wlr-randr",
        &["--output", &monitor.name, "--scale", &factor],
    )
    .ok_or("The compositor did not accept display scaling. Use Screens in the desktop settings.")?;
    Ok(format!(
        "Set display scaling to {percent}% on {}.",
        monitor.name
    ))
}
