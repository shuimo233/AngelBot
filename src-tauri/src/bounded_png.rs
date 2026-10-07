//! Shared bounded static-PNG validation. No model or desktop policy belongs here.
use std::io::Cursor;

pub(crate) fn validate(
    bytes: &[u8],
    max_bytes: usize,
    max_edge: u32,
    max_pixels: u64,
) -> Result<(u32, u32), ()> {
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(());
    }
    let decoded_budget = usize::try_from(max_pixels.checked_mul(4).ok_or(())?).map_err(|_| ())?;
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: decoded_budget,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| ())?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    if width == 0
        || height == 0
        || width > max_edge
        || height > max_edge
        || u64::from(width) * u64::from(height) > max_pixels
        || info.animation_control.is_some()
        || info.frame_control.is_some()
        || reader.output_buffer_size() > decoded_budget
    {
        return Err(());
    }
    let mut pixels = vec![0; reader.output_buffer_size()];
    reader.next_frame(&mut pixels).map_err(|_| ())?;
    reader.finish().map_err(|_| ())?;
    // Ancillary frame-control chunks can appear after the first IDAT.
    if reader.info().animation_control.is_some() || reader.info().frame_control.is_some() {
        return Err(());
    }
    Ok((width, height))
}
