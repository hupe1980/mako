//! Fuzz target for [`mako_engine::version::FormatVersion::parse`].
//!
//! Ensures that `FormatVersion::parse` never panics on arbitrary byte
//! sequences (including invalid UTF-8, truncated input, and malformed
//! format-version strings).
#![no_main]

use libfuzzer_sys::fuzz_target;
use mako_engine::version::FormatVersion;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Must never panic — only return Ok or Err.
        let _ = FormatVersion::parse(s);
    }
});
