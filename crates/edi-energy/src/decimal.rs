//! Numbers on the wire, read under the interchange's decimal mark.
//!
//! Allgemeine Festlegungen V6.1d Kap. 2.18 (S. 22) binds the decimal mark of
//! every EDIFACT number to the one the interchange's `UNA` defines. ISO 9735
//! lets a receiver accept either mark per value; a BDEW receiver reads the
//! declared one and refuses the other, because a sender who writes `1,5` under
//! `UNA:+.? '` has written something its own interchange says is no number.

use std::borrow::Cow;

/// The decimal mark the `UNA` at the head of `input` declares: `,` when its
/// position 3 is a comma, `.` otherwise — and `.` when there is no `UNA`.
///
/// A byte order mark and leading whitespace are skipped, as the tokenizer
/// skips them.
///
/// ```
/// use edi_energy::una_decimal_mark;
///
/// assert_eq!(una_decimal_mark(b"UNA:+,? 'UNB+UNOC:3'"), ',');
/// assert_eq!(una_decimal_mark(b"UNA:+.? 'UNB+UNOC:3'"), '.');
/// assert_eq!(una_decimal_mark(b"UNB+UNOC:3'"), '.');
/// ```
#[must_use]
pub fn una_decimal_mark(input: &[u8]) -> char {
    let input = input.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(input);
    let start = input
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(input.len());
    match &input[start..] {
        [b'U', b'N', b'A', _, _, b',', ..] => ',',
        _ => '.',
    }
}

/// `value` with `.` as its decimal mark, read under the interchange's `mark`.
///
/// `None` when the value carries the other mark: under `.` a `,` is refused,
/// under `,` a `.` is. The caller parses the result.
///
/// ```
/// use edi_energy::wire_decimal;
///
/// assert_eq!(wire_decimal("1.5", '.').as_deref(), Some("1.5"));
/// assert_eq!(wire_decimal("1,5", ',').as_deref(), Some("1.5"));
/// assert_eq!(wire_decimal("1,5", '.'), None);
/// assert_eq!(wire_decimal("1.5", ','), None);
/// ```
#[must_use]
pub fn wire_decimal(value: &str, mark: char) -> Option<Cow<'_, str>> {
    if mark == ',' {
        (!value.contains('.')).then(|| Cow::Owned(value.replace(',', ".")))
    } else {
        (!value.contains(',')).then_some(Cow::Borrowed(value))
    }
}
