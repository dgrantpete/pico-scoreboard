use serde::Deserialize;
use utoipa::IntoParams;

/// Query parameters for the logo endpoint
#[derive(Debug, Deserialize, IntoParams)]
pub struct LogoQuery {
    /// Width in pixels (default: 128)
    #[serde(default = "default_size")]
    pub width: u32,

    /// Height in pixels (default: 128)
    #[serde(default = "default_size")]
    pub height: u32,

    /// Background color as hex RGB888 without # (e.g., "FFFFFF").
    /// If provided, transparent pixels are blended with this color.
    pub background_color: Option<String>,
}

fn default_size() -> u32 {
    128
}

/// Supported output formats based on Accept header
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Png,
    Ppm,
}

impl OutputFormat {
    pub fn content_type(&self) -> &'static str {
        match self {
            OutputFormat::Png => "image/png",
            OutputFormat::Ppm => "image/x-portable-pixmap",
        }
    }
}
