//! Validated, transient tool images. No serialization or payload-bearing Debug.
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{fmt, sync::Arc};

pub const MAX_TOOL_IMAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_TOOL_IMAGE_TOTAL_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TOOL_IMAGES: usize = 4;
const MAX_PIXELS: u64 = 4 * 1024 * 1024;
const INVALID_IMAGE: &str = "工具图像格式无效或超出预算；未发送图像。";

#[derive(Clone)]
pub struct ToolImage {
    data: Arc<str>,
    bytes: usize,
    width: u32,
    height: u32,
}

impl ToolImage {
    /// Only bounded, fully decoded, non-animated PNG is accepted in this slice.
    pub fn from_base64(mime: &str, data: &str) -> Result<Self, &'static str> {
        if mime != "image/png" || data.len() > MAX_TOOL_IMAGE_BYTES.div_ceil(3) * 4 {
            return Err(INVALID_IMAGE);
        }
        let bytes = STANDARD.decode(data).map_err(|_| INVALID_IMAGE)?;
        Self::from_png(&bytes)
    }

    /// Native adapters can validate owned PNG bytes without a base64 round-trip.
    pub fn from_png(bytes: &[u8]) -> Result<Self, &'static str> {
        let (width, height) =
            crate::bounded_png::validate(bytes, MAX_TOOL_IMAGE_BYTES, 4096, MAX_PIXELS)
                .map_err(|_| INVALID_IMAGE)?;
        Ok(Self {
            data: STANDARD.encode(bytes).into(),
            bytes: bytes.len(),
            width,
            height,
        })
    }
    pub fn byte_len(&self) -> usize {
        self.bytes
    }
    pub(crate) fn data_url(&self) -> String {
        format!("data:image/png;base64,{}", self.data)
    }
}

impl fmt::Debug for ToolImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolImage")
            .field("mime", &"image/png")
            .field("bytes", &self.bytes)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn png(width: u32, height: u32) -> String {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![0; width as usize * height as usize * 4])
            .unwrap();
        writer.finish().unwrap();
        STANDARD.encode(bytes)
    }
    #[test]
    fn tool_image_rejects_frame_control_after_the_static_png_frame() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0; 16]).unwrap();
            let mut control = Vec::new();
            for value in [0_u32, 2, 2, 0, 0] {
                control.extend_from_slice(&value.to_be_bytes());
            }
            control.extend_from_slice(&1_u16.to_be_bytes());
            control.extend_from_slice(&1_u16.to_be_bytes());
            control.extend_from_slice(&[0, 0]);
            writer.write_chunk(png::chunk::fcTL, &control).unwrap();
            writer.finish().unwrap();
        }
        // The library accepts this ancillary tail; our static-only contract
        // must reject it after decoding rather than trusting read_info alone.
        let mut reader = png::Decoder::new(Cursor::new(&bytes)).read_info().unwrap();
        assert!(reader.info().frame_control.is_none());
        reader.next_frame(&mut [0; 16]).unwrap();
        reader.finish().unwrap();
        assert!(reader.info().frame_control.is_some());
        assert_eq!(
            ToolImage::from_base64("image/png", &STANDARD.encode(bytes)).unwrap_err(),
            INVALID_IMAGE
        );
    }

    #[test]
    fn tool_image_validates_payload_without_debug_leakage() {
        let encoded = png(2, 2);
        let image = ToolImage::from_base64("image/png", &encoded).unwrap();
        assert_eq!(image.data_url(), format!("data:image/png;base64,{encoded}"));
        let native = ToolImage::from_png(&STANDARD.decode(&encoded).unwrap()).unwrap();
        assert_eq!(native.data_url(), image.data_url());
        assert_eq!(native.byte_len(), image.byte_len());
        assert!(!format!("{image:?}").contains(&encoded));
        for (mime, data) in [
            ("image/jpeg", encoded.clone()),
            ("image/png", "bad".into()),
            ("image/png", STANDARD.encode(b"not a png")),
            ("image/png", png(4097, 1)),
            (
                "image/png",
                "A".repeat(MAX_TOOL_IMAGE_BYTES.div_ceil(3) * 4 + 4),
            ),
        ] {
            assert_eq!(
                ToolImage::from_base64(mime, &data).unwrap_err(),
                INVALID_IMAGE
            );
        }
        let mut corrupt = STANDARD.decode(encoded).unwrap();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(ToolImage::from_base64("image/png", &STANDARD.encode(corrupt)).is_err());
    }
}
