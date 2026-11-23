use cpal::{
    Device,
    traits::{DeviceTrait, HostTrait},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AmicaWindow, storage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioKind {
    Microphone,
    Speaker,
}

impl AudioKind {
    pub fn parse(kind: &str) -> Option<Self> {
        match kind {
            "microphone" => Some(Self::Microphone),
            "speaker" => Some(Self::Speaker),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::Speaker => "speaker",
        }
    }

    pub fn input(self) -> bool {
        self == Self::Microphone
    }
}

#[derive(Serialize)]
pub struct AudioOption {
    pub id: String,
    pub name: String,
}

pub fn initialize(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    )
}

pub(crate) fn read(db: &Connection, key: &str) -> rusqlite::Result<String> {
    db.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .optional()
    .map(Option::unwrap_or_default)
}

pub(crate) fn save(db: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn search_enabled(db: &Connection) -> bool {
    read(db, "google_search").is_ok_and(|setting| setting == "on")
}

fn devices(kind: AudioKind) -> Vec<Device> {
    let host = cpal::default_host();
    match kind {
        AudioKind::Microphone => host
            .input_devices()
            .map(Iterator::collect)
            .unwrap_or_default(),
        AudioKind::Speaker => host
            .output_devices()
            .map(Iterator::collect)
            .unwrap_or_default(),
    }
}

pub fn device(kind: AudioKind, id: &str) -> Option<Device> {
    if !id.is_empty()
        && let Some(device) = devices(kind).into_iter().find(|device| {
            device
                .id()
                .is_ok_and(|device_id| device_id.to_string() == id)
        })
    {
        return Some(device);
    }
    let host = cpal::default_host();
    match kind {
        AudioKind::Microphone => host.default_input_device(),
        AudioKind::Speaker => host.default_output_device(),
    }
}

pub fn name(device: &Device) -> String {
    device
        .description()
        .map(|device| device.name().to_owned())
        .unwrap_or_else(|_| "Unknown device".into())
}

pub fn options(kind: AudioKind) -> Vec<AudioOption> {
    let mut options = vec![AudioOption {
        id: String::new(),
        name: "System default".into(),
    }];
    for device in devices(kind) {
        if let Ok(id) = device.id() {
            let base = name(&device);
            let mut label = base.clone();
            let mut number = 2;
            while options.iter().any(|option| option.name == label) {
                label = format!("{base} ({number})");
                number += 1;
            }
            options.push(AudioOption {
                id: id.to_string(),
                name: label,
            });
        }
    }
    options
}

pub fn resolve(kind: AudioKind, requested: &str) -> Result<String, &'static str> {
    if requested.trim().is_empty() {
        return Err("Name the device or choose System default.");
    }
    if requested.eq_ignore_ascii_case("default") {
        return Ok(String::new());
    }
    options(kind)
        .into_iter()
        .find(|device| device.id == requested || device.name.eq_ignore_ascii_case(requested))
        .map(|device| device.id)
        .ok_or("That audio device is not available. List the connected devices to find its name.")
}

pub fn current(db: &Connection, kind: AudioKind) -> Option<String> {
    let saved = read(db, kind.key()).unwrap_or_default();
    device(kind, &saved).as_ref().map(name)
}

pub fn connect(window: &AmicaWindow) {
    let db = storage::open().ok().filter(|db| initialize(db).is_ok());
    if let Some(db) = &db {
        window.set_input_device_id(read(db, "microphone").unwrap_or_default().into());
        window.set_output_device_id(read(db, "speaker").unwrap_or_default().into());
        window.set_google_search_enabled(search_enabled(db));
        for key in [
            "reply_language",
            "reply_length",
            "reply_style",
            "captions",
            "reduced_motion",
        ] {
            let value = read(db, key).unwrap_or_default();
            if !value.is_empty() {
                set_conversation(window, key, &value);
            }
        }
    }
    let weak = window.as_weak();
    window.on_conversation_setting(move |key, value| {
        if let Some(window) = weak.upgrade()
            && write_setting(&window, &key, &value)
        {
            set_conversation(&window, &key, &value);
        }
    });
    let weak = window.as_weak();
    window.on_refresh_settings(move || {
        if let Some(window) = weak.upgrade() {
            refresh(&window);
        }
    });

    let weak = window.as_weak();
    window.on_select_audio_device(move |input, requested| {
        let Some(window) = weak.upgrade() else { return };
        let kind = if input {
            AudioKind::Microphone
        } else {
            AudioKind::Speaker
        };
        let id = match resolve(kind, &requested) {
            Ok(id) => id,
            Err(error) => {
                window.set_settings_error(error.into());
                refresh(&window);
                return;
            }
        };
        if write_setting(&window, kind.key(), &id) {
            let chosen = if input {
                window.get_input_device_id()
            } else {
                window.get_output_device_id()
            };
            if chosen != id {
                if input {
                    window.set_input_device_id(id.into());
                } else {
                    window.set_output_device_id(id.into());
                }
                window.invoke_apply_audio_device(input);
            }
        }
        refresh(&window);
    });

    let weak = window.as_weak();
    window.on_enable_google_search(move |enabled| {
        let Some(window) = weak.upgrade() else { return };
        if write_setting(&window, "google_search", if enabled { "on" } else { "off" }) {
            window.set_google_search_enabled(enabled);
        }
    });
}

fn set_conversation(window: &AmicaWindow, key: &str, value: &str) {
    match key {
        "reply_language" => window.set_reply_language(value.into()),
        "reply_length" => window.set_reply_length(value.into()),
        "reply_style" => window.set_reply_style(value.into()),
        "captions" => window.set_show_captions(value == "on"),
        "reduced_motion" => window.set_reduced_motion(value == "on"),
        _ => (),
    }
}

fn write_setting(window: &AmicaWindow, key: &str, value: &str) -> bool {
    let saved = storage::open()
        .is_ok_and(|db| initialize(&db).and_then(|()| save(&db, key, value)).is_ok());
    window.set_settings_error(
        if saved {
            ""
        } else {
            "Amica could not save this setting."
        }
        .into(),
    );
    saved
}

fn refresh(window: &AmicaWindow) {
    for kind in [AudioKind::Microphone, AudioKind::Speaker] {
        let id = if kind.input() {
            window.get_input_device_id()
        } else {
            window.get_output_device_id()
        };
        let options = options(kind);
        let index = options
            .iter()
            .position(|option| id == option.id)
            .unwrap_or(0) as i32;
        let model: Vec<_> = options
            .into_iter()
            .map(|option| SharedString::from(option.name))
            .collect();
        let model = ModelRc::new(VecModel::from(model));
        let current = device(kind, &id)
            .as_ref()
            .map(name)
            .unwrap_or_else(|| "No device detected".into());
        if kind.input() {
            window.set_microphone_options(model);
            window.set_microphone_selection(index);
            window.set_current_microphone(current.into());
        } else {
            window.set_speaker_options(model);
            window.set_speaker_selection(index);
            window.set_current_speaker(current.into());
        }
    }
}
