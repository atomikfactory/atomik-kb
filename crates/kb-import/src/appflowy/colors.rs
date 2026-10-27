//! AppFlowy's colours to atomik-kb's nine named ones: `0xAARRGGBB` strings (text and background
//! colours of runs, callouts) by hue, and the names of select options.

use kb_types::OptionColor;

/// `(r, g, b, a)` of `0xAARRGGBB`, `#AARRGGBB`, `0xRRGGBB` or `#RRGGBB`.
fn parse_argb(text: &str) -> Option<(u8, u8, u8, u8)> {
    let text = text.trim();
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .or_else(|| text.strip_prefix('#'))
        .unwrap_or(text);
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        6 => Some((
            ((value >> 16) & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            (value & 0xff) as u8,
            0xff,
        )),
        8 => Some((
            ((value >> 16) & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            (value & 0xff) as u8,
            ((value >> 24) & 0xff) as u8,
        )),
        _ => None,
    }
}

/// The palette colour nearest to an `0xAARRGGBB` colour. `None` for transparent colours and for
/// the colours that mean "default" (black and white text, white backgrounds). Greys are `gray`.
pub(super) fn palette(argb: &str, background: bool) -> Option<&'static str> {
    let (r, g, b, a) = parse_argb(argb)?;
    if a < 16 {
        return None;
    }
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let chroma = f64::from(max - min);
    let value = f64::from(max) / 255.0;
    if chroma < 24.0 {
        return if background {
            (value < 0.97).then_some("gray")
        } else {
            (0.15..=0.92).contains(&value).then_some("gray")
        };
    }
    let (rf, gf, bf) = (f64::from(r), f64::from(g), f64::from(b));
    let mut hue = if max == r {
        60.0 * (((gf - bf) / chroma).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((bf - rf) / chroma + 2.0)
    } else {
        60.0 * ((rf - gf) / chroma + 4.0)
    };
    if hue < 0.0 {
        hue += 360.0;
    }
    let lightness = (f64::from(max) + f64::from(min)) / 510.0;
    Some(match hue {
        h if !(12.0..348.0).contains(&h) => "red",
        h if h < 45.0 && lightness < 0.35 => "brown",
        h if h < 40.0 => "orange",
        h if h < 70.0 => "yellow",
        h if h < 165.0 => "green",
        h if h < 255.0 => "blue",
        h if h < 300.0 => "purple",
        _ => "pink",
    })
}

/// The colour of a select option by AppFlowy's name for it.
pub(super) fn option_color(name: &str) -> Option<OptionColor> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "purple" => OptionColor::Purple,
        "pink" | "lightpink" => OptionColor::Pink,
        "orange" => OptionColor::Orange,
        "yellow" => OptionColor::Yellow,
        "lime" | "green" => OptionColor::Green,
        "aqua" | "blue" => OptionColor::Blue,
        "red" => OptionColor::Red,
        "gray" | "grey" => OptionColor::Gray,
        "brown" => OptionColor::Brown,
        _ => return None,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn colours_by_hue() {
        assert_eq!(palette("0xFFFF0000", false), Some("red"));
        assert_eq!(palette("0xFFFFA500", false), Some("orange"));
        assert_eq!(palette("0xFFFFFF00", true), Some("yellow"));
        assert_eq!(palette("0xFF00C800", false), Some("green"));
        assert_eq!(palette("0xFF0000FF", false), Some("blue"));
        assert_eq!(palette("0xFF00FFFF", false), Some("blue"));
        assert_eq!(palette("0xFF8000FF", false), Some("purple"));
        assert_eq!(palette("0xFFFF69B4", false), Some("pink"));
        assert_eq!(palette("0xFF8B4513", false), Some("brown"));
        assert_eq!(palette("#FFE6F0FF", true), Some("blue"));
        assert_eq!(palette("FF3333", false), Some("red"));
    }

    #[test]
    fn defaults_and_greys() {
        assert_eq!(palette("0xFF000000", false), None);
        assert_eq!(palette("0xFFFFFFFF", false), None);
        assert_eq!(palette("0xFFFFFFFF", true), None);
        assert_eq!(palette("0xFF808080", false), Some("gray"));
        assert_eq!(palette("0xFFE0E0E0", true), Some("gray"));
        // Transparent, malformed.
        assert_eq!(palette("0x00FF0000", false), None);
        assert_eq!(palette("red", false), None);
        assert_eq!(palette("0xFF12", false), None);
        assert_eq!(palette("", false), None);
    }

    #[test]
    fn option_names() {
        for (name, color) in [
            ("Purple", OptionColor::Purple),
            ("Pink", OptionColor::Pink),
            ("LightPink", OptionColor::Pink),
            ("Orange", OptionColor::Orange),
            ("Yellow", OptionColor::Yellow),
            ("Lime", OptionColor::Green),
            ("Green", OptionColor::Green),
            ("Aqua", OptionColor::Blue),
            ("Blue", OptionColor::Blue),
        ] {
            assert_eq!(option_color(name), Some(color), "{name}");
        }
        assert_eq!(option_color("Chartreuse"), None);
    }
}
