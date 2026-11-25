use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use super::{SystemAction, execute, open, power};
use crate::{actions::catalog, desktop::PowerAction};

const FIXTURE: &str = r#"#!/bin/sh
tool=${0##*/}
printf '%s\n' "$tool $*" >> "$HOME/commands"
case "$tool:$*" in
    'wpctl:get-volume @DEFAULT_AUDIO_SINK@')
        [ ! -f "$HOME/use-pactl" ] || exit 1
        printf 'Volume: 0.42\n' ;;
    'pactl:--format=json list sinks')
        printf '%s\n' '[{"name":"alsa_output.hdmi","description":"HDMI Monitor"},{"name":"alsa_output.usb","description":"USB Headset"},{"name":"alsa_output.alt","description":"USB Speakers"}]' ;;
    'pactl:--format=json list sink-inputs')
        printf '%s\n' '[{"index":21,"properties":{"application.name":"Firefox","application.process.binary":"firefox-esr"}},{"index":22,"properties":{"application.name":"Discord"}},{"index":24,"properties":{"application.name":"Discord"}}]' ;;
    'pactl:get-default-sink') printf 'alsa_output.hdmi\n' ;;
    'pactl:--format=json get-sink-volume @DEFAULT_SINK@')
        value=55
        if [ -f "$HOME/volume" ]; then read -r value < "$HOME/volume"; fi
        printf '{"front-left":{"value_percent":"%s%%"}}\n' "$value" ;;
    'pactl:set-sink-volume @DEFAULT_SINK@ '*) printf '%s\n' "${3%?}" > "$HOME/volume" ;;
    'wlr-randr:--json')
        printf '%s\n' '[{"name":"HDMI-A-1","enabled":true,"modes":[{"width":1920,"height":1080,"current":true}]},{"name":"HDMI-A-2","enabled":false,"modes":[]}]' ;;
    'ddcutil:detect --brief') printf 'Display 1\n' ;;
    'ddcutil:'*' --terse getvcp 10') printf 'VCP 10 C 70 200\n' ;;
esac
"#;

#[test]
fn launch_and_system_commands_reach_the_linux_desktop_tools() {
    if let Some(home) = env::var_os("AMICA_TEST_COMMAND_MATRIX") {
        run_matrix(Path::new(&home));
        return;
    }

    let home = env::temp_dir().join(format!(
        "amica-desktop-test-{}-{}",
        std::process::id(),
        crate::local_time::now_ms()
    ));
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for tool in [
        "gio",
        "galculator",
        "mousepad",
        "mtpaint",
        "gpicview",
        "gnome-clocks",
        "gui-screenshot",
        "chromium",
        "rp-prefapps",
        "rpcc",
        "pavucontrol",
        "wpctl",
        "pactl",
        "wlr-randr",
        "ddcutil",
        "systemctl",
    ] {
        let path = bin.join(tool);
        fs::write(&path, FIXTURE).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let output = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "desktop::linux::tests::launch_and_system_commands_reach_the_linux_desktop_tools",
            "--nocapture",
        ])
        .env("AMICA_TEST_COMMAND_MATRIX", &home)
        .env("HOME", &home)
        .env("PATH", &bin)
        .env("WAYLAND_DISPLAY", "amica-test")
        .env("XDG_CURRENT_DESKTOP", "labwc")
        .output()
        .unwrap();
    fs::remove_dir_all(&home).unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

fn run_matrix(home: &Path) {
    use crate::actions::command::Command as Local;

    for (phrase, id) in [
        ("Open Spotify", "spotify"),
        ("Open YouTube", "youtube"),
        ("Open browser", "browser"),
        ("Launch calculator", "calculator"),
        ("Open text editor", "notepad"),
        ("Open paint", "paint"),
        ("Open photos", "photos"),
        ("Open clock", "clock"),
        ("Open screenshot tool", "snipping_tool"),
        ("Open Chromium", "edge"),
        ("Open recommended software", "microsoft_store"),
        ("Open file manager", "file_explorer"),
        ("Open system settings", "settings"),
    ] {
        assert_eq!(
            catalog::parse(phrase),
            Some(Local::Launch(id.into())),
            "{phrase}"
        );
        assert!(crate::actions::dispatcher::available(&Local::Launch(
            id.into()
        )));
        open(id).unwrap();
    }

    let run = |phrase: &str| {
        let Some(Local::System(action)) = catalog::parse(phrase) else {
            panic!("unrecognized system command {phrase}")
        };
        assert!(crate::actions::dispatcher::available(&Local::System(
            action.clone()
        )));
        execute(&action)
    };
    assert_eq!(
        run("Set system volume to 42%").unwrap(),
        "System volume is 42%."
    );
    assert!(run("Adjust master volume down 5 percentage points").is_ok());
    fs::write(home.join("use-pactl"), "").unwrap();
    assert_eq!(
        run("Adjust system volume up 8 percentage points").unwrap(),
        "System volume is 63%."
    );
    assert_eq!(
        run("Set Firefox volume to 30%").unwrap(),
        "Set Firefox volume to 30%."
    );
    assert_eq!(run("Mute Firefox in the mixer").unwrap(), "Muted Firefox.");
    assert!(
        run("Mute Discord in the mixer")
            .unwrap_err()
            .contains("More than one")
    );
    assert!(
        run("Switch audio output to USB")
            .unwrap_err()
            .contains("Several")
    );
    assert_eq!(
        run("Switch audio output to HDMI").unwrap(),
        "Default audio output is HDMI Monitor."
    );
    assert_eq!(
        run("Switch to USB Headset at 35% volume").unwrap(),
        "Default audio output is USB Headset at 35%."
    );
    assert_eq!(
        run("Show default audio output").unwrap(),
        "Default audio output is HDMI Monitor."
    );
    assert_eq!(
        run("Show display resolution").unwrap(),
        "HDMI-A-1 is 1920 × 1080 pixels."
    );
    assert_eq!(
        run("Show number of connected monitors").unwrap(),
        "2 displays are connected."
    );
    assert!(run("Set brightness to 35%").is_ok());
    assert_eq!(
        run("Set magnifier to 150%").unwrap(),
        "Set display scaling to 150% on HDMI-A-1."
    );
    for page in [
        "Wi-Fi",
        "Bluetooth",
        "display",
        "sound",
        "battery",
        "notifications",
        "keyboard",
        "mouse",
        "date and time",
        "accessibility",
        "storage",
        "software updates",
    ] {
        let result = run(&format!("Open settings for {page}"));
        assert!(
            result
                .as_ref()
                .is_ok_and(|message| message.starts_with("Opened")),
            "{page}: {result:?}"
        );
    }
    for metric in [
        "CPU usage",
        "RAM usage",
        "download rate",
        "upload rate",
        "free disk space",
        "power source",
    ] {
        assert!(run(&format!("Show {metric}")).is_ok(), "{metric}");
    }
    if let Err(message) = run("Show battery percentage") {
        assert_eq!(message, "Linux did not report a battery percentage.");
    }
    assert!(execute(&SystemAction::Brightness(101)).is_err());
    assert!(execute(&SystemAction::Magnifier(601)).is_err());
    assert!(execute(&SystemAction::Output("USB Headset".into(), Some(101))).is_err());

    for (phrase, id, action) in [
        ("Shut down this computer", "shutdown", PowerAction::Shutdown),
        ("Reboot this computer", "restart", PowerAction::Restart),
        ("Put this computer to sleep", "sleep", PowerAction::Sleep),
    ] {
        assert_eq!(catalog::parse(phrase), Some(Local::Power(id.into())));
        power(action).unwrap();
    }

    let finish = Instant::now() + Duration::from_secs(3);
    for expected in [
        "gio open -- https://open.spotify.com/",
        "gio open -- https://www.youtube.com/",
        "gio open -- https://www.google.com/",
        "galculator ",
        "mousepad ",
        "mtpaint ",
        "gpicview ",
        "gnome-clocks ",
        "gui-screenshot ",
        "chromium ",
        "rp-prefapps ",
        "rpcc ",
        "pavucontrol ",
        "rpcc localisation",
        "wpctl set-volume --limit 1.0 @DEFAULT_AUDIO_SINK@ 5%-",
        "pactl set-sink-volume @DEFAULT_SINK@ 63%",
        "pactl set-sink-input-volume 21 30%",
        "pactl set-sink-input-mute 21 1",
        "pactl set-default-sink alsa_output.hdmi",
        "pactl set-default-sink alsa_output.usb",
        "pactl set-sink-volume alsa_output.usb 35%",
        "setvcp 10 70",
        "wlr-randr --output HDMI-A-1 --scale 1.50",
        "systemctl poweroff",
        "systemctl reboot",
        "systemctl suspend",
    ] {
        loop {
            let commands = fs::read_to_string(home.join("commands")).unwrap();
            if commands.lines().any(|line| {
                line == expected || (expected.starts_with("setvcp") && line.ends_with(expected))
            }) {
                break;
            }
            assert!(
                Instant::now() < finish,
                "missing {expected:?} in {commands}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
