mod memory_entry;

use std::{cell::RefCell, rc::Rc, time::Instant};

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    AmicaWindow, Panel, PanelItem,
    desktop::{self, ExternalAction, PowerAction, TargetWindow, WindowAction},
    media::PlayRequest,
    settings::AudioKind,
};

#[derive(Clone, Debug, PartialEq)]
pub enum PanelAction {
    Show(Panel),
    Close(Panel),
    Window(WindowAction),
    ExitAmica,
    External(ExternalAction, TargetWindow),
    Launch(&'static str),
    Power(PowerAction),
    AudioDevice(AudioKind, String),
    SearchPage(String),
    PlayMedia(PlayRequest),
    Local(crate::actions::command::Command, Instant),
}

impl PanelAction {
    pub fn apply(self, window: &AmicaWindow) {
        match self {
            Self::Show(panel) => window.invoke_show_panel(panel),
            Self::Close(panel) => window.invoke_close_panel(panel),
            Self::Window(action) => action.apply(window),
            Self::ExitAmica => window.invoke_exit_amica(),
            Self::External(action, target) => {
                desktop::report_window_error(window, action.apply(target))
            }
            Self::Launch(app) => window.invoke_launch_app(app.into()),
            Self::Power(action) => desktop::request_power(window, action),
            Self::SearchPage(query) => desktop::open_search_results(window, query),
            Self::PlayMedia(request) => {
                window.invoke_play_media(request.service.id().into(), request.query.into());
            }
            Self::AudioDevice(kind, name) => {
                window.invoke_select_audio_device(kind.input(), name.into())
            }
            Self::Local(command, created) => {
                let result = crate::actions::dispatcher::dispatch(window, command, created);
                window.set_command_result(result.into());
            }
        }
    }

    pub fn after_reply(&self) -> bool {
        !matches!(self, Self::Show(panel) | Self::Close(panel) if !matches!(panel, Panel::Youtube | Panel::Spotify))
    }
}

#[derive(Default)]
struct Navigation {
    open: Vec<Panel>,
    active: Panel,
}

impl Navigation {
    fn show(&mut self, panel: Panel) -> bool {
        if matches!(panel, Panel::None | Panel::MemoryMap) {
            return false;
        }
        self.active = panel;
        if self.open.contains(&panel) {
            return false;
        }
        self.open.push(panel);
        true
    }

    fn close(&mut self, panel: Panel) -> bool {
        let panel = if panel == Panel::None {
            self.active
        } else {
            panel
        };
        let Some(index) = self.open.iter().position(|entry| *entry == panel) else {
            return false;
        };
        self.open.remove(index);
        if self.active == panel {
            self.active = self
                .open
                .get(index)
                .or_else(|| self.open.last())
                .copied()
                .unwrap_or(Panel::None);
        }
        true
    }

    fn publish(&self, window: &AmicaWindow) {
        let tabs = self
            .open
            .iter()
            .filter(|&&panel| panel != Panel::Apps)
            .map(|&panel| PanelItem {
                panel,
                label: match panel {
                    Panel::Conversation => "Conversation",
                    Panel::TimeDate => "Time & Date",
                    Panel::Saved => "Saved",
                    Panel::Youtube => "YouTube",
                    Panel::Spotify => "Spotify",
                    Panel::Settings => "Settings",
                    Panel::SystemSpecs => "System specs",
                    Panel::CommandPalette => "Commands",
                    Panel::Daily => "Tasks",
                    Panel::Routines => "Routines",
                    Panel::None | Panel::MemoryMap | Panel::Apps => unreachable!(),
                }
                .into(),
            })
            .collect::<Vec<_>>();
        window.set_panel_tabs(ModelRc::new(VecModel::from(tabs)));
        window.set_youtube_open(self.open.contains(&Panel::Youtube));
        window.set_spotify_open(self.open.contains(&Panel::Spotify));
    }
}

pub fn connect(window: &AmicaWindow) {
    memory_entry::connect(window);
    let weak = window.as_weak();
    window.on_toggle_fullscreen(move || {
        if let Some(window) = weak.upgrade() {
            WindowAction::ToggleFullscreen.apply(&window);
        }
    });

    let state = Rc::new(RefCell::new(Navigation::default()));
    let weak = window.as_weak();
    let show_state = Rc::clone(&state);
    window.on_show_panel(move |panel| {
        let Some(window) = weak.upgrade() else { return };
        if panel == Panel::MemoryMap {
            window.invoke_present_memory(false);
            return;
        }
        if panel == Panel::None {
            window.invoke_dismiss_memory();
            show_state.borrow_mut().active = Panel::None;
            window.set_active_panel(Panel::None);
            return;
        }
        window.invoke_dismiss_memory();
        if panel == Panel::Settings {
            window.invoke_refresh_settings();
        }
        if panel == Panel::SystemSpecs {
            window.invoke_scan_hardware();
        }
        let mut state = show_state.borrow_mut();
        if state.show(panel) {
            state.publish(&window);
        }
        window.set_active_panel(state.active);
    });

    let weak = window.as_weak();
    window.on_close_panel(move |panel| {
        let Some(window) = weak.upgrade() else { return };
        if panel == Panel::MemoryMap || (panel == Panel::None && window.get_memory_open()) {
            window.invoke_dismiss_memory();
            return;
        }
        let mut state = state.borrow_mut();
        if state.close(panel) {
            state.publish(&window);
            window.set_active_panel(state.active);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_empty_reopens_existing_tabs_and_returns_to_the_orb() {
        let mut state = Navigation::default();
        assert!(state.open.is_empty());
        assert_eq!(state.active, Panel::None);
        assert!(state.show(Panel::Saved));
        assert!(state.show(Panel::Conversation));
        assert!(state.show(Panel::TimeDate));
        assert!(!state.show(Panel::Conversation));
        assert_eq!(
            state.open,
            [Panel::Saved, Panel::Conversation, Panel::TimeDate]
        );
        assert_eq!(state.active, Panel::Conversation);
        assert!(state.close(Panel::Conversation));
        assert_eq!(state.active, Panel::TimeDate);
        assert!(state.close(Panel::None));
        assert_eq!(state.active, Panel::Saved);
        assert!(state.show(Panel::TimeDate));
        assert!(state.close(Panel::Saved));
        assert_eq!(state.active, Panel::TimeDate);
        assert!(state.close(Panel::TimeDate));
        assert_eq!(state.active, Panel::None);
        assert!(!state.close(Panel::None));
        assert!(state.open.is_empty());
    }
}
