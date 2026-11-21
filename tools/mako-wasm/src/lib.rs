//! The validator's module: `edi-energy` and `mako-fristen` behind one JSON
//! call, compiled to WebAssembly for the docs site's `/validator/` and `/rechner/` pages.
//!
//! | Export | Takes | Returns |
//! |---|---|---|
//! | `mw_alloc(len)` | a number of bytes | a buffer's pointer |
//! | `mw_free(ptr, len)` | a buffer from `mw_alloc` or `mw_call` | nothing |
//! | `mw_call(ptr, len)` | a UTF-8 JSON request | the JSON answer, packed |
//!
//! A packed answer holds its pointer in the high 32 bits and its length in the
//! low 32; the host reads it and frees both buffers. No `wasm-bindgen`: the
//! parity test calls [`call`] natively and the browser gets exactly those
//! bytes.
//!
//! Requests (`op`): `samples` — the bundled sample messages; `sample` — one
//! sample's text; `check` — every message of an interchange, named and
//! checked; `codes` — an Antwortcode in its Entscheidungsbaum; `frist` — a
//! process's Antwortfrist; `id` — an identifier's check digit; `pog`,
//! `eeg`, `preise`, `modul14a` — the calculators. Every answer is `{"ok": …}` or `{"refused": {code, message}}`.
//! The module explains what the crates compute and decides nothing they do
//! not.

mod check;
mod rechner;
mod samples;
mod werkzeuge;

use serde::Deserialize;
use serde_json::{Value, json};

/// The most a request may carry: a market message, not an archive.
pub const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
enum Request {
    Samples,
    Sample {
        name: String,
    },
    Check {
        text: String,
        /// When the message arrived, RFC 3339; the deadlines count from it.
        #[serde(default)]
        empfangen: Option<String>,
    },
    Codes {
        #[serde(default)]
        ebd: Option<String>,
        #[serde(default)]
        code: Option<String>,
    },
    Frist {
        #[serde(default)]
        suche: String,
        #[serde(default)]
        empfangen: Option<String>,
    },
    Id {
        wert: String,
    },
    Pog {
        #[serde(default)]
        verbrauch_kwh: Option<rust_decimal::Decimal>,
        #[serde(default)]
        leistung_kw: Option<rust_decimal::Decimal>,
        #[serde(default)]
        sect14a: bool,
        #[serde(default)]
        optional: bool,
        stichtag: String,
    },
    Eeg {
        art: String,
        leistung_kw: rust_decimal::Decimal,
        inbetriebnahme: String,
        #[serde(default)]
        voll: bool,
    },
    Preise {
        gruppe: String,
        von: i32,
        bis: i32,
    },
    Modul14a {
        arbeitspreis_ct: rust_decimal::Decimal,
        geraet_kwh: rust_decimal::Decimal,
        pauschale_eur: rust_decimal::Decimal,
    },
}

/// A refusal with its stable `code`.
pub(crate) fn refused(code: &str, message: impl Into<String>) -> Value {
    json!({ "refused": { "code": code, "message": message.into() } })
}

/// Answer one JSON request.
#[must_use]
pub fn call(request: &str) -> String {
    if request.len() > MAX_INPUT_BYTES + 4096 {
        return refused(
            "eingabe_zu_gross",
            format!(
                "Die Eingabe ist größer als {} MB.",
                MAX_INPUT_BYTES / 1024 / 1024
            ),
        )
        .to_string();
    }
    let answer = match serde_json::from_str::<Request>(request) {
        Err(e) => refused("anfrage_ungueltig", e.to_string()),
        Ok(Request::Samples) => json!({ "ok": samples::list() }),
        Ok(Request::Sample { name }) => match samples::text(&name) {
            Some(text) => json!({ "ok": { "name": name, "text": text } }),
            None => refused("beispiel_unbekannt", format!("Kein Beispiel {name:?}.")),
        },
        Ok(Request::Check { text, empfangen }) => check::check(&text, empfangen.as_deref()),
        Ok(Request::Codes { ebd, code }) => werkzeuge::codes(ebd.as_deref(), code.as_deref()),
        Ok(Request::Frist { suche, empfangen }) => match empfangen
            .as_deref()
            .map(|e| time::OffsetDateTime::parse(e, &time::format_description::well_known::Rfc3339))
        {
            Some(Err(_)) => refused(
                "empfangen_ungueltig",
                "Der Eingangszeitpunkt ist kein RFC-3339-Zeitpunkt.",
            ),
            t => werkzeuge::frist(&suche, t.and_then(Result::ok)),
        },
        Ok(Request::Id { wert }) => werkzeuge::id(&wert),
        Ok(Request::Pog {
            verbrauch_kwh,
            leistung_kw,
            sect14a,
            optional,
            stichtag,
        }) => rechner::pog(verbrauch_kwh, leistung_kw, sect14a, optional, &stichtag),
        Ok(Request::Eeg {
            art,
            leistung_kw,
            inbetriebnahme,
            voll,
        }) => rechner::eeg(&art, leistung_kw, &inbetriebnahme, voll),
        Ok(Request::Preise { gruppe, von, bis }) => rechner::preise(&gruppe, von, bis),
        Ok(Request::Modul14a {
            arbeitspreis_ct,
            geraet_kwh,
            pauschale_eur,
        }) => rechner::modul14a(arbeitspreis_ct, geraet_kwh, pauschale_eur),
    };
    answer.to_string()
}

// ── The C ABI ─────────────────────────────────────────────────────────────────

/// A buffer of `len` bytes for the host to write a request into.
#[unsafe(no_mangle)]
#[expect(unsafe_code, reason = "the C ABI the browser calls")]
pub extern "C" fn mw_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Free a buffer from [`mw_alloc`] or [`mw_call`].
///
/// # Safety
///
/// `ptr` and `len` must describe a buffer this module handed out and the host
/// has not freed.
#[unsafe(no_mangle)]
#[expect(unsafe_code, reason = "the C ABI the browser calls")]
pub unsafe extern "C" fn mw_free(ptr: *mut u8, len: usize) {
    // SAFETY: the caller passes back a buffer `mw_alloc` or `mw_call` created
    // with exactly this capacity.
    drop(unsafe { Vec::from_raw_parts(ptr, 0, len) });
}

/// Answer the request in `ptr..ptr + len`; the answer packed as
/// `(ptr << 32) | len`.
///
/// # Safety
///
/// `ptr` and `len` must describe a buffer from [`mw_alloc`] holding `len`
/// written bytes.
#[unsafe(no_mangle)]
#[expect(unsafe_code, reason = "the C ABI the browser calls")]
pub unsafe extern "C" fn mw_call(ptr: *const u8, len: usize) -> u64 {
    // SAFETY: the host wrote `len` bytes into a buffer from `mw_alloc`.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let answer = match std::str::from_utf8(bytes) {
        Ok(request) => call(request),
        Err(_) => refused("anfrage_ungueltig", "Die Anfrage ist kein UTF-8.").to_string(),
    };
    let mut out = answer.into_bytes();
    out.shrink_to_fit();
    let (out_ptr, out_len) = (out.as_mut_ptr(), out.len());
    std::mem::forget(out);
    // A wasm32 pointer and length each fit 32 bits.
    ((out_ptr as usize as u64) << 32) | out_len as u64
}
