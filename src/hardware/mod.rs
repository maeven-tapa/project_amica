#[cfg(windows)]
mod windows;

use serde::Serialize;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{AmicaWindow, HardwareRow};

#[derive(Serialize)]
pub struct Spec {
    label: &'static str,
    value: String,
}

pub fn scan() -> Result<Vec<Spec>, &'static str> {
    #[cfg(windows)]
    {
        windows::scan()
    }
    #[cfg(not(windows))]
    {
        Err("System specs scanning is available on Windows.")
    }
}

pub fn connect(window: &AmicaWindow) {
    let owner = window.as_weak();
    window.on_scan_hardware(move || {
        let Some(window) = owner.upgrade() else {
            return;
        };
        if window.get_hardware_scanning() {
            return;
        }
        window.set_hardware_scanning(true);
        window.set_hardware_error("".into());
        let target = owner.clone();
        let started = std::thread::Builder::new()
            .name("amica-hardware-scan".into())
            .spawn(move || {
                let result = scan();
                let _ = target.upgrade_in_event_loop(move |window| {
                    window.set_hardware_scanning(false);
                    match result {
                        Ok(specs) => {
                            let rows = specs.into_iter().map(|spec| HardwareRow {
                                label: spec.label.into(),
                                value: spec.value.into(),
                            });
                            window.set_hardware_rows(ModelRc::new(VecModel::from_iter(rows)));
                        }
                        Err(error) => window.set_hardware_error(error.into()),
                    }
                });
            });
        if started.is_err() {
            window.set_hardware_scanning(false);
            window.set_hardware_error("Amica could not start the hardware scan.".into());
        }
    });
}
