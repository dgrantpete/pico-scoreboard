//! Team and UI colors, and the brightening every team color goes through
//! before it reaches the panel.

/// Packed `0x00RRGGBB`, the shape the wire carries and the shape the renderer
/// needs for its derived shades (base-marker highlight/edge, endzone tints).
///
/// The MicroPython state kept two fields per team — the raw packed primary for
/// the shade math and a pre-converted RGB565 for text — and re-applied the
/// brightening in `display._base_marker_colors` to recover the channels.
/// One brightened `Rgb888` serves both, and RGB565 packing moves to the
/// renderer where the pixel format lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb888(pub u32);

/// Team primaries darker than this in every channel are scaled up: navy and
/// near-black stay legible on a black panel.
pub const TEAM_COLOR_MIN_CHANNEL: u32 = 128;

impl Rgb888 {
    pub const WHITE: Self = Self(0x00FF_FFFF);

    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self(((red as u32) << 16) | ((green as u32) << 8) | blue as u32)
    }

    pub const fn red(self) -> u8 {
        (self.0 >> 16) as u8
    }

    pub const fn green(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub const fn blue(self) -> u8 {
        self.0 as u8
    }

    /// Scale the color up until its brightest channel reaches
    /// [`TEAM_COLOR_MIN_CHANNEL`], preserving hue. Pure black has no hue to
    /// preserve and becomes mid gray.
    ///
    /// Integer math throughout. The MicroPython original computed a float
    /// scale in `state._team_color_to_rgb565` and the integer equivalent in
    /// `display._base_marker_colors`; the two disagree by one on channels
    /// where `channel * 128 / max` is exactly integral, because the float form
    /// truncates `127.999…`. This is the integer form.
    pub const fn brightened(self) -> Self {
        let (red, green, blue) = (self.0 >> 16 & 0xFF, self.0 >> 8 & 0xFF, self.0 & 0xFF);
        let max = if red >= green && red >= blue {
            red
        } else if green >= blue {
            green
        } else {
            blue
        };
        if max >= TEAM_COLOR_MIN_CHANNEL {
            return self;
        }
        if max == 0 {
            let gray = TEAM_COLOR_MIN_CHANNEL as u8;
            return Self::new(gray, gray, gray);
        }
        Self::new(
            (red * TEAM_COLOR_MIN_CHANNEL / max) as u8,
            (green * TEAM_COLOR_MIN_CHANNEL / max) as u8,
            (blue * TEAM_COLOR_MIN_CHANNEL / max) as u8,
        )
    }
}

impl From<scoreboard_wire::TeamColors> for Rgb888 {
    /// A team's primary, brightened — the only form any view stores.
    fn from(colors: scoreboard_wire::TeamColors) -> Self {
        Self(colors.primary & 0x00FF_FFFF).brightened()
    }
}

/// The configured UI palette, pushed from `config.colors`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiColors {
    pub primary: Rgb888,
    pub secondary: Rgb888,
    pub accent: Rgb888,
    pub clock_normal: Rgb888,
    pub clock_warning: Rgb888,
}

impl UiColors {
    pub const fn new() -> Self {
        Self {
            primary: Rgb888::WHITE,
            secondary: Rgb888::WHITE,
            accent: Rgb888::WHITE,
            clock_normal: Rgb888::WHITE,
            clock_warning: Rgb888::WHITE,
        }
    }
}

impl Default for UiColors {
    fn default() -> Self {
        Self::new()
    }
}
