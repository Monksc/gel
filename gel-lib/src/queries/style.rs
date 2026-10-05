//! Colour parsing, used by `Emit` to validate what a program put in a
//! shape's `fill` / `stroke`.
//!
//! `Fill` and `Outline` used to live here; they are now `SetData` with a
//! `fill` or `stroke` key. Validation stayed because the alternative is
//! silent: SVG renders an unrecognised colour as black, so a typo would
//! quietly produce a wrong proof rather than an error.

/// Parses the CSS colour subset a layout program needs.
///
/// Accepts `none`, `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb(r,g,b)`,
/// `rgba(r,g,b,a)` and the handful of names below. Anything else is an error
/// rather than a silent no-paint - a typo'd colour that quietly does nothing
/// is far harder to spot on a proof than one that stops the run.
pub fn parse_color(text: &str) -> Result<Option<(u8, u8, u8, u8)>, String> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();

    if lower == "none" || lower == "transparent" {
        return Ok(None);
    }

    let named = match lower.as_str() {
        "black" => Some((0, 0, 0)),
        "white" => Some((255, 255, 255)),
        "red" => Some((255, 0, 0)),
        "green" => Some((0, 128, 0)),
        "lime" => Some((0, 255, 0)),
        "blue" => Some((0, 0, 255)),
        "yellow" => Some((255, 255, 0)),
        "cyan" | "aqua" => Some((0, 255, 255)),
        "magenta" | "fuchsia" => Some((255, 0, 255)),
        "gray" | "grey" => Some((128, 128, 128)),
        "orange" => Some((255, 165, 0)),
        _ => None,
    };
    if let Some((r, g, b)) = named {
        return Ok(Some((r, g, b, 255)));
    }

    if let Some(hex) = lower.strip_prefix('#') {
        let parse = |s: &str| u8::from_str_radix(s, 16).map_err(|_| format!("bad hex in {text:?}"));
        return match hex.len() {
            // #rgb - each digit doubled, per CSS.
            3 => {
                let d: Vec<u8> = hex
                    .chars()
                    .map(|c| parse(&format!("{c}{c}")))
                    .collect::<Result<_, _>>()?;
                Ok(Some((d[0], d[1], d[2], 255)))
            }
            6 => Ok(Some((
                parse(&hex[0..2])?,
                parse(&hex[2..4])?,
                parse(&hex[4..6])?,
                255,
            ))),
            8 => Ok(Some((
                parse(&hex[0..2])?,
                parse(&hex[2..4])?,
                parse(&hex[4..6])?,
                parse(&hex[6..8])?,
            ))),
            _ => Err(format!("{text:?} is not #rgb, #rrggbb or #rrggbbaa")),
        };
    }

    if let Some(rest) = lower.strip_prefix("rgba(").or_else(|| lower.strip_prefix("rgb(")) {
        let rest = rest.strip_suffix(')').ok_or_else(|| format!("{text:?} is missing ')'"))?;
        let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
        if parts.len() != 3 && parts.len() != 4 {
            return Err(format!("{text:?} needs 3 or 4 components"));
        }
        let channel = |s: &str| {
            s.parse::<f64>()
                .map_err(|_| format!("bad number {s:?} in {text:?}"))
                .map(|v| v.clamp(0.0, 255.0).round() as u8)
        };
        let alpha = match parts.get(3) {
            // CSS alpha is 0-1, but accept 0-255 too rather than turning a
            // perfectly clear intent into a nearly-invisible shape.
            Some(a) => {
                let v: f64 = a.parse().map_err(|_| format!("bad alpha {a:?} in {text:?}"))?;
                if v <= 1.0 { (v * 255.0).round() as u8 } else { v.clamp(0.0, 255.0).round() as u8 }
            }
            None => 255,
        };
        return Ok(Some((channel(parts[0])?, channel(parts[1])?, channel(parts[2])?, alpha)));
    }

    Err(format!(
        "{text:?} is not a colour gel understands (none, a name, #rrggbb, or rgb()/rgba())"
    ))
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn parses_the_colour_forms_a_program_will_use() {
        assert_eq!(parse_color("none").unwrap(), None);
        assert_eq!(parse_color("red").unwrap(), Some((255, 0, 0, 255)));
        assert_eq!(parse_color("#FF0000").unwrap(), Some((255, 0, 0, 255)));
        assert_eq!(parse_color("#f00").unwrap(), Some((255, 0, 0, 255)));
        assert_eq!(parse_color("rgb(0, 128, 255)").unwrap(), Some((0, 128, 255, 255)));
        assert_eq!(parse_color("rgba(0,0,0,0.5)").unwrap(), Some((0, 0, 0, 128)));
        assert_eq!(parse_color("#00ff0080").unwrap(), Some((0, 255, 0, 128)));
    }

    /// A misspelled colour must stop the run. Silently leaving the shape
    /// unpainted is nearly invisible on a proof.
    #[test]
    fn an_unknown_colour_is_an_error() {
        assert!(parse_color("chartruese").is_err());
        assert!(parse_color("#ff00").is_err());
    }
}
