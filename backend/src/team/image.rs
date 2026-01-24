use image::{DynamicImage, GenericImageView, ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;

use crate::error::AppError;

/// Parse hex RGB888 color string (without #) into RGB tuple
pub fn parse_hex_color(hex: &str) -> Result<(u8, u8, u8), AppError> {
    if hex.len() != 6 {
        return Err(AppError::InvalidColor(hex.to_string()));
    }

    let r =
        u8::from_str_radix(&hex[0..2], 16).map_err(|_| AppError::InvalidColor(hex.to_string()))?;
    let g =
        u8::from_str_radix(&hex[2..4], 16).map_err(|_| AppError::InvalidColor(hex.to_string()))?;
    let b =
        u8::from_str_radix(&hex[4..6], 16).map_err(|_| AppError::InvalidColor(hex.to_string()))?;

    Ok((r, g, b))
}

/// Blend transparent pixels with a background color.
/// Uses standard alpha compositing: out = src * alpha + bg * (1 - alpha)
pub fn blend_with_background(img: &DynamicImage, bg: (u8, u8, u8)) -> RgbaImage {
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();

    let mut output = RgbaImage::new(width, height);

    for (x, y, pixel) in rgba.enumerate_pixels() {
        let Rgba([r, g, b, a]) = *pixel;

        if a == 255 {
            // Fully opaque - keep as is
            output.put_pixel(x, y, Rgba([r, g, b, 255]));
        } else if a == 0 {
            // Fully transparent - use background
            output.put_pixel(x, y, Rgba([bg.0, bg.1, bg.2, 255]));
        } else {
            // Partial transparency - blend
            let alpha = a as f32 / 255.0;
            let inv_alpha = 1.0 - alpha;

            let out_r = (r as f32 * alpha + bg.0 as f32 * inv_alpha).round() as u8;
            let out_g = (g as f32 * alpha + bg.1 as f32 * inv_alpha).round() as u8;
            let out_b = (b as f32 * alpha + bg.2 as f32 * inv_alpha).round() as u8;

            output.put_pixel(x, y, Rgba([out_r, out_g, out_b, 255]));
        }
    }

    output
}

/// Encode image as PNG bytes
pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, AppError> {
    let mut buffer = Cursor::new(Vec::new());
    img.write_to(&mut buffer, ImageFormat::Png)
        .map_err(|e| AppError::ImageDecode(e.to_string()))?;
    Ok(buffer.into_inner())
}

/// Convert image to PPM P6 binary format (RGB888, no alpha)
pub fn encode_ppm_p6(img: &RgbaImage) -> Vec<u8> {
    let (width, height) = img.dimensions();

    // PPM P6 header: "P6\n{width} {height}\n255\n"
    let header = format!("P6\n{} {}\n255\n", width, height);

    // Calculate total size: header + 3 bytes per pixel
    let pixel_count = (width * height) as usize;
    let mut output = Vec::with_capacity(header.len() + pixel_count * 3);

    // Write header
    output.extend_from_slice(header.as_bytes());

    // Write RGB data (strip alpha channel)
    for pixel in img.pixels() {
        let Rgba([r, g, b, _]) = *pixel;
        output.push(r);
        output.push(g);
        output.push(b);
    }

    output
}

/// Decode PNG bytes into a DynamicImage
pub fn decode_png(bytes: &[u8]) -> Result<DynamicImage, AppError> {
    image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map_err(|e| AppError::ImageDecode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color_valid() {
        assert_eq!(parse_hex_color("FF0000").unwrap(), (255, 0, 0));
        assert_eq!(parse_hex_color("00FF00").unwrap(), (0, 255, 0));
        assert_eq!(parse_hex_color("0000FF").unwrap(), (0, 0, 255));
        assert_eq!(parse_hex_color("FFFFFF").unwrap(), (255, 255, 255));
        assert_eq!(parse_hex_color("000000").unwrap(), (0, 0, 0));
        assert_eq!(parse_hex_color("ff0000").unwrap(), (255, 0, 0)); // lowercase
    }

    #[test]
    fn test_parse_hex_color_invalid() {
        assert!(parse_hex_color("").is_err());
        assert!(parse_hex_color("FFF").is_err()); // too short
        assert!(parse_hex_color("FFFFFFF").is_err()); // too long
        assert!(parse_hex_color("GGGGGG").is_err()); // invalid chars
        assert!(parse_hex_color("#FF0000").is_err()); // has #
    }

    #[test]
    fn test_ppm_header_format() {
        let img = RgbaImage::new(10, 20);
        let ppm = encode_ppm_p6(&img);

        // Check header
        let header_end = ppm.iter().position(|&b| b == b'\n').unwrap() + 1;
        let header_end = header_end
            + ppm[header_end..]
                .iter()
                .position(|&b| b == b'\n')
                .unwrap()
            + 1;
        let header_end = header_end
            + ppm[header_end..]
                .iter()
                .position(|&b| b == b'\n')
                .unwrap()
            + 1;

        let header = std::str::from_utf8(&ppm[..header_end]).unwrap();
        assert_eq!(header, "P6\n10 20\n255\n");

        // Check data size: 10 * 20 * 3 = 600 bytes
        assert_eq!(ppm.len() - header_end, 600);
    }

    #[test]
    fn test_blend_fully_transparent() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, Rgba([100, 100, 100, 0])); // fully transparent

        let dynamic = DynamicImage::ImageRgba8(img);
        let result = blend_with_background(&dynamic, (255, 0, 0));

        // Should be pure background color
        assert_eq!(*result.get_pixel(0, 0), Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn test_blend_fully_opaque() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, Rgba([100, 150, 200, 255])); // fully opaque

        let dynamic = DynamicImage::ImageRgba8(img);
        let result = blend_with_background(&dynamic, (255, 0, 0));

        // Should be original color
        assert_eq!(*result.get_pixel(0, 0), Rgba([100, 150, 200, 255]));
    }

    #[test]
    fn test_blend_half_transparent() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 128])); // ~50% transparent black

        let dynamic = DynamicImage::ImageRgba8(img);
        let result = blend_with_background(&dynamic, (255, 255, 255)); // white bg

        // Should be roughly gray (127-128 range due to rounding)
        let pixel = result.get_pixel(0, 0);
        assert!(pixel[0] >= 126 && pixel[0] <= 128);
        assert!(pixel[1] >= 126 && pixel[1] <= 128);
        assert!(pixel[2] >= 126 && pixel[2] <= 128);
        assert_eq!(pixel[3], 255);
    }
}
