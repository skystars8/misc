use super::{ButtonKind, RecordedEvent, SerializableEventType};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const FORMAT_NAME: &str = "mouse_macro_recorder";
const FORMAT_VERSION: u32 = 1;
const MAX_FILE_SIZE: u64 = 256 * 1024 * 1024;
const MAX_ABS_COORDINATE: f64 = 32_767.0;
const MAX_ABS_WHEEL_DELTA: i64 = 273;

#[derive(Debug, Deserialize)]
struct MacroHeader {
    format: String,
    version: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct MacroFileV1 {
    format: String,
    version: u32,
    events: Vec<MacroEventV1>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MacroEventV1 {
    at_ms: u64,
    #[serde(flatten)]
    action: MacroActionV1,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum MacroActionV1 {
    MouseMove { x: f64, y: f64 },
    ButtonPress { button: FileButton },
    ButtonRelease { button: FileButton },
    Wheel { delta_x: i64, delta_y: i64 },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FileButton {
    Left,
    Right,
    Middle,
}

impl From<ButtonKind> for FileButton {
    fn from(button: ButtonKind) -> Self {
        match button {
            ButtonKind::Left => Self::Left,
            ButtonKind::Right => Self::Right,
            ButtonKind::Middle => Self::Middle,
        }
    }
}

impl From<FileButton> for ButtonKind {
    fn from(button: FileButton) -> Self {
        match button {
            FileButton::Left => Self::Left,
            FileButton::Right => Self::Right,
            FileButton::Middle => Self::Middle,
        }
    }
}

impl From<&RecordedEvent> for MacroEventV1 {
    fn from(recorded: &RecordedEvent) -> Self {
        let action = match recorded.event {
            SerializableEventType::MouseMove { x, y } => MacroActionV1::MouseMove { x, y },
            SerializableEventType::ButtonPress(button) => MacroActionV1::ButtonPress {
                button: button.into(),
            },
            SerializableEventType::ButtonRelease(button) => MacroActionV1::ButtonRelease {
                button: button.into(),
            },
            SerializableEventType::Wheel { delta_x, delta_y } => {
                MacroActionV1::Wheel { delta_x, delta_y }
            }
        };

        Self {
            at_ms: recorded.timestamp_ms,
            action,
        }
    }
}

impl From<MacroEventV1> for RecordedEvent {
    fn from(saved: MacroEventV1) -> Self {
        let event = match saved.action {
            MacroActionV1::MouseMove { x, y } => SerializableEventType::MouseMove { x, y },
            MacroActionV1::ButtonPress { button } => {
                SerializableEventType::ButtonPress(button.into())
            }
            MacroActionV1::ButtonRelease { button } => {
                SerializableEventType::ButtonRelease(button.into())
            }
            MacroActionV1::Wheel { delta_x, delta_y } => {
                SerializableEventType::Wheel { delta_x, delta_y }
            }
        };

        Self {
            timestamp_ms: saved.at_ms,
            event,
        }
    }
}

pub(crate) fn validate_events(events: &[RecordedEvent]) -> Result<(), String> {
    for (index, event) in events.iter().enumerate() {
        if index > 0 && event.timestamp_ms < events[index - 1].timestamp_ms {
            return Err(format!(
                "event {} has a timestamp earlier than the previous event",
                index + 1
            ));
        }

        match event.event {
            SerializableEventType::MouseMove { x, y } => {
                if !x.is_finite() || !y.is_finite() {
                    return Err(format!(
                        "event {} contains a non-finite mouse coordinate",
                        index + 1
                    ));
                }
                if !(-MAX_ABS_COORDINATE..=MAX_ABS_COORDINATE).contains(&x)
                    || !(-MAX_ABS_COORDINATE..=MAX_ABS_COORDINATE).contains(&y)
                {
                    return Err(format!(
                        "event {} has a mouse coordinate outside the safe Windows range",
                        index + 1
                    ));
                }
            }
            SerializableEventType::Wheel { delta_x, delta_y } => {
                let safe = -MAX_ABS_WHEEL_DELTA..=MAX_ABS_WHEEL_DELTA;
                if !safe.contains(&delta_x) || !safe.contains(&delta_y) {
                    return Err(format!(
                        "event {} has a wheel delta outside the safe Windows range",
                        index + 1
                    ));
                }
            }
            _ => {}
        }
    }

    Ok(())
}

pub(crate) fn encode_macro(events: &[RecordedEvent]) -> Result<String, String> {
    validate_events(events)?;
    let saved = MacroFileV1 {
        format: FORMAT_NAME.to_owned(),
        version: FORMAT_VERSION,
        events: events.iter().map(MacroEventV1::from).collect(),
    };

    serde_json::to_string_pretty(&saved)
        .map_err(|error| format!("could not encode the macro: {error}"))
}

pub(crate) fn decode_macro(text: &str) -> Result<Vec<RecordedEvent>, String> {
    let header: MacroHeader =
        serde_json::from_str(text).map_err(|error| format!("invalid macro text at {error}"))?;

    if header.format != FORMAT_NAME {
        return Err(format!(
            "not a Mouse Macro Recorder file (format was {:?})",
            header.format
        ));
    }
    if header.version != FORMAT_VERSION {
        return Err(format!(
            "macro version {} is not supported (expected version {})",
            header.version, FORMAT_VERSION
        ));
    }

    let saved: MacroFileV1 = serde_json::from_str(text)
        .map_err(|error| format!("invalid version 1 macro at {error}"))?;
    let events: Vec<RecordedEvent> = saved.events.into_iter().map(RecordedEvent::from).collect();
    validate_events(&events)?;
    Ok(events)
}

#[cfg(target_os = "windows")]
fn replace_file(temp_path: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let temp_wide: Vec<u16> = temp_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();

    // Both buffers are NUL-terminated and remain alive for this synchronous
    // call. WRITE_THROUGH asks Windows not to return before the move is durable.
    if unsafe {
        MoveFileExW(
            temp_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn replace_file(temp_path: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temp_path, destination)
}

fn temporary_save_path(path: &Path, attempt: u32) -> Result<PathBuf, String> {
    let file_name = path
        .file_name()
        .ok_or_else(|| "the selected save path has no file name".to_owned())?;
    let mut temp_name = file_name.to_os_string();
    temp_name.push(format!(".{}.{}.tmp", std::process::id(), attempt));
    Ok(path.with_file_name(temp_name))
}

pub(crate) fn save_macro(path: &Path, events: &[RecordedEvent]) -> Result<(), String> {
    let text = encode_macro(events)?;
    let mut opened = None;
    for attempt in 0..100 {
        let temp_path = temporary_save_path(path, attempt)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => {
                opened = Some((temp_path, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("could not create a temporary save file: {error}")),
        }
    }

    let (temp_path, mut file) =
        opened.ok_or_else(|| "could not find an available temporary save-file name".to_owned())?;
    let write_result = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);

    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("could not write the macro safely: {error}"));
    }
    if let Err(error) = replace_file(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("could not replace the destination file: {error}"));
    }

    Ok(())
}

pub(crate) fn load_macro(path: &Path) -> Result<Vec<RecordedEvent>, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("could not read the file: {error}"))?;
    if metadata.len() > MAX_FILE_SIZE {
        return Err(format!(
            "the macro is too large ({:.1} MiB; maximum is 256 MiB)",
            metadata.len() as f64 / (1024.0 * 1024.0)
        ));
    }

    let text = fs::read_to_string(path)
        .map_err(|error| format!("could not read the file as UTF-8 text: {error}"))?;
    decode_macro(&text)
}

#[cfg(test)]
mod tests {
    use super::{decode_macro, encode_macro, load_macro, save_macro};
    use crate::{ButtonKind, RecordedEvent, SerializableEventType};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempMacro(PathBuf);

    impl Drop for TempMacro {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn example_events() -> Vec<RecordedEvent> {
        vec![
            RecordedEvent {
                timestamp_ms: 0,
                event: SerializableEventType::MouseMove { x: -20.5, y: 40.0 },
            },
            RecordedEvent {
                timestamp_ms: 12,
                event: SerializableEventType::ButtonPress(ButtonKind::Left),
            },
            RecordedEvent {
                timestamp_ms: 12,
                event: SerializableEventType::ButtonRelease(ButtonKind::Left),
            },
            RecordedEvent {
                timestamp_ms: 18,
                event: SerializableEventType::ButtonPress(ButtonKind::Right),
            },
            RecordedEvent {
                timestamp_ms: 19,
                event: SerializableEventType::ButtonRelease(ButtonKind::Right),
            },
            RecordedEvent {
                timestamp_ms: 21,
                event: SerializableEventType::ButtonPress(ButtonKind::Middle),
            },
            RecordedEvent {
                timestamp_ms: 22,
                event: SerializableEventType::ButtonRelease(ButtonKind::Middle),
            },
            RecordedEvent {
                timestamp_ms: 30,
                event: SerializableEventType::Wheel {
                    delta_x: -1,
                    delta_y: 2,
                },
            },
        ]
    }

    #[test]
    fn text_format_round_trips_every_event_type() {
        let events = example_events();
        let text = encode_macro(&events).expect("macro should encode");

        assert!(text.contains("\"format\": \"mouse_macro_recorder\""));
        assert!(text.contains("\"type\": \"mouse_move\""));
        assert!(text.contains("\"button\": \"middle\""));
        assert_eq!(decode_macro(&text).expect("macro should decode"), events);
    }

    #[test]
    fn unsupported_version_has_a_specific_error() {
        let text = encode_macro(&example_events())
            .expect("macro should encode")
            .replacen("\"version\": 1", "\"version\": 2", 1);

        let error = decode_macro(&text).expect_err("version 2 should be rejected");
        assert!(error.contains("version 2 is not supported"));
    }

    #[test]
    fn wrong_format_is_rejected() {
        let text = encode_macro(&example_events())
            .expect("macro should encode")
            .replacen("mouse_macro_recorder", "some_other_app", 1);

        let error = decode_macro(&text).expect_err("foreign format should be rejected");
        assert!(error.contains("not a Mouse Macro Recorder file"));
    }

    #[test]
    fn decreasing_timestamps_are_rejected() {
        let text = r#"{
            "format": "mouse_macro_recorder",
            "version": 1,
            "events": [
                { "at_ms": 20, "type": "wheel", "delta_x": 0, "delta_y": 1 },
                { "at_ms": 10, "type": "wheel", "delta_x": 0, "delta_y": -1 }
            ]
        }"#;

        let error = decode_macro(text).expect_err("timestamps should be monotonic");
        assert!(error.contains("timestamp earlier"));
    }

    #[test]
    fn non_finite_coordinates_are_rejected_before_saving() {
        let events = vec![RecordedEvent {
            timestamp_ms: 0,
            event: SerializableEventType::MouseMove {
                x: f64::NAN,
                y: 1.0,
            },
        }];

        let error = encode_macro(&events).expect_err("NaN should not be saved");
        assert!(error.contains("non-finite"));
    }

    #[test]
    fn unsafe_windows_input_ranges_are_rejected() {
        let coordinate = vec![RecordedEvent {
            timestamp_ms: 0,
            event: SerializableEventType::MouseMove {
                x: 32_768.0,
                y: 1.0,
            },
        }];
        assert!(encode_macro(&coordinate)
            .expect_err("overflowing coordinates should not be saved")
            .contains("safe Windows range"));

        let wheel = vec![RecordedEvent {
            timestamp_ms: 0,
            event: SerializableEventType::Wheel {
                delta_x: 0,
                delta_y: 274,
            },
        }];
        assert!(encode_macro(&wheel)
            .expect_err("overflowing wheel deltas should not be saved")
            .contains("safe Windows range"));
    }

    #[test]
    fn saved_file_can_be_replaced_and_loaded() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mouse_macro_recorder_test_{}_{}.txt",
            std::process::id(),
            unique
        ));
        let temp = TempMacro(path);
        let events = example_events();

        save_macro(&temp.0, &events).expect("first save should succeed");
        assert!(fs::read_to_string(&temp.0)
            .expect("saved macro should be UTF-8")
            .contains("mouse_macro_recorder"));

        let replacement = events[..2].to_vec();
        save_macro(&temp.0, &replacement).expect("replacement save should succeed");
        assert_eq!(
            load_macro(&temp.0).expect("replacement should load"),
            replacement
        );
    }
}
