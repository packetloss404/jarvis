use super::ChatStreamCaptureRequest;

pub(crate) fn ensure_workspace_capture_available() -> Result<(), &'static str> {
    Ok(())
}

pub(crate) fn capture_workspace_frame(request: ChatStreamCaptureRequest) -> Result<String, String> {
    use core_graphics::display::{
        kCGNullWindowID, kCGWindowImageDefault, kCGWindowListOptionOnScreenOnly, CGDisplay,
    };
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use image::codecs::jpeg::JpegEncoder;
    use image::imageops::FilterType;
    use image::{DynamicImage, ImageBuffer, Rgba};

    let bounds = CGRect::new(
        &CGPoint::new(request.x as f64, request.y as f64),
        &CGSize::new(request.width as f64, request.height as f64),
    );
    let image = CGDisplay::screenshot(
        bounds,
        kCGWindowListOptionOnScreenOnly,
        kCGNullWindowID,
        kCGWindowImageDefault,
    )
    .ok_or_else(|| "screen capture failed".to_string())?;

    let width = image.width() as u32;
    let height = image.height() as u32;
    let bytes_per_row = image.bytes_per_row();
    let raw = image.data().bytes().to_vec();

    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        let row = &raw[y * bytes_per_row..y * bytes_per_row + (width as usize * 4)];
        for px in row.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }

    let image = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, rgba)
        .ok_or_else(|| "failed to decode screenshot".to_string())?;

    let dynamic = DynamicImage::ImageRgba8(image);
    let resized = dynamic.resize(640, 360, FilterType::Triangle).to_rgb8();

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
