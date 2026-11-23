use std::ptr;

use windows::{
    Win32::{
        Media::Audio::{
            Endpoints::IAudioEndpointVolume, IAudioSessionControl2, IAudioSessionManager2,
            IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eMultimedia,
            eRender,
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance},
    },
    core::Interface,
};

use crate::actions::command::SystemAction;

use super::{
    external::process_name,
    windows::{ComApartment, open_protocol},
};

const AUDIO_ERROR: &str = "Windows did not expose the requested audio control.";

unsafe fn endpoint() -> windows::core::Result<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)
    }
}

pub fn execute(
    action: &SystemAction,
    target: Option<super::TargetWindow>,
) -> Result<String, String> {
    let _apartment = ComApartment::initialize()?;
    match action {
        SystemAction::Volume(value, relative) => {
            let endpoint = unsafe { endpoint() }.map_err(|_| AUDIO_ERROR)?;
            let volume: IAudioEndpointVolume =
                unsafe { endpoint.Activate(CLSCTX_ALL, None) }.map_err(|_| AUDIO_ERROR)?;
            let current =
                unsafe { volume.GetMasterVolumeLevelScalar() }.map_err(|_| AUDIO_ERROR)?;
            let requested = if *relative {
                current + *value as f32 / 100.0
            } else {
                *value as f32 / 100.0
            };
            if !*relative && !(0.0..=1.0).contains(&requested) {
                return Err("Choose a volume from 0 to 100 percent.".into());
            }
            unsafe { volume.SetMasterVolumeLevelScalar(requested.clamp(0.0, 1.0), ptr::null()) }
                .map_err(|_| AUDIO_ERROR)?;
            let actual = unsafe { volume.GetMasterVolumeLevelScalar() }.map_err(|_| AUDIO_ERROR)?;
            Ok(format!("Windows volume is {:.0}%.", actual * 100.0))
        }
        SystemAction::AppVolume(app, percent) => {
            if *percent > 100 {
                return Err("Choose a volume from 0 to 100 percent.".into());
            }
            let volume = session(app)?;
            unsafe { volume.SetMasterVolume(f32::from(*percent) / 100.0, ptr::null()) }
                .map_err(|_| AUDIO_ERROR)?;
            let actual = unsafe { volume.GetMasterVolume() }.map_err(|_| AUDIO_ERROR)?;
            Ok(format!("{app} volume is {:.0}%.", actual * 100.0))
        }
        SystemAction::Mute(app) => {
            let volume = session(app)?;
            unsafe { volume.SetMute(true, ptr::null()) }.map_err(|_| AUDIO_ERROR)?;
            Ok(format!("Muted {app}."))
        }
        SystemAction::Settings(page) => {
            let page_id = match page.to_lowercase().as_str() {
                "wi-fi" | "wifi" | "network" => "network-wifi",
                "bluetooth" => "bluetooth",
                "display" | "resolution" | "display scaling" | "scaling" | "orientation"
                | "brightness" | "night light" => "display",
                "sound" | "volume" | "audio" => "sound",
                "volume mixer" | "mixer" => "apps-volume",
                "accessibility" => "easeofaccess",
                "keyboard" => "easeofaccess-keyboard",
                "mouse" => "mousetouchpad",
                "power" | "power & battery" => "powersleep",
                "notifications" => "notifications",
                "storage" => "storagesense",
                "windows update" => "windowsupdate",
                _ => return Err("That Windows settings page is unavailable in Amica.".into()),
            };
            open_protocol(&format!("ms-settings:{page_id}"))?;
            Ok(format!("Opened Windows settings for {page}."))
        }
        SystemAction::Brightness(percent) => brightness(target, *percent),
        SystemAction::Magnifier(percent) => magnifier(*percent),
        SystemAction::Output(name, percent) => {
            open_protocol("ms-settings:sound")?;
            Ok(if let Some(percent) = percent {
                format!("Windows Sound is open. Choose {name} and set it to {percent}% there.")
            } else {
                format!("Windows Sound is open. Choose {name} under Output.")
            })
        }
        SystemAction::Metric(name) => metric(name),
    }
}

fn metric(name: &str) -> Result<String, String> {
    use windows_sys::Win32::{
        System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
    };
    match name {
        "battery percentage" | "kasalukuyang pinagmumulan ng kuryente" => {
            let mut status = SYSTEM_POWER_STATUS::default();
            if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
                return Err("Windows could not read the power status.".into());
            }
            if name == "battery percentage" {
                return if status.BatteryLifePercent <= 100 {
                    Ok(format!("Battery is {}%.", status.BatteryLifePercent))
                } else {
                    Err("Windows has no battery percentage for this PC.".into())
                };
            }
            Ok(match status.ACLineStatus {
                0 => "This PC is running on battery.",
                1 => "This PC is connected to AC power.",
                _ => "Windows did not report the power source.",
            }
            .into())
        }
        "display resolution" => Ok(format!(
            "Primary display: {} × {} pixels.",
            unsafe { GetSystemMetrics(SM_CXSCREEN) },
            unsafe { GetSystemMetrics(SM_CYSCREEN) }
        )),
        "number of connected monitors" => Ok(format!(
            "Windows reports {} connected displays.",
            super::layout::monitors().len()
        )),
        "default na audio output" => {
            let device = cpal::traits::HostTrait::default_output_device(&cpal::default_host())
                .ok_or("Windows did not report a default audio output.")?;
            Ok(format!(
                "Default Windows audio output: {}.",
                crate::settings::name(&device)
            ))
        }
        "free disk space" => {
            use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
            let directory = std::env::var("SystemRoot")
                .map_err(|_| "Windows did not report the system drive.")?;
            let mut available = 0;
            unsafe {
                GetDiskFreeSpaceExW(
                    &windows::core::HSTRING::from(directory),
                    Some(&mut available),
                    None,
                    None,
                )
            }
            .map_err(|_| "Windows did not report the free system drive space.")?;
            Ok(format!(
                "System drive free space: {:.1} GiB.",
                available as f64 / 1_073_741_824.0
            ))
        }
        metric => {
            let (label, key, unit) = match metric {
                "cpu usage" | "gamit ng cpu" => ("CPU usage", "cpu_percent_of_pc_capacity", "%"),
                "memory usage" | "ram usage" | "gamit ng memory" => {
                    ("RAM usage", "memory_percent_of_pc_ram", "%")
                }
                "download rate" | "bilis ng download" => {
                    ("Download rate", "download_mbps", " Mbps")
                }
                "upload rate" | "bilis ng upload" => ("Upload rate", "upload_mbps", " Mbps"),
                _ => return Err("The requested metric is unavailable.".into()),
            };
            let report = crate::usage::read(crate::usage::Scope::Computer)?;
            let value = report[key]
                .as_f64()
                .ok_or("Windows did not return that measurement yet.")?;
            Ok(format!("{label}: {value:.1}{unit}."))
        }
    }
}

fn brightness(target: Option<super::TargetWindow>, percent: u8) -> Result<String, String> {
    use windows_sys::Win32::{
        Devices::Display::{
            DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
            GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, SetMonitorBrightness,
        },
        Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromWindow},
    };
    const ERROR: &str =
        "The selected display did not expose brightness control. Try Windows Display settings.";
    if percent > 100 {
        return Err("Choose a brightness from 0 to 100 percent.".into());
    }
    let target = target.ok_or("Select a window on the desired display first.")?;
    let monitor = unsafe { MonitorFromWindow(target.handle as _, MONITOR_DEFAULTTONULL) };
    let mut count = 0;
    if monitor.is_null()
        || unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count) } == 0
        || count != 1
    {
        return Err(ERROR.into());
    }
    let mut devices = [PHYSICAL_MONITOR::default()];
    if unsafe { GetPhysicalMonitorsFromHMONITOR(monitor, count, devices.as_mut_ptr()) } == 0 {
        return Err(ERROR.into());
    }
    let handle = devices[0].hPhysicalMonitor;
    let (mut minimum, mut current, mut maximum) = (0, 0, 0);
    let supported =
        unsafe { GetMonitorBrightness(handle, &mut minimum, &mut current, &mut maximum) } != 0
            && maximum > minimum;
    let changed = supported
        && unsafe {
            SetMonitorBrightness(
                handle,
                minimum + ((u64::from(maximum - minimum) * u64::from(percent)) / 100) as u32,
            )
        } != 0;
    let actual = changed
        && unsafe { GetMonitorBrightness(handle, &mut minimum, &mut current, &mut maximum) } != 0
        && maximum > minimum;
    unsafe { DestroyPhysicalMonitors(count, devices.as_ptr()) };
    if !actual {
        return Err(ERROR.into());
    }
    Ok(format!(
        "Display brightness is {:.0}%.",
        f64::from(current.saturating_sub(minimum)) * 100.0 / f64::from(maximum - minimum)
    ))
}

fn magnifier(percent: u16) -> Result<String, String> {
    use windows_sys::Win32::UI::Magnification::{
        MagGetFullscreenTransform, MagInitialize, MagSetFullscreenTransform, MagUninitialize,
    };
    if !(100..=600).contains(&percent) {
        return Err("Choose a magnification from 100 to 600 percent.".into());
    }
    if unsafe { MagInitialize() } == 0 {
        return Err("Windows Magnifier is unavailable.".into());
    }
    let mut actual = 1.0;
    let (mut x, mut y) = (0, 0);
    let changed = unsafe {
        MagSetFullscreenTransform(f32::from(percent) / 100.0, 0, 0) != 0
            && MagGetFullscreenTransform(&mut actual, &mut x, &mut y) != 0
    };
    unsafe { MagUninitialize() };
    if !changed {
        return Err("Windows did not accept the requested magnification.".into());
    }
    Ok(format!("Windows magnification is {:.0}%.", actual * 100.0))
}

fn session(app: &str) -> Result<ISimpleAudioVolume, &'static str> {
    let normalized = app.trim().to_lowercase();
    let executable = match normalized.as_str() {
        "microsoft edge" | "edge" => "msedge",
        "media player" | "windows media player" => "wmplayer",
        "google chrome" => "chrome",
        "mozilla firefox" => "firefox",
        _ => &normalized,
    };
    let endpoint = unsafe { endpoint() }.map_err(|_| AUDIO_ERROR)?;
    let manager: IAudioSessionManager2 =
        unsafe { endpoint.Activate(CLSCTX_ALL, None) }.map_err(|_| AUDIO_ERROR)?;
    let sessions = unsafe { manager.GetSessionEnumerator() }.map_err(|_| AUDIO_ERROR)?;
    let count = unsafe { sessions.GetCount() }.map_err(|_| AUDIO_ERROR)?;
    let mut matching = (0..count).filter_map(|index| unsafe {
        let session: IAudioSessionControl2 = sessions.GetSession(index).ok()?.cast().ok()?;
        (process_name(session.GetProcessId().ok()?) == executable).then_some(session)
    });
    let selected = matching
        .next()
        .ok_or("There is no active audio session for that app.")?;
    if matching.next().is_some() {
        return Err("More than one audio session matches that app.");
    }
    selected.cast().map_err(|_| AUDIO_ERROR)
}
