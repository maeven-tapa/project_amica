use std::{
    cell::{Cell, RefCell},
    env,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    rc::{Rc, Weak},
    time::Duration,
};

use slint::{ComponentHandle, Timer, TimerMode};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, ContainsFullScreenElementChangedEventHandler,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    IsDocumentPlayingAudioChangedEventHandler, Microsoft::Web::WebView2::Win32::*,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, SourceChangedEventHandler,
    take_pwstr,
};
use windows::{
    Win32::Foundation::{E_POINTER, HWND, RECT},
    core::{BOOL, Error, Interface, PCWSTR, PWSTR},
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

use crate::{AmicaWindow, Panel, desktop};

use super::{
    PlayRequest, Service,
    playback::Playback,
    source::{SPOTIFY, Source, YOUTUBE},
};

const UNAVAILABLE: &str = "Could not start the embedded browser. Check that Microsoft Edge WebView2 Runtime is installed, then reload.";

enum Slot {
    Closed,
    Starting(usize),
    Ready(WebPage),
    Failed,
}

struct Pages {
    slots: [Slot; 2],
    serial: usize,
    apartment: Option<Apartment>,
    expanded: bool,
    restore_windowed: bool,
    playback: Option<Playback>,
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct WebPage {
    controller: ICoreWebView2Controller,
    view: ICoreWebView2,
    audio: Option<ICoreWebView2_8>,
    playing: Rc<Cell<bool>>,
    muted: bool,
    fullscreen: Rc<Cell<bool>>,
    source: Rc<RefCell<Source>>,
    shown: bool,
    bounds: [i32; 4],
}

impl Drop for WebPage {
    fn drop(&mut self) {
        let _ = unsafe { self.controller.Close() };
    }
}

impl WebPage {
    fn new(
        controller: ICoreWebView2Controller,
        index: usize,
        window: slint::Weak<AmicaWindow>,
    ) -> windows::core::Result<Self> {
        let view = unsafe { controller.CoreWebView2()? };
        let page = Self {
            audio: view.cast().ok(),
            controller,
            view,
            playing: Rc::default(),
            muted: false,
            fullscreen: Rc::default(),
            source: Rc::default(),
            shown: false,
            bounds: [0; 4],
        };
        unsafe {
            page.controller.SetIsVisible(false)?;
            let settings = page.view.Settings()?;
            settings.SetAreHostObjectsAllowed(false)?;
            settings.SetIsWebMessageEnabled(false)?;
            settings.SetAreDevToolsEnabled(false)?;
            page.view.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(|_, event| {
                    if let Some(event) = event {
                        let mut uri = PWSTR::null();
                        event.Uri(&mut uri)?;
                        let uri = take_pwstr(uri);
                        if !web_address(&uri) && uri != "about:blank" {
                            event.SetCancel(true)?;
                        }
                    }
                    Ok(())
                })),
                &mut 0,
            )?;
            page.view.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(|view, event| {
                    if let Some(event) = event {
                        event.SetHandled(true)?;
                        let mut uri = PWSTR::null();
                        event.Uri(&mut uri)?;
                        let uri = take_pwstr(uri);
                        if web_address(&uri)
                            && let Some(view) = view
                        {
                            let uri: Vec<_> = uri.encode_utf16().chain([0]).collect();
                            view.Navigate(PCWSTR(uri.as_ptr()))?;
                        }
                    }
                    Ok(())
                })),
                &mut 0,
            )?;
            page.controller.add_AcceleratorKeyPressed(
                &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, event| {
                    if let Some(event) = event {
                        let mut key = 0;
                        event.VirtualKey(&mut key)?;
                        if key == 0x70 {
                            event.SetHandled(true)?;
                            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                            event.KeyEventKind(&mut kind)?;
                            if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_UP {
                                let _ = window.upgrade_in_event_loop(|window| {
                                    window.invoke_toggle_fullscreen()
                                });
                            }
                        }
                    }
                    Ok(())
                })),
                &mut 0,
            )?;
            let source = Rc::clone(&page.source);
            page.view.add_SourceChanged(
                &SourceChangedEventHandler::create(Box::new(move |view, _| {
                    if let Some(view) = view {
                        let mut uri = PWSTR::null();
                        view.Source(&mut uri)?;
                        source.replace(Source::from_url(index, &take_pwstr(uri)));
                    }
                    Ok(())
                })),
                &mut 0,
            )?;
            let fullscreen = Rc::clone(&page.fullscreen);
            page.view.add_ContainsFullScreenElementChanged(
                &ContainsFullScreenElementChangedEventHandler::create(Box::new(move |view, _| {
                    if let Some(view) = view {
                        let mut active = BOOL::default();
                        view.ContainsFullScreenElement(&mut active)?;
                        fullscreen.set(active.as_bool());
                    }
                    Ok(())
                })),
                &mut 0,
            )?;
            if let Some(audio) = &page.audio {
                audio.SetIsMuted(false)?;
                let playing = Rc::clone(&page.playing);
                audio.add_IsDocumentPlayingAudioChanged(
                    &IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |view, _| {
                        if let Some(view) = view {
                            let audio: ICoreWebView2_8 = view.cast()?;
                            let mut active = BOOL::default();
                            audio.IsDocumentPlayingAudio(&mut active)?;
                            playing.set(active.as_bool());
                        }
                        Ok(())
                    })),
                    &mut 0,
                )?;
            }
        }
        page.navigate(if index == 0 { YOUTUBE } else { SPOTIFY })?;
        Ok(page)
    }

    fn navigate(&self, url: &str) -> windows::core::Result<()> {
        let uri: Vec<_> = url.encode_utf16().chain([0]).collect();
        unsafe { self.view.Navigate(PCWSTR(uri.as_ptr())) }
    }

    fn update(&mut self, bounds: [i32; 4], visible: bool) {
        if visible && bounds != self.bounds {
            self.bounds = bounds;
            let [left, top, right, bottom] = bounds;
            let _ = unsafe {
                self.controller.SetBounds(RECT {
                    left,
                    top,
                    right,
                    bottom,
                })
            };
        }
        if visible != self.shown {
            self.shown = visible;
            let _ = unsafe { self.controller.SetIsVisible(visible) };
        }
    }
}

fn web_address(uri: &str) -> bool {
    uri.starts_with("https://")
}

pub(super) fn connect(window: &AmicaWindow) -> Timer {
    let state = Rc::new(RefCell::new(Pages {
        slots: [Slot::Closed, Slot::Closed],
        serial: 0,
        apartment: None,
        expanded: false,
        restore_windowed: false,
        playback: None,
    }));

    let playback_state = Rc::clone(&state);
    let weak = window.as_weak();
    window.on_play_media(move |service, query| {
        let Some(window) = weak.upgrade() else { return };
        let Some(service) = Service::parse(&service) else {
            return;
        };
        let Ok(request) = PlayRequest::new(service, &query) else {
            return;
        };
        window.window().set_minimized(false);
        window.invoke_show_panel(service.panel());
        desktop::report_window_error(&window, Ok(()));
        let mut state = playback_state.borrow_mut();
        if matches!(state.slots[service.index()], Slot::Failed) {
            state.slots[service.index()] = Slot::Closed;
        }
        state.playback = Some(Playback::new(request));
    });

    let reload_state = Rc::clone(&state);
    let weak = window.as_weak();
    window.on_reload_media(move || {
        let Some(window) = weak.upgrade() else { return };
        let Some(index) = selected(&window) else {
            return;
        };
        let mut state = reload_state.borrow_mut();
        state.playback = None;
        match &mut state.slots[index] {
            Slot::Ready(page) => {
                let _ = unsafe { page.view.Reload() };
            }
            Slot::Failed => state.slots[index] = Slot::Closed,
            Slot::Closed | Slot::Starting(_) => {}
        }
    });

    let mute_state = Rc::clone(&state);
    let weak = window.as_weak();
    window.on_toggle_media_mute(move || {
        let Some(window) = weak.upgrade() else { return };
        if let Some(index) = selected(&window)
            && let Slot::Ready(page) = &mut mute_state.borrow_mut().slots[index]
            && let Some(audio) = &page.audio
            && unsafe { audio.SetIsMuted(!page.muted) }.is_ok()
        {
            page.muted = !page.muted;
        }
    });

    let player_state = Rc::clone(&state);
    let weak = window.as_weak();
    window.on_switch_media_player(move || {
        let Some(window) = weak.upgrade() else { return };
        if selected(&window) == Some(1)
            && let Slot::Ready(page) = &player_state.borrow().slots[1]
        {
            let destination = page
                .source
                .borrow()
                .spotify_player
                .as_ref()
                .map(|player| player.switch_to.clone());
            if let Some(url) = destination {
                let _ = page.navigate(&url);
            }
        }
    });

    let external_state = Rc::clone(&state);
    let weak = window.as_weak();
    window.on_open_media_external(move || {
        let Some(window) = weak.upgrade() else { return };
        let Some(index) = selected(&window) else {
            return;
        };
        let state = external_state.borrow();
        let current = match &state.slots[index] {
            Slot::Ready(page) => page.source.borrow().external.clone(),
            _ => None,
        };
        let fallback = if index == 0 { YOUTUBE } else { SPOTIFY };
        desktop::open_media_page(&window, current.unwrap_or_else(|| fallback.into()));
    });

    let weak = window.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(32), move || {
        let Some(window) = weak.upgrade() else { return };
        synchronize(&window, &state);
    });
    timer
}

fn selected(window: &AmicaWindow) -> Option<usize> {
    if window.get_memory_open()
        || window.get_saver_active()
        || !window.get_power_request().is_empty()
        || window.window().is_minimized()
    {
        return None;
    }
    match window.get_active_panel() {
        Panel::Youtube => Some(0),
        Panel::Spotify => Some(1),
        _ => None,
    }
}

fn synchronize(window: &AmicaWindow, state: &Rc<RefCell<Pages>>) {
    let visible = selected(window);
    let open = [window.get_youtube_open(), window.get_spotify_open()];
    let scale = window.window().scale_factor();
    let x = window.get_media_x();
    let y = window.get_media_y();
    let panel_bounds = [
        x,
        y,
        x + window.get_media_width(),
        y + window.get_media_height(),
    ]
    .map(|value| (value * scale).round() as i32);
    let mut creation = None;
    {
        let mut state = state.borrow_mut();
        let expanded = visible.is_some_and(|index| {
            open[index] && matches!(&state.slots[index], Slot::Ready(page) if page.fullscreen.get())
        });
        if expanded != state.expanded {
            if expanded {
                state.restore_windowed = !window.window().is_fullscreen();
                window.window().set_fullscreen(true);
            } else if state.restore_windowed && !window.get_saver_active() {
                window.window().set_fullscreen(false);
            }
            state.expanded = expanded;
        }
        if window.get_media_fullscreen() != expanded {
            window.set_media_fullscreen(expanded);
        }
        let media_status = if matches!(&state.slots[0], Slot::Ready(page) if page.playing.get()) {
            "YouTube playing"
        } else if matches!(&state.slots[1], Slot::Ready(page) if page.playing.get()) {
            "Spotify playing"
        } else {
            "No active media"
        };
        if window.get_home_media() != media_status {
            window.set_home_media(media_status.into());
        }
        let bounds = if expanded {
            let size = window.window().size();
            [0, 0, size.width as i32, size.height as i32]
        } else {
            panel_bounds
        };
        for (index, open) in open.into_iter().enumerate() {
            if !open {
                state.slots[index] = Slot::Closed;
                continue;
            }
            let visible = visible == Some(index);
            match &mut state.slots[index] {
                Slot::Ready(page) => page.update(bounds, visible),
                Slot::Closed if visible => {
                    state.serial += 1;
                    let serial = state.serial;
                    state.slots[index] = Slot::Starting(serial);
                    creation = Some((index, serial));
                }
                _ => {}
            }
        }
        if let Some(mut playback) = state.playback.take() {
            let index = playback.service.index();
            if visible == Some(index) {
                let progress = match &state.slots[index] {
                    Slot::Ready(page) => playback.advance(&page.view),
                    Slot::Failed => Some(Err(UNAVAILABLE)),
                    Slot::Closed | Slot::Starting(_) => None,
                };
                if let Some(result) = progress {
                    desktop::report_window_error(window, result);
                } else {
                    state.playback = Some(playback);
                }
            }
        }
        let error = if visible.is_some_and(|index| matches!(state.slots[index], Slot::Failed)) {
            UNAVAILABLE
        } else {
            ""
        };
        if window.get_media_error() != error {
            window.set_media_error(error.into());
        }
        let mut controls = (false, false, false, "");
        if let Some(index) = visible
            && let Slot::Ready(page) = &state.slots[index]
        {
            let source = page.source.borrow();
            let player_action = source.spotify_player.as_ref().map_or("", |player| {
                if player.embedded {
                    "Back to Spotify"
                } else {
                    "Open player"
                }
            });
            controls = (
                page.audio.is_some(),
                page.playing.get(),
                page.muted,
                player_action,
            );
        }
        let (audio, playing, muted, player_action) = controls;
        if window.get_media_audio_available() != audio {
            window.set_media_audio_available(audio);
        }
        if window.get_media_playing() != playing {
            window.set_media_playing(playing);
        }
        if window.get_media_muted() != muted {
            window.set_media_muted(muted);
        }
        if window.get_media_player_action() != player_action {
            window.set_media_player_action(player_action.into());
        }
    }
    if let Some((index, serial)) = creation
        && let Err(error) = start(window, state, index, serial)
    {
        eprintln!("Amica media: {error}");
        finish(&Rc::downgrade(state), index, serial, Err(error));
    }
}

fn start(
    window: &AmicaWindow,
    state: &Rc<RefCell<Pages>>,
    index: usize,
    serial: usize,
) -> windows::core::Result<()> {
    let hwnd = desktop::window_handle(window).ok_or_else(|| Error::from(E_POINTER))?;
    let hwnd = HWND(hwnd);
    if state.borrow().apartment.is_none() {
        unsafe {
            windows::core::HRESULT(CoInitializeEx(
                std::ptr::null(),
                COINIT_APARTMENTTHREADED as u32,
            ))
            .ok()?
        };
        state.borrow_mut().apartment = Some(Apartment);
    }
    let data = env::var_os("AMICA_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("LOCALAPPDATA").map(|path| PathBuf::from(path).join("Amica")))
        .ok_or_else(|| Error::from(E_POINTER))?
        .join("WebView2");
    let data: Vec<_> = data.as_os_str().encode_wide().chain([0]).collect();
    let state = Rc::downgrade(state);
    let window = window.as_weak();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result, environment| {
            let inner = || -> windows::core::Result<()> {
                result?;
                let environment = environment.ok_or_else(|| Error::from(E_POINTER))?;
                let state = state.clone();
                let window = window.clone();
                let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |result, controller| {
                        let page = result
                            .and_then(|()| controller.ok_or_else(|| Error::from(E_POINTER)))
                            .and_then(|controller| WebPage::new(controller, index, window));
                        finish(&state, index, serial, page);
                        Ok(())
                    },
                ));
                unsafe { environment.CreateCoreWebView2Controller(hwnd, &handler) }
            };
            if let Err(error) = inner() {
                finish(&state, index, serial, Err(error));
            }
            Ok(())
        },
    ));
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            PCWSTR(data.as_ptr()),
            None,
            &handler,
        )
    }
}

fn finish(
    state: &Weak<RefCell<Pages>>,
    index: usize,
    serial: usize,
    page: windows::core::Result<WebPage>,
) {
    if let Some(state) = state.upgrade() {
        let mut state = state.borrow_mut();
        if matches!(state.slots[index], Slot::Starting(active) if active == serial) {
            state.slots[index] = match page {
                Ok(page) => Slot::Ready(page),
                Err(error) => {
                    eprintln!("Amica media: {error}");
                    Slot::Failed
                }
            };
        }
    }
}
