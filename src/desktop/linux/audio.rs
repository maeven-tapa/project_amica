use std::process::Command;

use serde::Deserialize;
use serde_json::Value;

const DEFAULT: &str = "@DEFAULT_SINK@";
const UNAVAILABLE: &str = "The desktop audio service did not accept the command. Check PipeWire or PulseAudio and the requested device.";

#[derive(Deserialize)]
struct Sink {
    name: String,
    description: String,
}

#[derive(Deserialize)]
struct Stream {
    index: u32,
    properties: Value,
}

pub(super) fn set_volume(amount: i32, relative: bool) -> Result<String, String> {
    if amount.unsigned_abs() > 100 || (!relative && amount < 0) {
        return Err(
            "Choose a volume from 0 to 100 percent or an adjustment within 100 percentage points."
                .into(),
        );
    }
    let sink = "@DEFAULT_AUDIO_SINK@";
    if super::command_text("wpctl", &["get-volume", sink]).is_some() {
        let mut value = format!("{}%", amount.unsigned_abs());
        if relative {
            value.push(if amount < 0 { '-' } else { '+' });
        }
        super::command_text("wpctl", &["set-volume", "--limit", "1.0", sink, &value])
            .ok_or(UNAVAILABLE)?;
        let current = super::command_text("wpctl", &["get-volume", sink])
            .and_then(|text| text.split_whitespace().nth(1)?.parse::<f64>().ok())
            .map(|value| value * 100.0);
        return Ok(current.map_or_else(
            || "Updated the default audio output volume.".into(),
            |value| format!("System volume is {value:.0}%."),
        ));
    }

    let requested = if relative {
        (i32::from(sink_percent(DEFAULT).ok_or(UNAVAILABLE)?) + amount).clamp(0, 100)
    } else {
        amount
    };
    pactl(&["set-sink-volume", DEFAULT, &format!("{requested}%")])?;
    Ok(format!(
        "System volume is {}%.",
        sink_percent(DEFAULT).unwrap_or(requested as u16)
    ))
}

pub(super) fn app_volume(app: &str, percent: u8) -> Result<String, String> {
    validate_percent(percent)?;
    let stream = app_stream(app)?;
    pactl(&[
        "set-sink-input-volume",
        &stream.to_string(),
        &format!("{percent}%"),
    ])?;
    Ok(format!("Set {app} volume to {percent}%."))
}

pub(super) fn mute(app: &str) -> Result<String, String> {
    let stream = app_stream(app)?;
    pactl(&["set-sink-input-mute", &stream.to_string(), "1"])?;
    Ok(format!("Muted {app}."))
}

pub(super) fn select_output(name: &str, percent: Option<u8>) -> Result<String, String> {
    if let Some(percent) = percent {
        validate_percent(percent)?;
    }
    let sinks = sinks()?;
    let sink = find_sink(&sinks, name)?;
    pactl(&["set-default-sink", &sink.name])?;
    if let Some(percent) = percent {
        if pactl(&["set-sink-volume", &sink.name, &format!("{percent}%")]).is_err() {
            return Err(format!(
                "Selected {} as the default output. The volume change failed.",
                sink.description
            ));
        }
        return Ok(format!(
            "Default audio output is {} at {percent}%.",
            sink.description
        ));
    }
    Ok(format!("Default audio output is {}.", sink.description))
}

pub(super) fn default_output() -> Result<String, String> {
    use cpal::traits::HostTrait;

    let name = pactl(&["get-default-sink"])
        .ok()
        .and_then(|name| {
            sinks()
                .ok()?
                .into_iter()
                .find(|sink| sink.name == name)
                .map(|sink| sink.description)
        })
        .or_else(|| {
            cpal::default_host()
                .default_output_device()
                .as_ref()
                .map(crate::settings::name)
        })
        .ok_or("Linux did not report a default audio output.")?;
    Ok(format!("Default audio output is {name}."))
}

fn validate_percent(percent: u8) -> Result<(), &'static str> {
    (percent <= 100)
        .then_some(())
        .ok_or("Choose a volume from 0 to 100 percent.")
}

fn pactl(args: &[&str]) -> Result<String, &'static str> {
    let output = Command::new("pactl")
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "Install pulseaudio-utils to use Linux audio device and mixer commands."
            } else {
                UNAVAILABLE
            }
        })?;
    if !output.status.success() {
        return Err(UNAVAILABLE);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

fn sinks() -> Result<Vec<Sink>, &'static str> {
    serde_json::from_str(&pactl(&["--format=json", "list", "sinks"])?).map_err(|_| UNAVAILABLE)
}

fn sink_percent(sink: &str) -> Option<u16> {
    let output = pactl(&["--format=json", "get-sink-volume", sink]).ok()?;
    let value: Value = serde_json::from_str(&output).ok()?;
    value
        .as_object()?
        .values()
        .filter_map(|channel| {
            channel["value_percent"]
                .as_str()?
                .strip_suffix('%')?
                .trim()
                .parse()
                .ok()
        })
        .max()
}

fn find_sink<'a>(sinks: &'a [Sink], name: &str) -> Result<&'a Sink, String> {
    let name = name.trim();
    let mut matches = sinks
        .iter()
        .filter(|sink| {
            sink.name.eq_ignore_ascii_case(name) || sink.description.eq_ignore_ascii_case(name)
        })
        .peekable();
    if matches.peek().is_some() {
        return unique_sink(matches);
    }

    let words = words(name);
    let matching = sinks.iter().filter(|sink| {
        let description = self::words(&sink.description);
        !words.is_empty() && words.iter().all(|word| description.contains(word))
    });
    unique_sink(matching).map_err(|error| {
        if sinks.is_empty() {
            return "There are no available audio outputs.".into();
        }
        let options = sinks
            .iter()
            .take(8)
            .map(|sink| sink.description.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{error} Available outputs are {options}.")
    })
}

fn unique_sink<'a>(mut matches: impl Iterator<Item = &'a Sink>) -> Result<&'a Sink, String> {
    let sink = matches.next().ok_or("No audio output matches that name.")?;
    if matches.next().is_some() {
        return Err("Several audio outputs match. Use the full device name.".into());
    }
    Ok(sink)
}

fn words(name: &str) -> Vec<String> {
    name.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn app_stream(app: &str) -> Result<u32, &'static str> {
    let output = pactl(&["--format=json", "list", "sink-inputs"])?;
    let streams: Vec<Stream> = serde_json::from_str(&output).map_err(|_| UNAVAILABLE)?;
    select_stream(&streams, app)
}

fn select_stream(streams: &[Stream], app: &str) -> Result<u32, &'static str> {
    let app = app.trim().to_lowercase();
    let aliases: &[&str] = match app.as_str() {
        "edge" | "microsoft edge" => &["microsoft edge", "microsoft-edge", "msedge"],
        "chrome" | "google chrome" => &["google chrome", "google-chrome", "chrome"],
        "firefox" | "mozilla firefox" => &["mozilla firefox", "firefox", "firefox-esr"],
        "vlc" => &["vlc", "vlc media player", "org.videolan.vlc"],
        _ => &[&app],
    };
    let mut matches = streams.iter().filter(|stream| {
        !app.is_empty()
            && [
                "application.name",
                "application.process.binary",
                "application.id",
            ]
            .iter()
            .any(|key| {
                stream.properties[key].as_str().is_some_and(|value| {
                    aliases
                        .iter()
                        .any(|alias| value.eq_ignore_ascii_case(alias))
                })
            })
    });
    let stream = matches
        .next()
        .ok_or("There is no active audio stream for that app.")?;
    if matches.next().is_some() {
        return Err(
            "More than one audio stream matches that app. Open the volume mixer to choose a stream.",
        );
    }
    Ok(stream.index)
}
