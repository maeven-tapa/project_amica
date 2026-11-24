use serde::{Deserialize, Serialize};

use crate::desktop::ExternalAction;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn parse(direction: &str) -> Option<Self> {
        Some(match direction.to_lowercase().as_str() {
            "left" | "pakaliwa" | "kaliwa" => Self::Left,
            "right" | "pakanan" | "kanan" => Self::Right,
            "up" | "pataas" | "taas" => Self::Up,
            "down" | "pababa" | "baba" => Self::Down,
            _ => return None,
        })
    }

    #[cfg(windows)]
    pub fn offset(self, pixels: i32) -> (i32, i32) {
        match self {
            Self::Left => (-pixels, 0),
            Self::Right => (pixels, 0),
            Self::Up => (0, -pixels),
            Self::Down => (0, pixels),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Landmark {
    Center,
    TopLeft,
    Top,
    TopRight,
    BottomLeft,
    Bottom,
    BottomRight,
    Left,
    Right,
    TitleBar,
    LeftThird,
    MiddleThird,
    RightThird,
    LeftTwoThirds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
    Double,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputAction {
    Move(Direction, i32),
    Point(Landmark),
    Drag(Direction, i32),
    Click(Button, Option<String>),
    Scroll(Direction, i32),
    Keys(String, u8),
    Text(String),
    Select(Direction, u8, bool),
    SelectLine(bool),
    Grid(u8, u8),
    Cell(u8),
    Hover(String),
    Focus(String),
    Invoke(String),
    Toggle(String),
    Expand(String, bool),
    Slider(u8),
    SelectItem(u16),
    Role(String, u16),
    Inspect(String),
    List(u8),
    Mark(String),
    Find(String),
    Paste,
    Replace(String),
    Case(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowControl {
    Switch,
    Find,
    Mode(ExternalAction),
    Snap(Landmark),
    Resize(u16, u16),
    Display(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemAction {
    Volume(i32, bool),
    AppVolume(String, u8),
    Output(String, Option<u8>),
    Mute(String),
    Brightness(u8),
    Settings(String),
    Magnifier(u16),
    Metric(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileAction {
    OpenFolder(String),
    Create(String),
    Find(String),
    Filter(String),
    Sort(String),
    View(String),
    Copy(String),
    Move(String),
    Rename(String),
    Recycle(String),
    Edit(String),
    NewText(String),
    SaveCopy(String),
    Play(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserAction {
    Open(String),
    Search(String),
    Tab(String),
    History(i8),
    Zoom(u16),
    Bookmark(String),
    Reopen(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaAction {
    Play(String, bool),
    Search(String, String),
    Seek(i32, bool),
    Rate(u16),
    Status(String),
    PauseLater(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoutineAction {
    Run(String),
    Preview(String),
    Shortcut(String),
    SaveLayout(String),
    RestoreLayout(String),
    AddApp(String, String),
    Wait(String, u32),
    Remove(String, u8),
    Duplicate(String),
    LastRun(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Stop,
    DesktopMode(bool),
    Palette,
    Input(String, InputAction),
    Window(String, WindowControl),
    Launch(String),
    Clipboard(String),
    Capture(String),
    CopyTitle(String),
    Files(FileAction),
    Browser(BrowserAction),
    System(SystemAction),
    Media(MediaAction),
    Routines(RoutineAction),
    Timer(u32, String),
    Task(String),
    Reminder(u32, String),
    Note(String),
    FindNote(String),
    FindChat(String),
    View(String),
    Saver(String),
    SaverSetting(String, String),
    Power(String),
    Companion,
}

impl Command {
    pub fn family(&self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::DesktopMode(_) => "desktop_mode",
            Self::Palette => "command_palette",
            Self::Input(_, action) => match action {
                InputAction::Move(..) => "pointer_move",
                InputAction::Point(..) => "pointer_position",
                InputAction::Drag(..) => "pointer_drag",
                InputAction::Click(..) => "pointer_click",
                InputAction::Scroll(..) => "pointer_scroll",
                InputAction::Keys(..) => "key_press",
                InputAction::Text(..) => "text_insert",
                InputAction::Select(..)
                | InputAction::SelectLine(..)
                | InputAction::SelectItem(..) => "select",
                InputAction::Grid(..) | InputAction::Cell(..) => "pointer_grid",
                InputAction::Hover(..)
                | InputAction::Focus(..)
                | InputAction::Invoke(..)
                | InputAction::Toggle(..)
                | InputAction::Expand(..)
                | InputAction::Slider(..)
                | InputAction::Role(..)
                | InputAction::Inspect(..)
                | InputAction::List(..) => "accessibility",
                InputAction::Mark(..) => "pointer_marker",
                InputAction::Find(..) => "find_text",
                InputAction::Paste => "paste",
                InputAction::Replace(..) => "replace_selection",
                InputAction::Case(..) => "change_selection_case",
            },
            Self::Window(..) => "window",
            Self::Launch(..) => "launch",
            Self::Clipboard(..) | Self::CopyTitle(..) => "clipboard",
            Self::Capture(..) => "window_capture",
            Self::Files(..) => "files",
            Self::Browser(..) => "browser",
            Self::System(..) => "system",
            Self::Media(..) => "media",
            Self::Routines(..) => "routine",
            Self::Timer(..) => "timer",
            Self::Task(..) => "task",
            Self::Reminder(..) => "reminder",
            Self::Note(..) | Self::FindNote(..) => "note",
            Self::FindChat(..) => "chat_search",
            Self::View(..) => "view",
            Self::Saver(..) | Self::SaverSetting(..) => "screensaver",
            Self::Power(..) => "power",
            Self::Companion => "conversation",
        }
    }
}
