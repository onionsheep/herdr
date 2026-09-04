#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

use tracing::{info, warn};

use crate::platform::{ClipboardFileSelection, LimitedRead};
use crate::protocol::endpoint::{
    clipboard_file_message, MAX_CLIPBOARD_FILE_NAME_BYTES, MAX_CLIPBOARD_FILE_PAYLOAD,
};
use crate::protocol::ClientClipboardImageTarget;

use super::{write_to_server, ClientError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClipboardFile {
    pub(super) file_name: String,
    pub(super) bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FileProbe<T> {
    Absent,
    Rejected(&'static str),
    Ready(T),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ClipboardBridge {
    File(ClipboardFile),
    Image(crate::platform::ClipboardImage),
    Forward,
}

pub(super) fn select_clipboard_bridge(
    file: FileProbe<ClipboardFile>,
    image: impl FnOnce() -> Option<crate::platform::ClipboardImage>,
) -> ClipboardBridge {
    match file {
        FileProbe::Ready(file) => ClipboardBridge::File(file),
        FileProbe::Absent => image().map_or(ClipboardBridge::Forward, ClipboardBridge::Image),
        FileProbe::Rejected(reason) => {
            warn!(reason, "local clipboard file selection cannot be bridged");
            ClipboardBridge::Forward
        }
    }
}

pub(super) fn read_clipboard_file(selection: ClipboardFileSelection) -> FileProbe<ClipboardFile> {
    match selection {
        ClipboardFileSelection::Absent => FileProbe::Absent,
        ClipboardFileSelection::Rejected => {
            FileProbe::Rejected("clipboard file selection rejected")
        }
        ClipboardFileSelection::One(path) => read_file(path),
    }
}

fn read_file(path: PathBuf) -> FileProbe<ClipboardFile> {
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return FileProbe::Rejected("clipboard file metadata unavailable");
    };
    if !metadata.file_type().is_file() {
        return FileProbe::Rejected("clipboard selection is not a regular file");
    }
    let Some(file_name) = path.file_name() else {
        return FileProbe::Rejected("missing file name");
    };
    let file_name = file_name.to_string_lossy().into_owned();
    if file_name.is_empty() || file_name.len() > MAX_CLIPBOARD_FILE_NAME_BYTES {
        return FileProbe::Rejected("invalid file name length");
    }
    let Ok(file) = std::fs::File::open(&path) else {
        return FileProbe::Rejected("clipboard file cannot be opened");
    };
    let Ok(read) = crate::platform::read_limited_reader(file, MAX_CLIPBOARD_FILE_PAYLOAD) else {
        return FileProbe::Rejected("clipboard file cannot be read");
    };
    match read {
        LimitedRead::Empty => FileProbe::Rejected("clipboard file is empty"),
        LimitedRead::Oversized => FileProbe::Rejected("clipboard file is too large"),
        LimitedRead::Complete(bytes) => FileProbe::Ready(ClipboardFile { file_name, bytes }),
    }
}

pub(super) fn write_remote_file_to_server(
    stream: &mut impl super::ClientMessageSink,
    target: ClientClipboardImageTarget,
    file: ClipboardFile,
    source: &'static str,
) -> Result<(), ClientError> {
    info!(
        bytes = file.bytes.len(),
        file_name = file.file_name,
        source,
        "bridging local file to remote server"
    );
    let message = clipboard_file_message(target, file.file_name, &file.bytes).map_err(|error| {
        ClientError::ConnectionLost(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    write_to_server(stream, &message).map_err(ClientError::ConnectionLost)
}

#[cfg(unix)]
pub(super) fn read_file_from_terminal_drop(
    data: &[u8],
    is_remote_client: bool,
    remote_file_paste_enabled: bool,
) -> FileProbe<ClipboardFile> {
    if !is_remote_client || !remote_file_paste_enabled {
        return FileProbe::Absent;
    }
    let bytes = bracketed_paste_payload(data).unwrap_or(data);
    let Ok(text) = std::str::from_utf8(bytes) else {
        return FileProbe::Absent;
    };
    let Some(text) = normalized_terminal_drop_text(text) else {
        return FileProbe::Rejected("file drop must contain one path");
    };
    let text = unescape_terminal_drop_path(strip_matching_path_quotes(text));
    let path = Path::new(&text);
    if !path.is_absolute() {
        return FileProbe::Rejected("file drop path must be absolute");
    }
    read_file(path.to_path_buf())
}

#[cfg(windows)]
pub(super) fn read_file_from_client_events(
    events: &[crate::protocol::ClientInputEvent],
    is_remote_client: bool,
    remote_file_paste_enabled: bool,
) -> FileProbe<ClipboardFile> {
    if !is_remote_client || !remote_file_paste_enabled {
        return FileProbe::Absent;
    }
    let [crate::protocol::ClientInputEvent::Paste { text }] = events else {
        return FileProbe::Absent;
    };
    let Some(text) = normalized_terminal_drop_text(text) else {
        return FileProbe::Rejected("file drop must contain one path");
    };
    let path = PathBuf::from(strip_matching_path_quotes(text));
    if !path.is_absolute() {
        return FileProbe::Rejected("file drop path must be absolute");
    }
    read_file(path)
}

fn normalized_terminal_drop_text(text: &str) -> Option<&str> {
    let text = text.trim_end_matches(['\r', '\n']);
    (!text.is_empty() && !text.contains(['\r', '\n'])).then_some(text)
}

fn strip_matching_path_quotes(text: &str) -> &str {
    if text.len() < 2 {
        return text;
    }
    match (text.as_bytes().first(), text.as_bytes().last()) {
        (Some(b'\''), Some(b'\'')) | (Some(b'"'), Some(b'"')) => &text[1..text.len() - 1],
        _ => text,
    }
}

#[cfg(unix)]
fn bracketed_paste_payload(data: &[u8]) -> Option<&[u8]> {
    data.strip_prefix(b"\x1b[200~")?.strip_suffix(b"\x1b[201~")
}

#[cfg(unix)]
fn unescape_terminal_drop_path(text: &str) -> String {
    let mut unescaped = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(escaped) = chars.next() {
                unescaped.push(escaped);
            } else {
                unescaped.push(ch);
            }
        } else {
            unescaped.push(ch);
        }
    }
    unescaped
}
