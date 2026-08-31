//! BDEW MaKo interchanges are `UNOC:3` — ISO 8859-1, not UTF-8.
//!
//! In `UNOC` the `ü` of "Prüfidentifikator" is the single byte `0xFC`. Handing
//! those bytes to a UTF-8 parser rejects a *conformant* interchange, and party
//! names, addresses and `FTX` free text carry umlauts routinely. These tests
//! pin both directions of that boundary.
//!
//! Gated on `aperak` because the fixture is an APERAK: without the feature the
//! message type is not registered and `parse` cannot dispatch it.
#![cfg(feature = "aperak")]

use edi_energy::EdiEnergyMessage as _;

/// The APERAK fixture, whose `FTX` carries "Prüfidentifikator ungültig".
///
/// Stored as **ISO 8859-1**, which is what its own `UNB+UNOC:3` declares — so
/// these are the bytes a conformant counterparty actually sends. Read as bytes
/// rather than `include_str!`: the file is deliberately not UTF-8.
const FIXTURE: &[u8] =
    include_bytes!("fixtures/aperak/valid/beispiel_29001_verarbeitbarkeitsfehler.edi");

/// A conformant `UNOC` interchange — umlauts as single Latin-1 bytes — must
/// parse, with the text arriving as correct Rust `String` content.
#[test]
fn a_conformant_unoc_interchange_with_umlauts_parses() {
    let latin1 = FIXTURE;
    assert!(
        latin1.contains(&0xFC),
        "the fixture must exercise a non-ASCII Latin-1 byte"
    );
    assert!(
        !latin1.windows(2).any(|w| w == [0xC3, 0xBC]),
        "Latin-1 input must not contain the UTF-8 encoding of ü"
    );

    let msg = edi_energy::parse(latin1).expect("conformant UNOC interchange must parse");

    // Assert on the decoded content wherever it sits: this test is about the
    // character repertoire, not about which FTX element carries the text.
    let carries_umlaut = msg.segments().iter().any(|seg| {
        seg.elements.iter().any(|el| {
            el.components
                .iter()
                .any(|(v, _)| v.contains("Prüfidentifikator") && v.contains("ungültig"))
        })
    });
    assert!(
        carries_umlaut,
        "decoded interchange lost its umlauts: {:?}",
        msg.segments().iter().map(|s| &*s.tag).collect::<Vec<_>>()
    );
}

/// `UNOY` is UTF-8, and ASCII payloads are unaffected — the zero-copy path must
/// keep working for the interchanges the repertoire logic does not touch.
#[test]
fn ascii_payloads_are_unaffected() {
    let decoded: String = FIXTURE.iter().map(|&b| b as char).collect();
    let ascii: String = decoded.replace('ü', "ue").replace('ä', "ae");
    assert!(ascii.is_ascii());
    edi_energy::parse(ascii.as_bytes()).expect("ASCII UNOC interchange must parse");
}

/// The same bytes must decode identically through the streaming path, which
/// sniffs the `UNB` without reading the whole interchange into memory.
#[test]
fn the_streaming_path_decodes_the_same_repertoire() {
    let msgs: Vec<_> =
        edi_energy::parse_interchange(std::io::Cursor::new(FIXTURE.to_vec())).collect();
    assert!(
        msgs.iter().any(std::result::Result::is_ok),
        "streaming parse of a conformant UNOC interchange yielded no message: {msgs:?}"
    );
}

/// **Every** reader-based entry point must decode the repertoire, not just the
/// streaming one.
///
/// `Parser::parse_reader`, `parse_interchange_buffered` and the reader form of
/// `parse_interchange_full` each have their own path to the tokeniser. One that
/// skipped the transcode would make the same conformant interchange parse or
/// fail depending only on which overload the caller reached for — and the
/// reader forms are what an AS4 adapter uses, where the payload arrives as a
/// stream.
#[test]
fn every_reader_entry_point_decodes_the_declared_repertoire() {
    use edi_energy::Parser;

    let parser = Parser::new();

    parser
        .parse_reader(std::io::Cursor::new(FIXTURE.to_vec()))
        .expect("parse_reader must decode UNOC");

    let (_header, iter) = parser
        .parse_interchange_buffered(std::io::Cursor::new(FIXTURE.to_vec()))
        .expect("parse_interchange_buffered must decode UNOC");
    let buffered: Vec<_> = iter.collect();
    assert!(
        buffered.iter().any(std::result::Result::is_ok),
        "buffered parse yielded no message: {buffered:?}"
    );

    let full = parser
        .parse_interchange_full(std::io::Cursor::new(FIXTURE.to_vec()))
        .expect("parse_interchange_full must decode UNOC");
    assert!(!full.messages.is_empty());
}

/// An outbound interchange must be encoded into the repertoire its own `UNB`
/// declares. A `UNOC` header over a UTF-8 body is mojibake at the counterparty
/// with nothing in the file to explain it — and mako's own parser, which now
/// decodes by the declared repertoire, would read it back wrong too.
#[test]
fn an_outbound_interchange_is_encoded_into_its_declared_repertoire() {
    use edi_energy::builders::InterchangeBuilder;

    let hostile = "Zählpunkt ungültig";
    // A minimal UNH…UNT, rendered as UTF-8 the way the message builders do.
    let message = format!("UNH+1+APERAK:D:07B:UN:2.4a'BGM+313+1000+9'FTX+ABO+++{hostile}'UNT+4+1'")
        .into_bytes();

    let wire = InterchangeBuilder::new("9900987654321", "9900123456789", "REF-CS-1")
        .transmission("260809", "0915")
        .message(message)
        .build()
        .expect("build interchange");

    assert!(
        wire.starts_with(b"UNB+UNOC:3+"),
        "the fixture must declare UNOC"
    );
    assert!(
        wire.contains(&0xE4) && wire.contains(&0xFC),
        "ä and ü must be single Latin-1 bytes on the wire"
    );
    assert!(
        !wire
            .windows(2)
            .any(|w| w == [0xC3, 0xA4] || w == [0xC3, 0xBC]),
        "no UTF-8 multi-byte sequence may survive into a UNOC interchange"
    );

    // The round trip closes: our own parser decodes by the declared repertoire.
    let back = edi_energy::parse_interchange(std::io::Cursor::new(wire))
        .next()
        .expect("one message")
        .expect("parses");
    let carries = back.segments().iter().any(|seg| {
        seg.elements
            .iter()
            .any(|el| el.components.iter().any(|(v, _)| v.contains(hostile)))
    });
    assert!(
        carries,
        "the free text must survive the encode/decode round trip"
    );
}

/// Every fixture must be stored in the repertoire its own `UNB` declares.
///
/// A fixture that declares `UNOC:3` (ISO 8859-1) while being stored as UTF-8 is
/// not what a counterparty sends, and it hides the very bug this module exists
/// to prevent: the parser decodes by the declared repertoire, so a UTF-8 `ü`
/// (`C3 BC`) in a `UNOC` fixture comes back as `Ã¼` and every assertion that
/// does not look at the text still passes.
#[test]
fn fixtures_are_stored_in_the_repertoire_they_declare() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("edi") {
                out.push(path);
            }
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(
        !files.is_empty(),
        "no fixtures found under {}",
        root.display()
    );

    let mut offenders = Vec::new();
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        // Only single-byte repertoires can be misstored this way; UNOY is UTF-8.
        let declares_single_byte = bytes
            .windows(9)
            .any(|w| w.starts_with(b"UNB+UNO") && w[7] != b'Y');
        if !declares_single_byte {
            continue;
        }
        // A valid multi-byte UTF-8 sequence in a single-byte repertoire is the
        // signature of a file saved as UTF-8 against its own declaration.
        if bytes.iter().any(|&b| b >= 0x80) && std::str::from_utf8(&bytes).is_ok() {
            offenders.push(
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        }
    }

    assert!(
        offenders.is_empty(),
        "these fixtures declare a single-byte repertoire but are stored as UTF-8 \
         — save them in the declared repertoire so they match what a counterparty \
         sends:\n  {}",
        offenders.join("\n  ")
    );
}
