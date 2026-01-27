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

/// Convert image to raw RGB888 bytes (3 bytes per pixel, no header)
///
/// Pixels are stored in row-major order: R0,G0,B0,R1,G1,B1,...
/// Alpha channel is discarded.
pub fn encode_rgb888_raw(img: &RgbaImage) -> Vec<u8> {
    let (width, height) = img.dimensions();
    let pixel_count = (width * height) as usize;

    let mut output = Vec::with_capacity(pixel_count * 3);

    for pixel in img.pixels() {
        let Rgba([r, g, b, _]) = *pixel;
        output.push(r);
        output.push(g);
        output.push(b);
    }

    output
}

/// Convert image to raw RGB565 bytes (2 bytes per pixel, little-endian)
///
/// RGB565 format: RRRRR GGGGGG BBBBB (5 bits red, 6 bits green, 5 bits blue)
/// - Red:   bits 15-11 (top 5 bits of 8-bit red)
/// - Green: bits 10-5  (top 6 bits of 8-bit green)
/// - Blue:  bits 4-0   (top 5 bits of 8-bit blue)
///
/// Byte order: Little-endian (low byte first) for embedded compatibility.
/// Pixels are stored in row-major order.
pub fn encode_rgb565_raw(img: &RgbaImage) -> Vec<u8> {
    let (width, height) = img.dimensions();
    let pixel_count = (width * height) as usize;

    let mut output = Vec::with_capacity(pixel_count * 2);

    for pixel in img.pixels() {
        let Rgba([r, g, b, _]) = *pixel;

        // Convert RGB888 to RGB565
        let r5 = (r >> 3) as u16;
        let g6 = (g >> 2) as u16;
        let b5 = (b >> 3) as u16;

        let rgb565: u16 = (r5 << 11) | (g6 << 5) | b5;

        // Little-endian: low byte first
        output.push((rgb565 & 0xFF) as u8);
        output.push((rgb565 >> 8) as u8);
    }

    output
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

    #[test]
    fn test_rgb888_raw_size() {
        let img = RgbaImage::new(10, 20);
        let raw = encode_rgb888_raw(&img);
        // 10 * 20 * 3 = 600 bytes
        assert_eq!(raw.len(), 600);
    }

    #[test]
    fn test_rgb888_raw_pixel_values() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, Rgba([0xAB, 0xCD, 0xEF, 0xFF]));
        let raw = encode_rgb888_raw(&img);
        assert_eq!(raw, vec![0xAB, 0xCD, 0xEF]);
    }

    #[test]
    fn test_rgb888_raw_strips_alpha() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, Rgba([0x12, 0x34, 0x56, 0x78])); // alpha ignored
        let raw = encode_rgb888_raw(&img);
        assert_eq!(raw, vec![0x12, 0x34, 0x56]);
    }

    #[test]
    fn test_rgb565_raw_size() {
        let img = RgbaImage::new(10, 20);
        let raw = encode_rgb565_raw(&img);
        // 10 * 20 * 2 = 400 bytes
        assert_eq!(raw.len(), 400);
    }

    #[test]
    fn test_rgb565_pure_red() {
        let mut img = RgbaImage::new(1, 1);
        // Pure red: R=255 (0xFF), G=0, B=0
        // RGB565: 11111 000000 00000 = 0xF800
        // Little-endian: 0x00, 0xF8
        img.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        let raw = encode_rgb565_raw(&img);
        assert_eq!(raw, vec![0x00, 0xF8]);
    }

    #[test]
    fn test_rgb565_pure_green() {
        let mut img = RgbaImage::new(1, 1);
        // Pure green: R=0, G=255, B=0
        // RGB565: 00000 111111 00000 = 0x07E0
        // Little-endian: 0xE0, 0x07
        img.put_pixel(0, 0, Rgba([0, 255, 0, 255]));
        let raw = encode_rgb565_raw(&img);
        assert_eq!(raw, vec![0xE0, 0x07]);
    }

    #[test]
    fn test_rgb565_pure_blue() {
        let mut img = RgbaImage::new(1, 1);
        // Pure blue: R=0, G=0, B=255
        // RGB565: 00000 000000 11111 = 0x001F
        // Little-endian: 0x1F, 0x00
        img.put_pixel(0, 0, Rgba([0, 0, 255, 255]));
        let raw = encode_rgb565_raw(&img);
        assert_eq!(raw, vec![0x1F, 0x00]);
    }

    #[test]
    fn test_rgb565_white() {
        let mut img = RgbaImage::new(1, 1);
        // White: R=255, G=255, B=255
        // RGB565: 11111 111111 11111 = 0xFFFF
        // Little-endian: 0xFF, 0xFF
        img.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        let raw = encode_rgb565_raw(&img);
        assert_eq!(raw, vec![0xFF, 0xFF]);
    }

    #[test]
    fn test_rgb565_black() {
        let mut img = RgbaImage::new(1, 1);
        // Black: R=0, G=0, B=0
        // RGB565: 00000 000000 00000 = 0x0000
        img.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        let raw = encode_rgb565_raw(&img);
        assert_eq!(raw, vec![0x00, 0x00]);
    }
}
