# Mouse Macro Recorder

A simple, lightweight **Windows** mouse macro recorder written in **Rust**.

Records mouse movement, left / right / middle button presses & releases, and mouse wheel events.  
Supports long recordings (30+ minutes) with accurate timing playback.

## Features

- ▶ **Start Record** – begins capturing all mouse activity globally
- ⏹ **Stop Record** – ends the current recording
- ⏯ **Play Back** – faithfully replays the recorded sequence with original timing
- Visible **Pause**, **Resume**, and **Stop Playback** controls
- Global playback controls: **F1** pauses, **F2** resumes, and **Space** stops
- Save recordings as human-readable `.txt` macro files and load them in a later session
- Live event counter and duration display while recording
- Memory-efficient storage of events (suitable for half-hour+ macros)
- Clean, native GUI built with `egui` / `eframe`

## Requirements

- Windows 10 / 11
- Rust (stable) – install from https://rustup.rs
- Visual Studio Build Tools (for MSVC linker) if not already installed

## Build & Run

```bash
cd mouse_macro_recorder
cargo build --release
```

The executable will be at:

```
target/release/mouse_macro_recorder.exe
```

Or simply:

```bash
cargo run --release
```

**Tip:** For best results when playing back into games or elevated applications, right-click the `.exe` → **Run as administrator**.

## How it works

1. A global low-level mouse hook (via the `rdev` crate) listens for all mouse events system-wide.
2. While recording, every relevant event is stored with a high-resolution relative timestamp (milliseconds since record start).
3. On playback the events are re-injected using the Windows input simulation APIs, sleeping the exact duration between them so timing is preserved.

Playback uses an interruptible scheduler, so time spent paused is not counted toward event delays. Pause and stop are checked before every event, including events that share the same millisecond timestamp.

## Saving and Loading Macros

1. Record a macro and click **Stop Record**.
2. Click **Save Macro...** and choose a `.txt` file.
3. In a later session, click **Load Macro...**, select that file, and click **Play**.

Macro files are pretty-printed UTF-8 JSON with a versioned format. They can be read or edited in a text editor; invalid or out-of-order events are rejected without replacing the macro currently in memory.

## Limitations / Notes

- Only **mouse** events are recorded (keyboard is ignored by design).
- Extremely dense mouse movement (hundreds of events per second) will use more memory; 30-minute recordings are typically only a few dozen MB.
- Some applications (especially those running as admin or with anti-cheat) may ignore simulated input unless this tool is also elevated.
- Global hotkeys are active only during playback. The GUI buttons remain available if a keyboard uses an `Fn` layer for F1/F2.

## License

MIT / Apache-2.0 – free to use and modify.
