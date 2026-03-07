//! Cross-platform workspace capture abstraction for live streaming.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub(crate) use platform::{capture_workspace_frame, ensure_workspace_capture_available};

use super::core::ChatStreamCaptureRequest;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod unsupported {
    use super::ChatStreamCaptureRequest;

    pub(crate) fn ensure_workspace_capture_available() -> Result<(), &'static str> {
        Err("workspace streaming is not supported on this platform yet")
    }

    pub(crate) fn capture_workspace_frame(
        _request: ChatStreamCaptureRequest,
    ) -> Result<String, String> {
        Err("workspace streaming is not supported on this platform yet".into())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
use unsupported as platform;
