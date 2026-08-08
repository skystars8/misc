#![windows_subsystem = "windows"]

mod file_dialog;
mod macro_file;
mod playback;

use eframe::egui;
use playback::{PlaybackControl, PlaybackOutcome};
use rdev::{listen, simulate, Button, Event, EventType, Key};
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F1, VK_F2, VK_SPACE};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::RECT,
    UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect},
};

/// A recorded mouse event with relative timestamp (ms since recording start)
#[derive(Clone, Copy, Debug, PartialEq)]
struct RecordedEvent {
    /// Milliseconds since the start of recording
    timestamp_ms: u64,
    event: SerializableEventType,
}

/// Serializable version of rdev EventType (only mouse-related)
#[derive(Clone, Copy, Debug, PartialEq)]
enum SerializableEventType {
    MouseMove { x: f64, y: f64 },
    ButtonPress(ButtonKind),
    ButtonRelease(ButtonKind),
    Wheel { delta_x: i64, delta_y: i64 },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ButtonKind {
    Left,
    Right,
    Middle,
}

impl TryFrom<Button> for ButtonKind {
    type Error = ();

    fn try_from(button: Button) -> Result<Self, Self::Error> {
        match button {
            Button::Left => Ok(Self::Left),
            Button::Right => Ok(Self::Right),
            Button::Middle => Ok(Self::Middle),
            _ => Err(()),
        }
    }
}

impl From<ButtonKind> for Button {
    fn from(button: ButtonKind) -> Self {
        match button {
            ButtonKind::Left => Button::Left,
            ButtonKind::Right => Button::Right,
            ButtonKind::Middle => Button::Middle,
        }
    }
}

impl SerializableEventType {
    fn as_rdev_event(self) -> EventType {
        match self {
            Self::MouseMove { x, y } => EventType::MouseMove { x, y },
            Self::ButtonPress(button) => EventType::ButtonPress(button.into()),
            Self::ButtonRelease(button) => EventType::ButtonRelease(button.into()),
            Self::Wheel { delta_x, delta_y } => EventType::Wheel { delta_x, delta_y },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScreenRect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl ScreenRect {
    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Removes the final contiguous run of events whose last known cursor
/// position is inside the recorder window. This strips the trip to and click
/// on Stop Record without discarding the user's last action outside the app.
fn trim_trailing_window_events(events: &mut Vec<RecordedEvent>, rect: ScreenRect) -> usize {
    let original_len = events.len();
    let mut last_position = None;
    let mut trim_from = None;

    for (index, recorded) in events.iter().enumerate() {
        if let SerializableEventType::MouseMove { x, y } = recorded.event {
            last_position = Some((x, y));
        }

        let inside = last_position.is_some_and(|(x, y)| rect.contains(x, y));
        if inside {
            trim_from.get_or_insert(index);
        } else {
            trim_from = None;
        }
    }

    if let Some(index) = trim_from {
        events.truncate(index);
    }
    original_len - events.len()
}

#[cfg(target_os = "windows")]
fn foreground_window_rect() -> Option<ScreenRect> {
    // Stop Record is invoked by a click in this app, so the foreground HWND
    // at that moment is the recorder window whose physical-pixel rectangle we
    // need to compare with rdev's physical mouse coordinates.
    let window = unsafe { GetForegroundWindow() };
    if window == 0 {
        return None;
    }

    // RECT is a plain Win32 output structure fully initialized by
    // GetWindowRect on success.
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(window, &mut rect) } == 0 {
        return None;
    }

    Some(ScreenRect {
        left: f64::from(rect.left),
        top: f64::from(rect.top),
        right: f64::from(rect.right),
        bottom: f64::from(rect.bottom),
    })
}

#[cfg(not(target_os = "windows"))]
fn foreground_window_rect() -> Option<ScreenRect> {
    None
}

#[cfg(target_os = "windows")]
fn start_global_hotkey_monitor(playback: &Arc<PlaybackControl>) {
    let playback = Arc::downgrade(playback);
    thread::spawn(move || {
        let mut f1_was_down = false;
        let mut f2_was_down = false;
        let mut space_was_down = false;

        while let Some(playback) = playback.upgrade() {
            // GetAsyncKeyState's high bit reports whether the physical key is
            // currently held. Edge detection prevents key-repeat toggles.
            let f1_down = unsafe { GetAsyncKeyState(VK_F1 as i32) as u16 & 0x8000 != 0 };
            let f2_down = unsafe { GetAsyncKeyState(VK_F2 as i32) as u16 & 0x8000 != 0 };
            let space_down = unsafe { GetAsyncKeyState(VK_SPACE as i32) as u16 & 0x8000 != 0 };
            let snapshot = playback.snapshot();

            if snapshot.playing && !snapshot.stopping {
                if f1_down && !f1_was_down {
                    playback.request_pause();
                }
                if f2_down && !f2_was_down {
                    playback.request_resume();
                }
                if space_down && !space_was_down {
                    playback.request_stop();
                }
            }

            f1_was_down = f1_down;
            f2_was_down = f2_down;
            space_was_down = space_down;
            drop(playback);
            thread::sleep(Duration::from_millis(8));
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn start_global_hotkey_monitor(_playback: &Arc<PlaybackControl>) {}

struct MacroApp {
    is_recording: Arc<AtomicBool>,
    playback: Arc<PlaybackControl>,
    recorded_events: Arc<Mutex<Vec<RecordedEvent>>>,
    record_start: Arc<Mutex<Option<Instant>>>,
    listener_error: Arc<Mutex<Option<String>>>,
    listener_error_reported: bool,
    status: String,
    event_count: usize,
    duration_secs: f64,
    current_macro_path: Option<PathBuf>,
    playback_worker: Option<thread::JoinHandle<()>>,
}

impl MacroApp {
    fn new() -> Self {
        let is_recording = Arc::new(AtomicBool::new(false));
        let playback = Arc::new(PlaybackControl::new());
        let recorded_events = Arc::new(Mutex::new(Vec::new()));
        let record_start: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let listener_error = Arc::new(Mutex::new(None));

        // Start the global mouse listener once. rdev also reports keyboard
        // events, so keep those as a fallback while the independent Win32
        // monitor provides reliable global playback hotkeys.
        {
            let is_recording = Arc::clone(&is_recording);
            let playback = Arc::downgrade(&playback);
            let recorded_events = Arc::clone(&recorded_events);
            let record_start = Arc::clone(&record_start);
            let listener_error = Arc::clone(&listener_error);

            thread::spawn(move || {
                let callback_is_recording = Arc::clone(&is_recording);
                let callback = move |event: Event| {
                    if let EventType::KeyPress(key) = &event.event_type {
                        if let Some(playback) = playback.upgrade() {
                            match key {
                                Key::F1 => playback.request_pause(),
                                Key::F2 => playback.request_resume(),
                                Key::Space => playback.request_stop(),
                                _ => false,
                            };
                        }
                    }

                    if !callback_is_recording.load(Ordering::SeqCst) {
                        return;
                    }

                    let start = match record_start.lock() {
                        Ok(guard) => match *guard {
                            Some(start) => start,
                            None => return,
                        },
                        Err(_) => return,
                    };
                    let elapsed = start.elapsed().as_millis() as u64;

                    let serializable = match event.event_type {
                        EventType::MouseMove { x, y } => {
                            Some(SerializableEventType::MouseMove { x, y })
                        }
                        EventType::ButtonPress(button) => ButtonKind::try_from(button)
                            .ok()
                            .map(SerializableEventType::ButtonPress),
                        EventType::ButtonRelease(button) => ButtonKind::try_from(button)
                            .ok()
                            .map(SerializableEventType::ButtonRelease),
                        EventType::Wheel { delta_x, delta_y } => {
                            Some(SerializableEventType::Wheel { delta_x, delta_y })
                        }
                        _ => None,
                    };

                    if let Some(event) = serializable {
                        if let Ok(mut events) = recorded_events.lock() {
                            // Stop may have been requested after the first
                            // check but before this lock became available.
                            if callback_is_recording.load(Ordering::SeqCst) {
                                events.push(RecordedEvent {
                                    timestamp_ms: elapsed,
                                    event,
                                });
                            }
                        }
                    }
                };

                let failure = match listen(callback) {
                    Ok(()) => "the global listener stopped unexpectedly".to_owned(),
                    Err(error) => format!("{error:?}"),
                };
                is_recording.store(false, Ordering::SeqCst);
                if let Ok(mut slot) = listener_error.lock() {
                    *slot = Some(failure);
                }
            });
        }

        start_global_hotkey_monitor(&playback);

        Self {
            is_recording,
            playback,
            recorded_events,
            record_start,
            listener_error,
            listener_error_reported: false,
            status: "Ready. Click Start Record or load a saved macro.".to_owned(),
            event_count: 0,
            duration_secs: 0.0,
            current_macro_path: None,
            playback_worker: None,
        }
    }

    fn start_recording(&mut self) {
        if self.playback.snapshot().playing {
            self.status = "Cannot record while playing.".to_owned();
            return;
        }
        if let Some(error) = self.input_listener_error() {
            self.status = format!("Cannot record: the input listener failed ({error}).");
            return;
        }

        match self.recorded_events.lock() {
            Ok(mut events) => events.clear(),
            Err(_) => {
                self.status = "Could not clear the previous recording.".to_owned();
                return;
            }
        }
        match self.record_start.lock() {
            Ok(mut start) => *start = Some(Instant::now()),
            Err(_) => {
                self.status = "Could not start the recording timer.".to_owned();
                return;
            }
        }

        self.is_recording.store(true, Ordering::SeqCst);
        self.status = "🔴 Recording... Move mouse and click!".to_owned();
        self.event_count = 0;
        self.duration_secs = 0.0;
        self.current_macro_path = None;
    }

    fn stop_recording(&mut self) {
        self.is_recording.store(false, Ordering::SeqCst);
        let recorder_rect = foreground_window_rect();
        if let Ok(mut start) = self.record_start.lock() {
            *start = None;
        }

        let stats = self.recorded_events.lock().ok().map(|mut events| {
            if let Some(rect) = recorder_rect {
                trim_trailing_window_events(&mut events, rect);
            }
            (
                events.len(),
                events
                    .last()
                    .map_or(0.0, |last| last.timestamp_ms as f64 / 1000.0),
            )
        });
        if let Some((count, duration)) = stats {
            self.event_count = count;
            self.duration_secs = duration;
        }

        self.status = format!(
            "✅ Stopped. {} events • {:.1}s",
            self.event_count, self.duration_secs
        );
    }

    fn play_back(&mut self) {
        if self.is_recording.load(Ordering::SeqCst) {
            self.status = "Stop recording first.".to_owned();
            return;
        }
        if self.playback.snapshot().playing {
            self.status = "Already playing.".to_owned();
            return;
        }

        let events = match self.recorded_events.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => {
                self.status = "Error accessing the recording.".to_owned();
                return;
            }
        };

        if events.is_empty() {
            self.status = "No recording to play.".to_owned();
            return;
        }
        if let Err(error) = macro_file::validate_events(&events) {
            self.status = format!("Cannot play this macro: {error}");
            return;
        }
        if let Some(previous_worker) = self.playback_worker.take() {
            if previous_worker.join().is_err() {
                self.status = "The previous playback worker ended unexpectedly.".to_owned();
                return;
            }
        }
        if !self.playback.begin() {
            self.status = "Already playing.".to_owned();
            return;
        }

        self.status = "▶️ Playing back... (F1 pause / F2 resume / Space stop)".to_owned();
        let playback = Arc::clone(&self.playback);

        self.playback_worker = Some(thread::spawn(move || {
            let mut pressed_buttons = HashSet::new();
            let worker_result = catch_unwind(AssertUnwindSafe(|| {
                let mut last_timestamp = 0_u64;
                let mut outcome = PlaybackOutcome::Completed;

                for recorded in events {
                    let delay =
                        Duration::from_millis(recorded.timestamp_ms.saturating_sub(last_timestamp));
                    if !playback.wait_for_delay(delay) {
                        outcome = PlaybackOutcome::Stopped;
                        break;
                    }

                    let event_type = recorded.event.as_rdev_event();
                    let result = playback.run_when_ready(|| simulate(&event_type));
                    match result {
                        None => {
                            outcome = PlaybackOutcome::Stopped;
                            break;
                        }
                        Some(Err(error)) => {
                            outcome = PlaybackOutcome::Failed(format!(
                                "Windows rejected an input event: {error:?}"
                            ));
                            break;
                        }
                        Some(Ok(())) => {}
                    };

                    match recorded.event {
                        SerializableEventType::ButtonPress(button) => {
                            pressed_buttons.insert(button);
                        }
                        SerializableEventType::ButtonRelease(button) => {
                            pressed_buttons.remove(&button);
                        }
                        _ => {}
                    }
                    last_timestamp = recorded.timestamp_ms;
                }

                outcome
            }));

            let mut outcome = worker_result.unwrap_or_else(|_| {
                PlaybackOutcome::Failed("the playback worker panicked".to_owned())
            });
            if matches!(outcome, PlaybackOutcome::Completed) && playback.snapshot().stopping {
                outcome = PlaybackOutcome::Stopped;
            }

            // Never leave a simulated button held if playback is cancelled,
            // fails, or ends with an unmatched press in an edited text file.
            for button in pressed_buttons {
                if let Err(error) = simulate(&EventType::ButtonRelease(button.into())) {
                    if matches!(outcome, PlaybackOutcome::Completed) {
                        outcome = PlaybackOutcome::Failed(format!(
                            "could not release a mouse button: {error:?}"
                        ));
                    }
                }
            }

            playback.finish(outcome);
        }));
    }

    fn pause_playback(&mut self) {
        if self.playback.request_pause() {
            self.status = "⏸ Playback paused. F2 or Resume continues.".to_owned();
        }
    }

    fn resume_playback(&mut self) {
        if self.playback.request_resume() {
            self.status = "▶️ Playing back... (F1 pause / F2 resume / Space stop)".to_owned();
        }
    }

    fn stop_playback(&mut self) {
        if self.playback.request_stop() {
            self.status = "⏹ Stopping playback...".to_owned();
        }
    }

    fn input_listener_error(&self) -> Option<String> {
        self.listener_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
    }

    fn save_macro_to_file(&mut self) {
        let path = match file_dialog::choose_save_path(self.current_macro_path.as_deref()) {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(error) => {
                self.status = format!("Could not open Save dialog: {error}");
                return;
            }
        };

        let events = match self.recorded_events.lock() {
            Ok(events) => events.clone(),
            Err(_) => {
                self.status = "Could not access the macro to save it.".to_owned();
                return;
            }
        };

        match macro_file::save_macro(&path, &events) {
            Ok(()) => {
                let name = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                self.status = format!("💾 Saved {name} ({} events).", events.len());
                self.current_macro_path = Some(path);
            }
            Err(error) => {
                self.status = format!("Could not save macro: {error}");
            }
        }
    }

    fn load_macro_from_file(&mut self) {
        let path = match file_dialog::choose_load_path(self.current_macro_path.as_deref()) {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(error) => {
                self.status = format!("Could not open Load dialog: {error}");
                return;
            }
        };

        let events = match macro_file::load_macro(&path) {
            Ok(events) => events,
            Err(error) => {
                self.status = format!("Could not load macro: {error}");
                return;
            }
        };
        let count = events.len();
        let duration = events
            .last()
            .map_or(0.0, |event| event.timestamp_ms as f64 / 1000.0);

        match self.recorded_events.lock() {
            Ok(mut current) => *current = events,
            Err(_) => {
                self.status = "Could not replace the current macro.".to_owned();
                return;
            }
        }
        if let Ok(mut start) = self.record_start.lock() {
            *start = None;
        }

        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        self.event_count = count;
        self.duration_secs = duration;
        self.status = format!("📂 Loaded {name} ({count} events • {duration:.1}s).");
        self.current_macro_path = Some(path);
    }
}

impl eframe::App for MacroApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let listener_error = self.input_listener_error();
        if let Some(error) = &listener_error {
            if !self.listener_error_reported {
                self.is_recording.store(false, Ordering::SeqCst);
                self.status = format!("Global mouse listener failed: {error}");
                self.listener_error_reported = true;
            }
        }

        if let Some(outcome) = self.playback.take_outcome() {
            let join_failed = self
                .playback_worker
                .take()
                .is_some_and(|worker| worker.join().is_err());
            self.status = if join_failed {
                "Playback failed: the worker ended unexpectedly.".to_owned()
            } else {
                match outcome {
                    PlaybackOutcome::Completed => format!(
                        "✅ Playback finished. {} events • {:.1}s",
                        self.event_count, self.duration_secs
                    ),
                    PlaybackOutcome::Stopped => format!(
                        "⏹ Playback stopped. {} events • {:.1}s",
                        self.event_count, self.duration_secs
                    ),
                    PlaybackOutcome::Failed(error) => format!("Playback failed: {error}"),
                }
            };
        }

        let initial_playback = self.playback.snapshot();
        if initial_playback.playing && !initial_playback.stopping {
            let (pause, resume, stop) = ctx.input(|input| {
                (
                    input.key_pressed(egui::Key::F1),
                    input.key_pressed(egui::Key::F2),
                    input.key_pressed(egui::Key::Space),
                )
            });
            if pause {
                self.pause_playback();
            }
            if resume {
                self.resume_playback();
            }
            if stop {
                self.stop_playback();
            }
        }

        let playback = self.playback.snapshot();
        if playback.playing {
            self.status = if playback.stopping {
                "⏹ Stopping playback...".to_owned()
            } else if playback.paused {
                "⏸ Playback paused. F2 or Resume continues; Space stops.".to_owned()
            } else {
                "▶️ Playing back... (F1 pause / F2 resume / Space stop)".to_owned()
            };
            ctx.request_repaint_after(Duration::from_millis(80));
        }

        if self.is_recording.load(Ordering::SeqCst) {
            let live_count = self.recorded_events.lock().ok().map(|events| events.len());
            if let Some(count) = live_count {
                self.event_count = count;
            }
            let live_duration = self
                .record_start
                .lock()
                .ok()
                .and_then(|start| start.map(|instant| instant.elapsed().as_secs_f64()));
            if let Some(duration) = live_duration {
                self.duration_secs = duration;
            }
            self.status = format!(
                "🔴 Recording... {} events • {:.1}s",
                self.event_count, self.duration_secs
            );
            ctx.request_repaint_after(Duration::from_millis(80));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.heading("🖱️ Mouse Macro Recorder");
                ui.add_space(8.0);

                ui.label(egui::RichText::new(&self.status).size(16.0));
                if let Some(error) = &listener_error {
                    ui.colored_label(
                        egui::Color32::RED,
                        format!("Recording is unavailable: global listener error {error}"),
                    );
                }
                ui.add_space(14.0);

                ui.horizontal(|ui| {
                    let can_record = !self.is_recording.load(Ordering::SeqCst)
                        && !playback.playing
                        && listener_error.is_none();

                    if ui
                        .add_enabled(
                            can_record,
                            egui::Button::new("●  Start Record").min_size(egui::vec2(150.0, 40.0)),
                        )
                        .clicked()
                    {
                        self.start_recording();
                        ctx.request_repaint();
                    }

                    if ui
                        .add_enabled(
                            self.is_recording.load(Ordering::SeqCst),
                            egui::Button::new("■  Stop Record").min_size(egui::vec2(150.0, 40.0)),
                        )
                        .clicked()
                    {
                        self.stop_recording();
                        ctx.request_repaint();
                    }
                });

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let can_play = !self.is_recording.load(Ordering::SeqCst)
                        && !playback.playing
                        && self.event_count > 0;

                    if ui
                        .add_enabled(
                            can_play,
                            egui::Button::new("▶  Play").min_size(egui::vec2(105.0, 38.0)),
                        )
                        .clicked()
                    {
                        self.play_back();
                        ctx.request_repaint();
                    }

                    if ui
                        .add_enabled(
                            playback.playing && !playback.paused && !playback.stopping,
                            egui::Button::new("Ⅱ  Pause").min_size(egui::vec2(105.0, 38.0)),
                        )
                        .clicked()
                    {
                        self.pause_playback();
                        ctx.request_repaint();
                    }

                    if ui
                        .add_enabled(
                            playback.playing && playback.paused && !playback.stopping,
                            egui::Button::new("▶  Resume").min_size(egui::vec2(105.0, 38.0)),
                        )
                        .clicked()
                    {
                        self.resume_playback();
                        ctx.request_repaint();
                    }

                    if ui
                        .add_enabled(
                            playback.playing && !playback.stopping,
                            egui::Button::new("■  Stop").min_size(egui::vec2(105.0, 38.0)),
                        )
                        .clicked()
                    {
                        self.stop_playback();
                        ctx.request_repaint();
                    }
                });

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let idle = !self.is_recording.load(Ordering::SeqCst) && !playback.playing;
                    if ui
                        .add_enabled(
                            idle,
                            egui::Button::new("📂  Load Macro...")
                                .min_size(egui::vec2(150.0, 36.0)),
                        )
                        .clicked()
                    {
                        self.load_macro_from_file();
                        ctx.request_repaint();
                    }
                    if ui
                        .add_enabled(
                            idle && self.event_count > 0,
                            egui::Button::new("💾  Save Macro...")
                                .min_size(egui::vec2(150.0, 36.0)),
                        )
                        .clicked()
                    {
                        self.save_macro_to_file();
                        ctx.request_repaint();
                    }
                });

                if let Some(path) = &self.current_macro_path {
                    ui.small(format!("File: {}", path.display()));
                } else {
                    ui.small("File: unsaved recording");
                }

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);

                ui.label(format!("Events: {}", self.event_count));
                ui.label(format!("Duration: {:.1} seconds", self.duration_secs));

                ui.add_space(10.0);
                ui.small("• Records mouse movement, left/right/middle clicks and wheel");
                ui.small("• Saved .txt macros are readable JSON and can be loaded later");
                ui.small("• Hotkeys: F1 = Pause • F2 = Resume • Space = Stop playback");
                ui.small("• Tip: Run as Administrator if playback fails in some apps");
            });
        });
    }
}

impl Drop for MacroApp {
    fn drop(&mut self) {
        self.is_recording.store(false, Ordering::SeqCst);
        self.playback.request_stop();
        if let Some(worker) = self.playback_worker.take() {
            let _ = worker.join();
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([580.0, 490.0])
            .with_min_inner_size([580.0, 490.0])
            .with_resizable(false)
            .with_title("Mouse Macro Recorder"),
        ..Default::default()
    };

    eframe::run_native(
        "Mouse Macro Recorder",
        options,
        Box::new(|_cc| Ok(Box::new(MacroApp::new()))),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        trim_trailing_window_events, ButtonKind, RecordedEvent, ScreenRect, SerializableEventType,
    };

    #[test]
    fn stop_record_click_is_trimmed_but_outside_actions_are_preserved() {
        let mut events = vec![
            RecordedEvent {
                timestamp_ms: 10,
                event: SerializableEventType::MouseMove { x: 50.0, y: 50.0 },
            },
            RecordedEvent {
                timestamp_ms: 20,
                event: SerializableEventType::ButtonPress(ButtonKind::Left),
            },
            RecordedEvent {
                timestamp_ms: 21,
                event: SerializableEventType::ButtonRelease(ButtonKind::Left),
            },
            RecordedEvent {
                timestamp_ms: 30,
                event: SerializableEventType::MouseMove { x: 110.0, y: 110.0 },
            },
            RecordedEvent {
                timestamp_ms: 40,
                event: SerializableEventType::ButtonPress(ButtonKind::Left),
            },
            RecordedEvent {
                timestamp_ms: 41,
                event: SerializableEventType::ButtonRelease(ButtonKind::Left),
            },
        ];
        let recorder = ScreenRect {
            left: 100.0,
            top: 100.0,
            right: 300.0,
            bottom: 300.0,
        };

        assert_eq!(trim_trailing_window_events(&mut events, recorder), 3);
        assert_eq!(events.len(), 3);
        assert_eq!(
            events.last().map(|event| event.event),
            Some(SerializableEventType::ButtonRelease(ButtonKind::Left))
        );
    }
}
