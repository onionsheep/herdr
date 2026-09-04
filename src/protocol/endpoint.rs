//! Stable endpoint compatibility contract for client-owned shells.
//!
//! The endpoint generation is intentionally independent from the private
//! binary protocol used by same-install CLI, direct-terminal, and handoff
//! paths. Generation 1 is the compatibility floor for Local, SSH, and Cloud
//! shell endpoints and must remain available indefinitely unless retired for a
//! security reason. New JSON fields must be optional or have serde defaults;
//! new enum values need an `Unknown` fallback. Unknown named controls are
//! optional and ignored unless negotiated as part of the core.

use std::fmt;
use std::io;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::{
    ClientClipboardImageTarget, ClientMessage, ClientShellSnapshot, ClientSurfaceSize,
    ServerMessage,
};

pub const ENDPOINT_PROTOCOL_GENERATION: u32 = 1;
pub const ENDPOINT_HELLO_KIND: &str = "endpoint.hello.v1";
pub const ENDPOINT_WELCOME_KIND: &str = "endpoint.welcome.v1";
pub const SNAPSHOT_CODEC_V1: &str = "shell.snapshot.v1";
pub const ENDPOINT_SNAPSHOT_KIND: &str = SNAPSHOT_CODEC_V1;
pub const SURFACE_CODEC_V1: &str = "shell.surface.v1";
pub const INPUT_CODEC_V1: &str = "shell.input.semantic.v1";
pub const BLOB_CODEC_V1: &str = "shell.blob.v1";
pub const SURFACE_INTEREST_CAPABILITY: &str = "surface_interest";
pub const PRESENTATION_EFFECTS_FENCE_CAPABILITY: &str = "presentation_effects_fence";
pub const PRESENTATION_EFFECTS_SYNC_KIND: &str = "endpoint.presentation.sync.v1";
pub const PRESENTATION_EFFECTS_READY_KIND: &str = "endpoint.presentation.ready.v1";
pub const HEALTH_CHECK_CAPABILITY: &str = "health_check";
pub const HEALTH_PING_KIND: &str = "endpoint.health.ping.v1";
pub const HEALTH_PONG_KIND: &str = "endpoint.health.pong.v1";

fn default_true() -> bool {
    true
}
pub const CLIPBOARD_FILE_CONTROL_KIND: &str = "clipboard.file.v1";
pub const CLIPBOARD_FILE_CAPABILITY: &str = CLIPBOARD_FILE_CONTROL_KIND;
pub const MAX_CLIPBOARD_FILE_NAME_BYTES: usize = 255;
pub const MAX_CLIPBOARD_FILE_PAYLOAD: usize = super::MAX_CLIPBOARD_IMAGE_PAYLOAD;
const MAX_CLIPBOARD_FILE_JSON_METADATA_BYTES: usize = 4 * 1024;
const MAX_CLIPBOARD_FILE_BASE64_BYTES: usize = MAX_CLIPBOARD_FILE_PAYLOAD.div_ceil(3) * 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointClientHello {
    pub generation: u32,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
    pub surface_size: ClientSurfaceSize,
    pub pixel_mouse: bool,
    pub direct_graphics: bool,
    pub endpoint_keybindings: bool,
    pub mouse_capture: bool,
    #[serde(default = "default_true")]
    pub surface_active: bool,
    #[serde(default)]
    pub snapshot_codecs: Vec<String>,
    #[serde(default)]
    pub surface_codecs: Vec<String>,
    #[serde(default)]
    pub input_codecs: Vec<String>,
    #[serde(default)]
    pub blob_codecs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointHandshakeError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointServerWelcome {
    pub generation: u32,
    pub server_version: String,
    pub snapshot_codec: String,
    pub surface_codec: String,
    pub input_codec: String,
    pub blob_codec: String,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EndpointHandshakeError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointClipboardFile {
    pub target: ClientClipboardImageTarget,
    pub file_name: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedClipboardFile {
    pub target: ClientClipboardImageTarget,
    pub file_name: String,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub enum ClipboardFileDecodeError {
    PayloadTooLarge,
    EmptyPayload,
    InvalidJson(serde_json::Error),
    InvalidFileName,
    InvalidBase64(base64::DecodeError),
}

impl fmt::Display for ClipboardFileDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => formatter.write_str("clipboard file payload is too large"),
            Self::EmptyPayload => formatter.write_str("clipboard file payload is empty"),
            Self::InvalidJson(error) => write!(formatter, "invalid clipboard file JSON: {error}"),
            Self::InvalidFileName => formatter.write_str("invalid clipboard file name"),
            Self::InvalidBase64(error) => {
                write!(formatter, "invalid clipboard file base64: {error}")
            }
        }
    }
}

impl std::error::Error for ClipboardFileDecodeError {}

pub fn clipboard_file_message(
    target: ClientClipboardImageTarget,
    file_name: String,
    data: &[u8],
) -> serde_json::Result<ClientMessage> {
    validate_clipboard_file_name(&file_name).map_err(serde_json::Error::io)?;
    if data.is_empty() {
        return Err(serde_json::Error::io(io::Error::new(
            io::ErrorKind::InvalidData,
            "clipboard file payload is empty",
        )));
    }
    if data.len() > MAX_CLIPBOARD_FILE_PAYLOAD {
        return Err(serde_json::Error::io(io::Error::new(
            io::ErrorKind::InvalidData,
            "clipboard file payload is too large",
        )));
    }

    let payload = EndpointClipboardFile {
        target,
        file_name,
        data_base64: base64::engine::general_purpose::STANDARD.encode(data),
    };
    Ok(ClientMessage::EndpointControl {
        kind: CLIPBOARD_FILE_CONTROL_KIND.into(),
        data: serde_json::to_string(&payload)?,
    })
}

pub fn decode_clipboard_file(data: &str) -> Result<DecodedClipboardFile, ClipboardFileDecodeError> {
    if data.len() > MAX_CLIPBOARD_FILE_BASE64_BYTES + MAX_CLIPBOARD_FILE_JSON_METADATA_BYTES {
        return Err(ClipboardFileDecodeError::PayloadTooLarge);
    }
    let payload: EndpointClipboardFile =
        serde_json::from_str(data).map_err(ClipboardFileDecodeError::InvalidJson)?;
    validate_clipboard_file_name(&payload.file_name)
        .map_err(|_| ClipboardFileDecodeError::InvalidFileName)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload.data_base64)
        .map_err(ClipboardFileDecodeError::InvalidBase64)?;
    if decoded.is_empty() {
        return Err(ClipboardFileDecodeError::EmptyPayload);
    }
    if decoded.len() > MAX_CLIPBOARD_FILE_PAYLOAD {
        return Err(ClipboardFileDecodeError::PayloadTooLarge);
    }

    Ok(DecodedClipboardFile {
        target: payload.target,
        file_name: payload.file_name,
        data: decoded,
    })
}

fn validate_clipboard_file_name(file_name: &str) -> io::Result<()> {
    if file_name.is_empty() || file_name.len() > MAX_CLIPBOARD_FILE_NAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "clipboard file name must contain 1 to 255 UTF-8 bytes",
        ));
    }
    Ok(())
}

pub fn snapshot_message(snapshot: &ClientShellSnapshot) -> serde_json::Result<ServerMessage> {
    Ok(ServerMessage::EndpointControl {
        kind: ENDPOINT_SNAPSHOT_KIND.into(),
        data: serde_json::to_string(snapshot)?,
    })
}

impl EndpointClientHello {
    pub fn supports_required_codecs(&self) -> bool {
        self.snapshot_codecs
            .iter()
            .any(|codec| codec == SNAPSHOT_CODEC_V1)
            && self
                .surface_codecs
                .iter()
                .any(|codec| codec == SURFACE_CODEC_V1)
            && self
                .input_codecs
                .iter()
                .any(|codec| codec == INPUT_CODEC_V1)
            && self.blob_codecs.iter().any(|codec| codec == BLOB_CODEC_V1)
    }
}

impl EndpointServerWelcome {
    pub fn compatible(methods: Vec<String>) -> Self {
        Self {
            generation: ENDPOINT_PROTOCOL_GENERATION,
            server_version: crate::build_info::version(),
            snapshot_codec: SNAPSHOT_CODEC_V1.into(),
            surface_codec: SURFACE_CODEC_V1.into(),
            input_codec: INPUT_CODEC_V1.into(),
            blob_codec: BLOB_CODEC_V1.into(),
            methods,
            capabilities: vec![
                SURFACE_INTEREST_CAPABILITY.into(),
                PRESENTATION_EFFECTS_FENCE_CAPABILITY.into(),
                HEALTH_CHECK_CAPABILITY.into(),
                CLIPBOARD_FILE_CAPABILITY.into(),
            ],
            error: None,
        }
    }

    pub fn incompatible(code: &str, message: impl Into<String>) -> Self {
        Self {
            generation: ENDPOINT_PROTOCOL_GENERATION,
            server_version: crate::build_info::version(),
            snapshot_codec: SNAPSHOT_CODEC_V1.into(),
            surface_codec: SURFACE_CODEC_V1.into(),
            input_codec: INPUT_CODEC_V1.into(),
            blob_codec: BLOB_CODEC_V1.into(),
            methods: Vec::new(),
            capabilities: Vec::new(),
            error: Some(EndpointHandshakeError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> EndpointClientHello {
        EndpointClientHello {
            generation: ENDPOINT_PROTOCOL_GENERATION,
            cell_width_px: 8,
            cell_height_px: 16,
            surface_size: ClientSurfaceSize { cols: 80, rows: 24 },
            pixel_mouse: true,
            direct_graphics: false,
            endpoint_keybindings: false,
            mouse_capture: true,
            surface_active: true,
            snapshot_codecs: vec![SNAPSHOT_CODEC_V1.into()],
            surface_codecs: vec![SURFACE_CODEC_V1.into()],
            input_codecs: vec![INPUT_CODEC_V1.into()],
            blob_codecs: vec![BLOB_CODEC_V1.into()],
        }
    }

    fn snapshot() -> ClientShellSnapshot {
        ClientShellSnapshot {
            boot_id: "boot".into(),
            revision: 1,
            config_diagnostic: None,
            product_announcement: None,
            update_available: None,
            update_install_command: "herdr update".into(),
            server_keybindings_toml: None,
            latest_release_notes_available: false,
            integration_updates_available: false,
            worktree_directory: String::new(),
            release_notes: None,
            focused_workspace_id: None,
            focused_tab_id: None,
            focused_pane_id: None,
            tab_bar_right: Vec::new(),
            tab_bar_right_separator: String::new(),
            agent_view_label: None,
            agent_order: Vec::new(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes: Vec::new(),
            agents: Vec::new(),
            commands: Vec::new(),
        }
    }

    #[test]
    fn hello_ignores_future_named_fields() {
        let mut value = serde_json::to_value(hello()).unwrap();
        value["future_feature"] = serde_json::json!({"enabled": true});
        let decoded: EndpointClientHello = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, hello());
    }

    #[test]
    fn frozen_generation_one_handshake_decodes() {
        let hello: EndpointClientHello = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/endpoint-hello-v1.json"
        )))
        .unwrap();
        assert!(hello.supports_required_codecs());

        let welcome: EndpointServerWelcome = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/endpoint-welcome-v1.json"
        )))
        .unwrap();
        assert_eq!(welcome.generation, ENDPOINT_PROTOCOL_GENERATION);
        assert_eq!(welcome.snapshot_codec, SNAPSHOT_CODEC_V1);
        assert_eq!(welcome.surface_codec, SURFACE_CODEC_V1);
        assert_eq!(welcome.input_codec, INPUT_CODEC_V1);
        assert_eq!(welcome.blob_codec, BLOB_CODEC_V1);
        assert!(welcome.capabilities.is_empty());
    }

    #[test]
    fn frozen_welcome_without_capabilities_decodes_with_empty_capabilities() {
        let welcome: EndpointServerWelcome = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/endpoint-welcome-v1.json"
        )))
        .unwrap();

        assert!(welcome.capabilities.is_empty());
    }

    #[test]
    fn compatible_welcome_advertises_single_file_clipboard_bridge() {
        let welcome = EndpointServerWelcome::compatible(vec!["pane.focus".into()]);

        assert!(welcome
            .capabilities
            .iter()
            .any(|value| value == CLIPBOARD_FILE_CAPABILITY));
    }

    #[test]
    fn clipboard_file_control_roundtrips_binary_bytes_and_target() {
        let message = clipboard_file_message(
            crate::protocol::ClientClipboardImageTarget::Pane("w1:p1".into()),
            "报告 2026.pdf".into(),
            b"\0binary\xff",
        )
        .unwrap();
        let crate::protocol::ClientMessage::EndpointControl { kind, data } = message else {
            panic!("file bridge must use endpoint control");
        };

        assert_eq!(kind, CLIPBOARD_FILE_CONTROL_KIND);
        let decoded = decode_clipboard_file(&data).unwrap();
        assert_eq!(
            decoded.target,
            crate::protocol::ClientClipboardImageTarget::Pane("w1:p1".into())
        );
        assert_eq!(decoded.file_name, "报告 2026.pdf");
        assert_eq!(decoded.data, b"\0binary\xff");
    }

    #[test]
    fn clipboard_file_control_rejects_invalid_payloads() {
        assert!(decode_clipboard_file(
            r#"{"target":"DirectTerminal","file_name":"report.pdf","data_base64":"not base64!"}"#
        )
        .is_err());
        assert!(
            decode_clipboard_file(r#"{"target":"DirectTerminal","data_base64":"dGVzdA=="}"#)
                .is_err()
        );
        assert!(clipboard_file_message(
            crate::protocol::ClientClipboardImageTarget::DirectTerminal,
            String::new(),
            b"contents",
        )
        .is_err());
        assert!(clipboard_file_message(
            crate::protocol::ClientClipboardImageTarget::DirectTerminal,
            "empty.bin".into(),
            b"",
        )
        .is_err());
        assert!(matches!(
            decode_clipboard_file(
                r#"{"target":"DirectTerminal","file_name":"empty.bin","data_base64":""}"#
            ),
            Err(ClipboardFileDecodeError::EmptyPayload)
        ));
        assert!(clipboard_file_message(
            crate::protocol::ClientClipboardImageTarget::DirectTerminal,
            "x".repeat(MAX_CLIPBOARD_FILE_NAME_BYTES + 1),
            b"contents",
        )
        .is_err());
        assert!(clipboard_file_message(
            crate::protocol::ClientClipboardImageTarget::DirectTerminal,
            "huge.bin".into(),
            &vec![0; MAX_CLIPBOARD_FILE_PAYLOAD + 1],
        )
        .is_err());
    }

    #[test]
    fn frozen_generation_one_snapshot_decodes() {
        let snapshot: ClientShellSnapshot = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/endpoint-snapshot-v1.json"
        )))
        .unwrap();
        assert_eq!(snapshot.boot_id, "boot-v1");
        assert_eq!(
            snapshot.workspaces[0].agent_status,
            crate::api::schema::AgentStatus::Unknown
        );
    }

    #[test]
    fn snapshot_message_uses_named_json_control() {
        let snapshot = snapshot();
        let ServerMessage::EndpointControl { kind, data } = snapshot_message(&snapshot).unwrap()
        else {
            panic!("snapshot should use endpoint control");
        };
        assert_eq!(kind, ENDPOINT_SNAPSHOT_KIND);
        let decoded: ClientShellSnapshot = serde_json::from_str(&data).unwrap();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn snapshot_json_tolerates_future_fields_and_command_actions() {
        let mut snapshot = match snapshot_message(&snapshot()).unwrap() {
            ServerMessage::EndpointControl { data, .. } => {
                serde_json::from_str::<serde_json::Value>(&data).unwrap()
            }
            _ => unreachable!(),
        };
        snapshot["future_projection"] = serde_json::json!({"enabled": true});
        snapshot["commands"] = serde_json::json!([{
            "command_id": "future",
            "binding_label": "x",
            "binding_labels": ["x"],
            "action": "FutureAction",
            "description": null
        }]);

        let decoded: ClientShellSnapshot = serde_json::from_value(snapshot).unwrap();
        assert_eq!(
            decoded.commands[0].action,
            crate::protocol::ClientShellCommandAction::Unknown
        );
    }

    #[test]
    fn legacy_hello_defaults_to_an_active_surface() {
        let mut value = serde_json::to_value(hello()).unwrap();
        value.as_object_mut().unwrap().remove("surface_active");
        let decoded: EndpointClientHello = serde_json::from_value(value).unwrap();
        assert!(decoded.surface_active);
    }

    #[test]
    fn compatible_server_advertises_endpoint_lifecycle_capabilities() {
        let welcome = EndpointServerWelcome::compatible(Vec::new());
        assert_eq!(
            welcome.capabilities,
            vec![
                SURFACE_INTEREST_CAPABILITY.to_string(),
                PRESENTATION_EFFECTS_FENCE_CAPABILITY.to_string(),
                HEALTH_CHECK_CAPABILITY.to_string(),
                CLIPBOARD_FILE_CAPABILITY.to_string(),
            ]
        );
    }

    #[test]
    fn required_codecs_are_explicit() {
        let mut value = hello();
        assert!(value.supports_required_codecs());
        value.snapshot_codecs.clear();
        assert!(!value.supports_required_codecs());

        let mut value = hello();
        value.surface_codecs.clear();
        assert!(!value.supports_required_codecs());

        let mut value = hello();
        value.input_codecs.clear();
        assert!(!value.supports_required_codecs());

        let mut value = hello();
        value.blob_codecs.clear();
        assert!(!value.supports_required_codecs());
    }

    #[test]
    fn welcome_ignores_future_named_fields() {
        let welcome = EndpointServerWelcome::compatible(vec!["pane.close".into()]);
        let mut value = serde_json::to_value(&welcome).unwrap();
        value["future_service"] = serde_json::json!("v2");
        let decoded: EndpointServerWelcome = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, welcome);
    }
}
