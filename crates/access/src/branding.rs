use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::io::Cursor;

/// Only bounded raster pixels survive; never persist remote URLs, SVG or metadata.
pub(crate) fn normalize_icon(value: Option<String>) -> Result<Option<String>> {
    let Some(value) = value else { return Ok(None) };
    if value.len() > 32768 {
        return Err(Error::InvalidInput);
    }
    let raw = STANDARD
        .decode(
            value
                .strip_prefix("data:image/png;base64,")
                .ok_or(Error::InvalidInput)?,
        )
        .map_err(|_| Error::InvalidInput)?;
    let mut decoder = png::Decoder::new(Cursor::new(raw));
    decoder.set_limits(png::Limits { bytes: 128 * 1024 });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| Error::InvalidInput)?;
    if reader.info().width != 64
        || reader.info().height != 64
        || reader.info().animation_control.is_some()
    {
        return Err(Error::InvalidInput);
    }
    let size = reader.output_buffer_size().ok_or(Error::InvalidInput)?;
    if size > 64 * 64 * 4 {
        return Err(Error::InvalidInput);
    }
    let mut pixels = vec![0; size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|_| Error::InvalidInput)?;
    reader.finish().map_err(|_| Error::InvalidInput)?;
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 64, 64);
        encoder.set_color(info.color_type);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|_| Error::InvalidInput)?;
        writer
            .write_image_data(&pixels[..info.buffer_size()])
            .map_err(|_| Error::InvalidInput)?;
    }
    Ok(Some(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(out)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn icon(width: u32) -> String {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, 64);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .add_text_chunk("Comment".into(), "private metadata".into())
                .unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![200; width as usize * 64 * 4])
                .unwrap();
        }
        format!("data:image/png;base64,{}", STANDARD.encode(bytes))
    }
    #[test]
    fn canonical_raster_and_removal() {
        let result = normalize_icon(Some(icon(64))).unwrap().unwrap();
        let bytes = STANDARD
            .decode(result.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap();
        assert!(
            !bytes
                .windows(b"private metadata".len())
                .any(|w| w == b"private metadata")
        );
        assert_eq!(normalize_icon(Some(result.clone())).unwrap(), Some(result));
        assert_eq!(normalize_icon(None).unwrap(), None);
    }
    #[test]
    fn rejects_external_active_corrupt_and_oversized_images() {
        for value in [
            "https://example.test/avatar.png".to_string(),
            "data:image/svg+xml,<svg/>".into(),
            "data:image/png;base64,bm90LXBuZw==".into(),
            "x".repeat(32769),
            icon(65),
        ] {
            assert!(normalize_icon(Some(value)).is_err());
        }
        let good = icon(64);
        assert!(normalize_icon(Some(good[..good.len() - 12].into())).is_err());
    }
}
