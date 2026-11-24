use slint::ComponentHandle;

use crate::AmicaWindow;

pub const WINDOW_MODE_IDS: &[&str] = &[
    "fullscreen",
    "windowed",
    "toggle_fullscreen",
    "maximize",
    "restore",
    "minimize",
    "show",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    Fullscreen,
    Windowed,
    ToggleFullscreen,
    Maximize,
    Restore,
    Minimize,
    Show,
}

impl WindowAction {
    pub fn parse(id: &str) -> Option<Self> {
        Some(match id {
            "fullscreen" => Self::Fullscreen,
            "windowed" => Self::Windowed,
            "toggle_fullscreen" => Self::ToggleFullscreen,
            "maximize" => Self::Maximize,
            "restore" => Self::Restore,
            "minimize" => Self::Minimize,
            "show" => Self::Show,
            _ => return None,
        })
    }

    pub fn apply(self, window: &AmicaWindow) {
        if self == Self::Show {
            window.window().set_minimized(false);
            #[cfg(windows)]
            super::external::show_self(window);
            return;
        }
        let window = window.window();
        match self {
            Self::Fullscreen => {
                window.set_minimized(false);
                window.set_fullscreen(true);
            }
            Self::Windowed => window.set_fullscreen(false),
            Self::ToggleFullscreen => window.set_fullscreen(!window.is_fullscreen()),
            Self::Maximize => {
                window.set_minimized(false);
                window.set_fullscreen(false);
                window.set_maximized(true);
            }
            Self::Restore => {
                window.set_fullscreen(false);
                window.set_maximized(false);
                window.set_minimized(false);
            }
            Self::Minimize => window.set_minimized(true),
            Self::Show => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalAction {
    Fullscreen,
    Restore,
    Minimize,
    Maximize,
    Close,
}

impl ExternalAction {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "fullscreen" => Self::Fullscreen,
            "restore" | "windowed" => Self::Restore,
            "minimize" => Self::Minimize,
            "maximize" => Self::Maximize,
            "close" => Self::Close,
            _ => return None,
        })
    }

    pub fn apply(self, target: TargetWindow) -> Result<(), &'static str> {
        #[cfg(windows)]
        {
            super::external::apply(target, self)
        }
        #[cfg(not(windows))]
        {
            let _ = (self, target);
            Err("External window controls are available on Windows.")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TargetWindow {
    pub(super) handle: usize,
    pub(super) process: u32,
}

impl TargetWindow {
    pub fn find(name: &str) -> Result<Self, &'static str> {
        #[cfg(windows)]
        {
            super::external::find(name)
        }
        #[cfg(not(windows))]
        {
            let _ = name;
            Err("External window controls are available on Windows.")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Shutdown,
    Restart,
    Sleep,
}

impl PowerAction {
    pub fn run(self) -> Result<(), &'static str> {
        #[cfg(windows)]
        {
            super::windows::power(self)
        }
        #[cfg(target_os = "linux")]
        {
            super::linux::power(self)
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Err("PC power commands are unavailable on this platform.")
        }
    }
}
