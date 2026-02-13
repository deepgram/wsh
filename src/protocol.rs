//! Unix socket protocol for wsh client/server communication.
//!
//! Wire format: `[type: u8][length: u32 big-endian][payload: bytes]`
//!
//! Control frames carry JSON payloads; data frames carry raw bytes.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Frame type byte values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    // Control frames (JSON payload)
    CreateSession = 0x01,
    CreateSessionResponse = 0x02,
    AttachSession = 0x03,
    AttachSessionResponse = 0x04,
    Detach = 0x05,
    Resize = 0x06,
    Error = 0x07,
    ListSessions = 0x08,
    ListSessionsResponse = 0x09,
    KillSession = 0x0A,
    KillSessionResponse = 0x0B,
    DetachSession = 0x0C,
    DetachSessionResponse = 0x0D,

    // Data frames (raw bytes payload)
    PtyOutput = 0x10,
    StdinInput = 0x11,

    // Visual state sync frames (JSON payload, server → client)
    OverlaySync = 0x12,
    PanelSync = 0x13,
}

impl FrameType {
    pub fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(Self::CreateSession),
            0x02 => Some(Self::CreateSessionResponse),
            0x03 => Some(Self::AttachSession),
            0x04 => Some(Self::AttachSessionResponse),
            0x05 => Some(Self::Detach),
            0x06 => Some(Self::Resize),
            0x07 => Some(Self::Error),
            0x08 => Some(Self::ListSessions),
            0x09 => Some(Self::ListSessionsResponse),
            0x0A => Some(Self::KillSession),
            0x0B => Some(Self::KillSessionResponse),
            0x0C => Some(Self::DetachSession),
            0x0D => Some(Self::DetachSessionResponse),
            0x10 => Some(Self::PtyOutput),
            0x11 => Some(Self::StdinInput),
            0x12 => Some(Self::OverlaySync),
            0x13 => Some(Self::PanelSync),
            _ => None,
        }
    }
}

/// Maximum frame payload size (16 MiB). Prevents OOM on malformed data.
const MAX_PAYLOAD_SIZE: u32 = 16 * 1024 * 1024;

/// A protocol frame with a type tag and payload.
#[derive(Debug, Clone)]
pub struct Frame {
    pub frame_type: FrameType,
    pub payload: Bytes,
}

impl Frame {
    /// Create a new frame.
    pub fn new(frame_type: FrameType, payload: Bytes) -> Self {
        Self {
            frame_type,
            payload,
        }
    }

    /// Create a control frame from a serializable message.
    pub fn control<T: Serialize>(frame_type: FrameType, msg: &T) -> Result<Self, serde_json::Error> {
        let payload = serde_json::to_vec(msg)?;
        Ok(Self::new(frame_type, Bytes::from(payload)))
    }

    /// Create a data frame (PtyOutput or StdinInput).
    pub fn data(frame_type: FrameType, data: Bytes) -> Self {
        Self::new(frame_type, data)
    }

    /// Encode this frame into bytes.
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(5 + self.payload.len());
        buf.put_u8(self.frame_type as u8);
        buf.put_u32(self.payload.len() as u32);
        buf.put(self.payload.as_ref());
        buf.freeze()
    }

    /// Write this frame to an async writer.
    pub async fn write_to<W: AsyncWriteExt + Unpin>(&self, writer: &mut W) -> io::Result<()> {
        let encoded = self.encode();
        writer.write_all(&encoded).await?;
        writer.flush().await
    }

    /// Read a frame from an async reader.
    pub async fn read_from<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Self> {
        let type_byte = reader.read_u8().await?;
        let frame_type = FrameType::from_u8(type_byte).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown frame type: 0x{:02x}", type_byte),
            )
        })?;

        let length = reader.read_u32().await?;
        if length > MAX_PAYLOAD_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame payload too large: {} bytes", length),
            ));
        }

        let mut payload = vec![0u8; length as usize];
        reader.read_exact(&mut payload).await?;

        Ok(Self {
            frame_type,
            payload: Bytes::from(payload),
        })
    }

    /// Decode a frame from a byte buffer (synchronous, for testing).
    pub fn decode(mut data: &[u8]) -> io::Result<Self> {
        if data.len() < 5 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "frame too short",
            ));
        }

        let type_byte = data.get_u8();
        let frame_type = FrameType::from_u8(type_byte).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown frame type: 0x{:02x}", type_byte),
            )
        })?;

        let length = data.get_u32();
        if length > MAX_PAYLOAD_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame payload too large: {} bytes", length),
            ));
        }

        if data.remaining() < length as usize {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete frame payload",
            ));
        }

        let payload = Bytes::copy_from_slice(&data[..length as usize]);

        Ok(Self {
            frame_type,
            payload,
        })
    }

    /// Parse the payload as a JSON control message.
    pub fn parse_json<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.payload)
    }
}

// ── Control message types ──────────────────────────────────────────

/// Client → Server: request to create a new session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionMsg {
    pub name: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Option<std::collections::HashMap<String, String>>,
    pub rows: u16,
    pub cols: u16,
}

/// Server → Client: response after session creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionResponseMsg {
    pub name: String,
    pub pid: Option<u32>,
    pub rows: u16,
    pub cols: u16,
}

/// Client → Server: request to attach to an existing session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachSessionMsg {
    pub name: String,
    pub scrollback: ScrollbackRequest,
    pub rows: u16,
    pub cols: u16,
}

/// How much scrollback to replay on attach.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollbackRequest {
    None,
    Lines(usize),
    All,
}

/// Server → Client: response after attaching to a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachSessionResponseMsg {
    pub name: String,
    pub rows: u16,
    pub cols: u16,
    /// Raw terminal bytes for scrollback replay (base64-encoded in JSON).
    #[serde(with = "base64_bytes")]
    pub scrollback: Vec<u8>,
    /// Raw terminal bytes for current screen state (base64-encoded in JSON).
    #[serde(with = "base64_bytes")]
    pub screen: Vec<u8>,
    /// Current input routing mode (passthrough or capture).
    #[serde(default)]
    pub input_mode: crate::input::mode::Mode,
    /// Current screen mode (normal or alt).
    #[serde(default)]
    pub screen_mode: crate::overlay::ScreenMode,
    /// ID of the currently focused overlay/panel, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_id: Option<String>,
}

/// Client → Server: resize notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResizeMsg {
    pub rows: u16,
    pub cols: u16,
}

/// Server → Client: error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMsg {
    pub code: String,
    pub message: String,
}

/// Client → Server: request to list all sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSessionsMsg {}

/// Server → Client: response with the list of sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSessionsResponseMsg {
    pub sessions: Vec<SessionInfoMsg>,
}

/// Info about a single session, used in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoMsg {
    pub name: String,
    pub pid: Option<u32>,
    pub command: String,
    pub rows: u16,
    pub cols: u16,
    pub clients: usize,
}

/// Client → Server: request to kill (destroy) a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KillSessionMsg {
    pub name: String,
}

/// Server → Client: confirmation that a session was killed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KillSessionResponseMsg {
    pub name: String,
}

/// Client → Server: request to detach (signal) a session without destroying it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetachSessionMsg {
    pub name: String,
}

/// Server → Client: confirmation that a session was detached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetachSessionResponseMsg {
    pub name: String,
}

/// Server → Client: full overlay state sync.
///
/// Sent when any overlay changes, contains ALL current overlays.
/// Full-state sync is simpler than delta updates and overlay counts are small.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlaySyncMsg {
    pub overlays: Vec<crate::overlay::Overlay>,
}

/// Server → Client: full panel state sync.
///
/// Sent when any panel changes, contains ALL current panels plus layout info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelSyncMsg {
    pub panels: Vec<crate::panel::Panel>,
    pub scroll_region_top: u16,
    pub scroll_region_bottom: u16,
}

/// Visual state change notification (internal, not a wire type).
#[derive(Debug, Clone)]
pub enum VisualUpdate {
    OverlaysChanged,
    PanelsChanged,
}

/// Serde helper for base64-encoded byte vectors in JSON.
mod base64_bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_type_round_trip() {
        let types = [
            FrameType::CreateSession,
            FrameType::CreateSessionResponse,
            FrameType::AttachSession,
            FrameType::AttachSessionResponse,
            FrameType::Detach,
            FrameType::Resize,
            FrameType::Error,
            FrameType::ListSessions,
            FrameType::ListSessionsResponse,
            FrameType::KillSession,
            FrameType::KillSessionResponse,
            FrameType::DetachSession,
            FrameType::DetachSessionResponse,
            FrameType::PtyOutput,
            FrameType::StdinInput,
            FrameType::OverlaySync,
            FrameType::PanelSync,
        ];
        for ft in types {
            let byte = ft as u8;
            let decoded = FrameType::from_u8(byte).unwrap();
            assert_eq!(decoded, ft);
        }
    }

    #[test]
    fn frame_type_invalid_byte() {
        assert!(FrameType::from_u8(0xFF).is_none());
        assert!(FrameType::from_u8(0x00).is_none());
        assert!(FrameType::from_u8(0x0E).is_none());
    }

    #[test]
    fn frame_encode_decode_round_trip() {
        let frame = Frame::new(FrameType::StdinInput, Bytes::from("hello world"));
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded).unwrap();
        assert_eq!(decoded.frame_type, FrameType::StdinInput);
        assert_eq!(decoded.payload, Bytes::from("hello world"));
    }

    #[test]
    fn frame_encode_decode_empty_payload() {
        let frame = Frame::new(FrameType::Detach, Bytes::new());
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded).unwrap();
        assert_eq!(decoded.frame_type, FrameType::Detach);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn frame_encode_decode_large_payload() {
        let data = vec![0xABu8; 65536];
        let frame = Frame::new(FrameType::PtyOutput, Bytes::from(data.clone()));
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded).unwrap();
        assert_eq!(decoded.frame_type, FrameType::PtyOutput);
        assert_eq!(decoded.payload.len(), 65536);
        assert_eq!(decoded.payload.as_ref(), data.as_slice());
    }

    #[test]
    fn frame_decode_too_short() {
        let result = Frame::decode(&[0x01, 0x00, 0x00]);
        assert!(result.is_err());
    }

    #[test]
    fn frame_decode_invalid_type() {
        let data = [0xFF, 0x00, 0x00, 0x00, 0x00];
        let result = Frame::decode(&data);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("unknown frame type"));
    }

    #[test]
    fn frame_decode_incomplete_payload() {
        // Header says 10 bytes but only 3 provided
        let data = [0x10, 0x00, 0x00, 0x00, 0x0A, 0x01, 0x02, 0x03];
        let result = Frame::decode(&data);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn frame_async_write_read_round_trip() {
        let frame = Frame::new(FrameType::PtyOutput, Bytes::from("async test data"));

        let mut buf = Vec::new();
        frame.write_to(&mut buf).await.unwrap();

        let mut cursor = io::Cursor::new(buf);
        let decoded = Frame::read_from(&mut cursor).await.unwrap();
        assert_eq!(decoded.frame_type, FrameType::PtyOutput);
        assert_eq!(decoded.payload, Bytes::from("async test data"));
    }

    #[tokio::test]
    async fn frame_async_read_eof() {
        let mut cursor = io::Cursor::new(Vec::<u8>::new());
        let result = Frame::read_from(&mut cursor).await;
        assert!(result.is_err());
    }

    #[test]
    fn control_frame_create_session() {
        let msg = CreateSessionMsg {
            name: Some("test".to_string()),
            command: None,
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        };
        let frame = Frame::control(FrameType::CreateSession, &msg).unwrap();
        assert_eq!(frame.frame_type, FrameType::CreateSession);

        let decoded: CreateSessionMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.name.as_deref(), Some("test"));
        assert_eq!(decoded.rows, 24);
        assert_eq!(decoded.cols, 80);
    }

    #[test]
    fn control_frame_create_session_response() {
        let msg = CreateSessionResponseMsg {
            name: "session-0".to_string(),
            pid: None,
            rows: 40,
            cols: 120,
        };
        let frame = Frame::control(FrameType::CreateSessionResponse, &msg).unwrap();
        let decoded: CreateSessionResponseMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.name, "session-0");
        assert_eq!(decoded.pid, None);
        assert_eq!(decoded.rows, 40);
        assert_eq!(decoded.cols, 120);
    }

    #[test]
    fn control_frame_attach_session() {
        let msg = AttachSessionMsg {
            name: "my-session".to_string(),
            scrollback: ScrollbackRequest::Lines(100),
            rows: 24,
            cols: 80,
        };
        let frame = Frame::control(FrameType::AttachSession, &msg).unwrap();
        let decoded: AttachSessionMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.name, "my-session");
        assert!(matches!(decoded.scrollback, ScrollbackRequest::Lines(100)));
    }

    #[test]
    fn control_frame_attach_session_response_with_scrollback() {
        let msg = AttachSessionResponseMsg {
            name: "test".to_string(),
            rows: 24,
            cols: 80,
            scrollback: b"scrollback data here".to_vec(),
            screen: b"\x1b[H\x1b[2Jscreen".to_vec(),
            input_mode: crate::input::mode::Mode::Passthrough,
            screen_mode: crate::overlay::ScreenMode::Normal,
            focused_id: None,
        };
        let frame = Frame::control(FrameType::AttachSessionResponse, &msg).unwrap();
        let decoded: AttachSessionResponseMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.name, "test");
        assert_eq!(decoded.scrollback, b"scrollback data here");
        assert_eq!(decoded.screen, b"\x1b[H\x1b[2Jscreen");
        assert_eq!(decoded.input_mode, crate::input::mode::Mode::Passthrough);
        assert_eq!(decoded.screen_mode, crate::overlay::ScreenMode::Normal);
        assert_eq!(decoded.focused_id, None);
    }

    #[test]
    fn control_frame_attach_session_response_with_session_state() {
        let msg = AttachSessionResponseMsg {
            name: "test".to_string(),
            rows: 24,
            cols: 80,
            scrollback: Vec::new(),
            screen: Vec::new(),
            input_mode: crate::input::mode::Mode::Capture,
            screen_mode: crate::overlay::ScreenMode::Alt,
            focused_id: Some("overlay-123".to_string()),
        };
        let frame = Frame::control(FrameType::AttachSessionResponse, &msg).unwrap();
        let decoded: AttachSessionResponseMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.input_mode, crate::input::mode::Mode::Capture);
        assert_eq!(decoded.screen_mode, crate::overlay::ScreenMode::Alt);
        assert_eq!(decoded.focused_id, Some("overlay-123".to_string()));
    }

    #[test]
    fn control_frame_resize() {
        let msg = ResizeMsg {
            rows: 50,
            cols: 200,
        };
        let frame = Frame::control(FrameType::Resize, &msg).unwrap();
        let decoded: ResizeMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.rows, 50);
        assert_eq!(decoded.cols, 200);
    }

    #[test]
    fn control_frame_error() {
        let msg = ErrorMsg {
            code: "session_not_found".to_string(),
            message: "No session named 'foo'".to_string(),
        };
        let frame = Frame::control(FrameType::Error, &msg).unwrap();
        let decoded: ErrorMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.code, "session_not_found");
        assert_eq!(decoded.message, "No session named 'foo'");
    }

    #[test]
    fn scrollback_request_variants() {
        let json_none = serde_json::json!("none");
        let decoded: ScrollbackRequest = serde_json::from_value(json_none).unwrap();
        assert!(matches!(decoded, ScrollbackRequest::None));

        let json_all = serde_json::json!("all");
        let decoded: ScrollbackRequest = serde_json::from_value(json_all).unwrap();
        assert!(matches!(decoded, ScrollbackRequest::All));

        let json_lines = serde_json::json!({"lines": 50});
        let decoded: ScrollbackRequest = serde_json::from_value(json_lines).unwrap();
        assert!(matches!(decoded, ScrollbackRequest::Lines(50)));
    }

    #[test]
    fn control_frame_list_sessions() {
        let msg = ListSessionsMsg {};
        let frame = Frame::control(FrameType::ListSessions, &msg).unwrap();
        assert_eq!(frame.frame_type, FrameType::ListSessions);
        let decoded: ListSessionsMsg = frame.parse_json().unwrap();
        let _ = decoded; // empty struct, just verify deserialization
    }

    #[test]
    fn control_frame_list_sessions_response() {
        let msg = ListSessionsResponseMsg {
            sessions: vec![
                SessionInfoMsg {
                    name: "alpha".to_string(),
                    pid: Some(1234),
                    command: "/bin/bash".to_string(),
                    rows: 24,
                    cols: 80,
                    clients: 1,
                },
                SessionInfoMsg {
                    name: "beta".to_string(),
                    pid: None,
                    command: String::new(),
                    rows: 0,
                    cols: 0,
                    clients: 0,
                },
            ],
        };
        let frame = Frame::control(FrameType::ListSessionsResponse, &msg).unwrap();
        let decoded: ListSessionsResponseMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.sessions.len(), 2);
        assert_eq!(decoded.sessions[0].name, "alpha");
        assert_eq!(decoded.sessions[0].pid, Some(1234));
        assert_eq!(decoded.sessions[0].command, "/bin/bash");
        assert_eq!(decoded.sessions[0].rows, 24);
        assert_eq!(decoded.sessions[0].cols, 80);
        assert_eq!(decoded.sessions[0].clients, 1);
        assert_eq!(decoded.sessions[1].name, "beta");
        assert_eq!(decoded.sessions[1].pid, None);
    }

    #[test]
    fn control_frame_list_sessions_response_empty() {
        let msg = ListSessionsResponseMsg { sessions: vec![] };
        let frame = Frame::control(FrameType::ListSessionsResponse, &msg).unwrap();
        let decoded: ListSessionsResponseMsg = frame.parse_json().unwrap();
        assert!(decoded.sessions.is_empty());
    }

    #[test]
    fn control_frame_kill_session() {
        let msg = KillSessionMsg { name: "my-session".to_string() };
        let frame = Frame::control(FrameType::KillSession, &msg).unwrap();
        let decoded: KillSessionMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.name, "my-session");
    }

    #[test]
    fn control_frame_kill_session_response() {
        let msg = KillSessionResponseMsg { name: "killed-session".to_string() };
        let frame = Frame::control(FrameType::KillSessionResponse, &msg).unwrap();
        let decoded: KillSessionResponseMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.name, "killed-session");
    }

    #[test]
    fn control_frame_detach_session() {
        let msg = DetachSessionMsg { name: "my-session".to_string() };
        let frame = Frame::control(FrameType::DetachSession, &msg).unwrap();
        let decoded: DetachSessionMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.name, "my-session");
    }

    #[test]
    fn control_frame_detach_session_response() {
        let msg = DetachSessionResponseMsg { name: "detached-session".to_string() };
        let frame = Frame::control(FrameType::DetachSessionResponse, &msg).unwrap();
        let decoded: DetachSessionResponseMsg = frame.parse_json().unwrap();
        assert_eq!(decoded.name, "detached-session");
    }

    #[tokio::test]
    async fn multiple_frames_sequential() {
        let frames = vec![
            Frame::new(FrameType::StdinInput, Bytes::from("hello")),
            Frame::new(FrameType::PtyOutput, Bytes::from("world")),
            Frame::new(FrameType::Detach, Bytes::new()),
        ];

        let mut buf = Vec::new();
        for f in &frames {
            f.write_to(&mut buf).await.unwrap();
        }

        let mut cursor = io::Cursor::new(buf);
        let f1 = Frame::read_from(&mut cursor).await.unwrap();
        assert_eq!(f1.frame_type, FrameType::StdinInput);
        assert_eq!(f1.payload, Bytes::from("hello"));

        let f2 = Frame::read_from(&mut cursor).await.unwrap();
        assert_eq!(f2.frame_type, FrameType::PtyOutput);
        assert_eq!(f2.payload, Bytes::from("world"));

        let f3 = Frame::read_from(&mut cursor).await.unwrap();
        assert_eq!(f3.frame_type, FrameType::Detach);
        assert!(f3.payload.is_empty());
    }
}
