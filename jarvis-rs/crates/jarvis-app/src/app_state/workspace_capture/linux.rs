use super::ChatStreamCaptureRequest;

pub(crate) fn ensure_workspace_capture_available() -> Result<(), &'static str> {
    Err("workspace streaming on Linux is not implemented yet")
}

pub(crate) fn capture_workspace_frame(
    _request: ChatStreamCaptureRequest,
) -> Result<String, String> {
    Err("workspace streaming on Linux is not implemented yet".into())
}
