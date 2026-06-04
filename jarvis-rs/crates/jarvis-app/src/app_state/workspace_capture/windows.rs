use super::ChatStreamCaptureRequest;

pub(crate) fn ensure_workspace_capture_available() -> Result<(), &'static str> {
    // xcap selects the Windows Graphics Capture / DXGI backend lazily; the first
    // capture attempt surfaces any real failure, so advertise availability here.
    Ok(())
}

pub(crate) fn capture_workspace_frame(request: ChatStreamCaptureRequest) -> Result<String, String> {
    use image::codecs::jpeg::JpegEncoder;
    use image::imageops::FilterType;
    use image::{DynamicImage, ImageBuffer, Rgba};
    use xcap::Monitor;

    // Window rect in physical screen coordinates (winit outer_position/outer_size).
    let req_x = request.x;
    let req_y = request.y;
    let req_w = request.width;
    let req_h = request.height;

    // Pick the monitor that contains the window's top-left corner; fall back to the first.
    let monitors = Monitor::all().map_err(|e| format!("enumerate monitors failed: {e}"))?;
    if monitors.is_empty() {
        return Err("no monitors available".into());
    }
    let monitor = monitors
        .iter()
        .find(|m| {
            let mx = m.x().unwrap_or(0);
            let my = m.y().unwrap_or(0);
            let mw = m.width().unwrap_or(0) as i32;
            let mh = m.height().unwrap_or(0) as i32;
            req_x >= mx && req_x < mx + mw && req_y >= my && req_y < my + mh
        })
        .unwrap_or(&monitors[0]);

    let mon_x = monitor.x().map_err(|e| format!("monitor origin failed: {e}"))?;
    let mon_y = monitor.y().map_err(|e| format!("monitor origin failed: {e}"))?;

    let shot = monitor
        .capture_image()
        .map_err(|e| format!("screen capture failed: {e}"))?;
    let shot_w = shot.width();
    let shot_h = shot.height();
    // Decouple from xcap's internal `image` version: take the raw RGBA8 bytes and
    // rebuild with this crate's `image` types.
    let raw: Vec<u8> = shot.into_raw();

    let full = ImageBuffer::<Rgba<u8>, _>::from_raw(shot_w, shot_h, raw)
        .ok_or_else(|| "failed to decode screenshot".to_string())?;

    // Crop the window region in monitor-local coordinates, clamped to the captured frame.
    let local_x = (req_x - mon_x).max(0) as u32;
    let local_y = (req_y - mon_y).max(0) as u32;
    let crop_w = req_w.min(shot_w.saturating_sub(local_x)).max(1);
    let crop_h = req_h.min(shot_h.saturating_sub(local_y)).max(1);

    let cropped = DynamicImage::ImageRgba8(full).crop_imm(local_x, local_y, crop_w, crop_h);

    let resized = cropped.resize(640, 360, FilterType::Triangle).to_rgb8();

    let mut jpg = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut jpg, 35);
    encoder
        .encode_image(&DynamicImage::ImageRgb8(resized))
        .map_err(|e| format!("jpeg encode failed: {e}"))?;

    use base64::Engine as _;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpg)
    ))
}
