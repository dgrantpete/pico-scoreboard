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

/// Team primaries darker than this in every channel are scaled up, so
/// near-black has somewhere to start from.
pub const TEAM_COLOR_MIN_CHANNEL: u32 = 128;

/// The perceived brightness (BT.601 luma, 0–255) every team color is lifted
/// to. The max-channel rule alone leaves navies and purples at luma 30–60 —
/// the Mets, the Rockies, the Kraken, the Oilers were barely visible as text.
/// 80 was chosen against every MLB, NBA, NFL, NHL, MLS, EPL and Liga MX
/// primary: it lifts every navy and purple to a readable blue, and leaves
/// saturated reds (luma 70–80, already vivid on an LED) where they are —
/// at 96 Liverpool, Arsenal and the Cardinals start turning pink.
pub const TEAM_COLOR_MIN_LUMA: u32 = 80;

/// Luma weights, in 256ths: 0.299 / 0.587 / 0.114.
const LUMA_WEIGHTS: (u32, u32, u32) = (77, 150, 29);

/// `luma = (weighted + 128) >> 8`, so a color reaches the floor exactly when
/// its weighted sum reaches this.
const LUMA_TARGET: u32 = 256 * TEAM_COLOR_MIN_LUMA - 128;

/// The weighted sum of pure white: the weights total 256.
const LUMA_WHITE: u32 = 256 * 255;

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

    /// BT.601 luma, rounded: how bright the color reads, 0–255.
    pub const fn luma(self) -> u32 {
        (weighted(self.0 >> 16 & 0xFF, self.0 >> 8 & 0xFF, self.0 & 0xFF) + 128) >> 8
    }

    /// Lift a team color until it is legible on the black panel. Two floors,
    /// in order, and a color already above both is returned untouched:
    ///
    /// 1. **Brightest channel ≥ [`TEAM_COLOR_MIN_CHANNEL`]**, by scaling every
    ///    channel by the same factor — hue preserved. Pure black has no hue
    ///    and becomes mid gray.
    /// 2. **Luma ≥ [`TEAM_COLOR_MIN_LUMA`]**, by scaling again (hue
    ///    preserved) and, only where a channel saturates first, blending
    ///    toward white by the remaining amount.
    ///
    /// Step 2 works on the weighted sum rather than on rounded luma and
    /// rounds every channel up, which makes the floor exact: the scaled sum is
    /// at least the target because each term is, and the blend closes exactly
    /// the remaining gap over the remaining headroom. No result lands one
    /// short.
    ///
    /// The frozen MicroPython firmware still applies only step 1
    /// (`state._team_color_to_rgb565`); the luma floor is Rust-only.
    pub const fn brightened(self) -> Self {
        let (mut red, mut green, mut blue) = (self.0 >> 16 & 0xFF, self.0 >> 8 & 0xFF, self.0 & 0xFF);

        let max = max3(red, green, blue);
        if max == 0 {
            let gray = TEAM_COLOR_MIN_CHANNEL;
            return Self::from_channels(gray, gray, gray);
        }
        if max < TEAM_COLOR_MIN_CHANNEL {
            red = red * TEAM_COLOR_MIN_CHANNEL / max;
            green = green * TEAM_COLOR_MIN_CHANNEL / max;
            blue = blue * TEAM_COLOR_MIN_CHANNEL / max;
        }

        // Never zero here: the brightest channel is at least 128.
        let sum = weighted(red, green, blue);
        if sum >= LUMA_TARGET {
            return Self::from_channels(red, green, blue);
        }
        red = min_255((red * LUMA_TARGET).div_ceil(sum));
        green = min_255((green * LUMA_TARGET).div_ceil(sum));
        blue = min_255((blue * LUMA_TARGET).div_ceil(sum));

        let sum = weighted(red, green, blue);
        if sum >= LUMA_TARGET {
            return Self::from_channels(red, green, blue);
        }
        let (gap, headroom) = (LUMA_TARGET - sum, LUMA_WHITE - sum);
        Self::from_channels(
            red + ((255 - red) * gap).div_ceil(headroom),
            green + ((255 - green) * gap).div_ceil(headroom),
            blue + ((255 - blue) * gap).div_ceil(headroom),
        )
    }

    const fn from_channels(red: u32, green: u32, blue: u32) -> Self {
        Self(red << 16 | green << 8 | blue)
    }
}

const fn weighted(red: u32, green: u32, blue: u32) -> u32 {
    LUMA_WEIGHTS.0 * red + LUMA_WEIGHTS.1 * green + LUMA_WEIGHTS.2 * blue
}

const fn max3(a: u32, b: u32, c: u32) -> u32 {
    if a >= b && a >= c {
        a
    } else if b >= c {
        b
    } else {
        c
    }
}

const fn min_255(value: u32) -> u32 {
    if value > 255 { 255 } else { value }
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
