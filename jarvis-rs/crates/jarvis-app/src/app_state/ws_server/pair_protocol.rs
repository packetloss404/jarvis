//! Inner payload protocol for pair-programming Room sessions.
//!
//! `PairFrame` is the application-level message carried *inside* the opaque
//! [`RelayEnvelope`](super::relay_protocol::RelayEnvelope) — the relay never
//! sees these contents. Host is the source of truth: only the host emits
//! `pty_output`/`resize`/`driver_changed`/`snapshot`/`session_meta`;
//! navigators emit only `term_input`/`cursor`/`request_control`.
//!
//! Wire format: `#[serde(tag = "type", rename_all = "snake_case")]`. Raw byte
//! payloads (terminal data) are base64-encoded strings on the wire.

use serde::{Deserialize, Serialize};

/// Maximum bytes accepted for a single `term_input` keystroke frame. A real
/// keystroke (even a bracketed paste chunk) is tiny; this caps a hostile or
/// buggy navigator from fanning a multi-KB paste-bomb into the host PTY.
pub const MAX_TERM_INPUT_BYTES: usize = 4096;

/// Application-level pair-session message carried inside a `RelayEnvelope`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PairFrame {
    /// Host → room: a chunk of the shared terminal's PTY output stream.
    PtyOutput {
        #[serde(with = "b64")]
        data: Vec<u8>,
    },

    /// Driver → room → host: keystrokes to write into the host PTY.
    TermInput {
        from: String,
        #[serde(with = "b64")]
        data: Vec<u8>,
    },

    /// Any → room: a remote (ghost) cursor position update.
    Cursor { from: String, row: u16, col: u16 },

    /// Host → room: the shared terminal was resized.
    Resize { cols: u16, rows: u16 },

    /// Host → room: the driver seat changed hands.
    DriverChanged {
        new_driver: String,
        old_driver: String,
    },

    /// Navigator → host: ask to take the driver seat.
    RequestControl { from: String },

    /// Navigator → host: announce presence + display name on room join.
    ///
    /// The relay's `member_joined` carries only the opaque hex member id, so a
    /// navigator emits this once it is room-ready to hand the host its
    /// `display_name`. The host calls `join_session` with it (so driver gating
    /// recognises the navigator) and re-broadcasts `SessionMeta` with the
    /// updated roster.
    Join { from: String, name: String },

    /// Host → joining member: a mid-session replay snapshot of terminal state.
    Snapshot {
        #[serde(with = "b64")]
        data: Vec<u8>,
        cols: u16,
        rows: u16,
        driver: String,
    },

    /// Host → room on create AND on every join: session metadata for joiners.
    ///
    /// Re-broadcast on each `member_joined`/`Join` so late joiners (the relay
    /// does all-but-sender fan-out with no replay) still learn the host name,
    /// dimensions, takeover policy, and the current roster of real display
    /// names — without it a late joiner would render raw hex member ids.
    SessionMeta {
        host: String,
        host_name: String,
        cols: u16,
        rows: u16,
        allow_takeover: bool,
        /// Current participants (host + navigators) with sanitized display
        /// names and roles, so navigators render names instead of member ids.
        #[serde(default)]
        roster: Vec<RosterEntry>,
    },
}

/// A single participant entry carried in [`PairFrame::SessionMeta`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RosterEntry {
    pub member_id: String,
    pub name: String,
    /// `"host"` | `"driver"` | `"view"`.
    pub role: String,
}

/// serde helper: encode `Vec<u8>` as a base64 string on the wire.
mod b64 {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&B64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        B64.decode(s.as_bytes()).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_output_roundtrips_with_base64() {
        let frame = PairFrame::PtyOutput {
            data: vec![0x1b, b'[', b'2', b'J'],
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"pty_output\""));
        // base64 of the escape sequence, not raw bytes.
        assert!(json.contains("\"data\":\"G1sySg==\""));
        let back: PairFrame = serde_json::from_str(&json).unwrap();
        match back {
            PairFrame::PtyOutput { data } => assert_eq!(data, vec![0x1b, b'[', b'2', b'J']),
            _ => panic!("expected pty_output"),
        }
    }

    #[test]
    fn session_meta_tag_is_snake_case() {
        let frame = PairFrame::SessionMeta {
            host: "u1".into(),
            host_name: "Host".into(),
            cols: 80,
            rows: 24,
            allow_takeover: true,
            roster: vec![RosterEntry {
                member_id: "u1".into(),
                name: "Host".into(),
                role: "host".into(),
            }],
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"session_meta\""));
        assert!(json.contains("\"roster\""));
    }
}
