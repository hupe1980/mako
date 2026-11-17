//! A QR code as SVG, for a template to place with `image(bytes(..))`.
//!
//! The invoice letter prints the public Nachrechnen link (billingd's
//! `nachrechnen::public_link`) as text and as a QR code, so a customer reads the
//! invoice on paper and recomputes it on a phone. Error correction level M
//! (15 %) survives a fold and a smudge; the quiet zone is the four modules
//! ISO/IEC 18004 asks for.

use qrcodegen::{QrCode, QrCodeEcc};

/// Modules of light border around the symbol (ISO/IEC 18004 Kap. 6.3.8).
const QUIET_ZONE: i32 = 4;

/// The SVG of a QR code encoding `text`: one path of unit squares on a white
/// ground, scaled by the template.
///
/// # Errors
///
/// `text` is longer than a version-40 symbol at level M holds.
pub fn svg(text: &str) -> Result<String, String> {
    let code = QrCode::encode_text(text, QrCodeEcc::Medium)
        .map_err(|_| format!("{} characters do not fit a QR code", text.chars().count()))?;
    let size = code.size();
    let dim = size + 2 * QUIET_ZONE;
    let mut path = String::new();
    for y in 0..size {
        for x in 0..size {
            if code.get_module(x, y) {
                use std::fmt::Write as _;
                let _ = write!(path, "M{},{}h1v1h-1z", x + QUIET_ZONE, y + QUIET_ZONE);
            }
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dim} {dim}\" \
         shape-rendering=\"crispEdges\"><rect width=\"{dim}\" height=\"{dim}\" \
         fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>"
    ))
}

#[cfg(test)]
mod tests {
    /// The symbol is square, carries its quiet zone, and the same text draws
    /// the same code — a re-print of a letter is byte-identical.
    #[test]
    fn a_link_draws_a_stable_square_symbol() {
        let link = "https://kundenportal.example.de/nachrechnen/AAAAAAAAAAAAAAAAAAAAAA";
        let a = super::svg(link).expect("fits");
        assert_eq!(a, super::svg(link).expect("fits"));
        assert!(a.starts_with("<svg"));
        // 67 bytes at level M is version 5: 37 modules plus 2 × 4 of quiet zone.
        assert!(a.contains("viewBox=\"0 0 45 45\""));
        assert!(super::svg(&"x".repeat(5000)).is_err());
    }
}
