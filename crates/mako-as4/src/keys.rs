//! Public-key comparison of certificates.
//!
//! BDEW AS4-Profil 1.2 §2.3.4.4 (S. 17): „Für die Signatur, die Verschlüsselung
//! und den Aufbau der TLS-Verbindung MÜSSEN unterschiedliche Schlüsselpaare
//! verwendet werden." `asx-rs` refuses two of the three *certificates* being
//! equal; two certificates issued for one key pair pass that check. The key
//! pair is what the rule is about, so it is compared here by the certificates'
//! `subjectPublicKeyInfo`.

/// The DER `subjectPublicKeyInfo` of an X.509 certificate, or `None` when the
/// DER is not a certificate this reader understands.
///
/// RFC 5280 §4.1: `Certificate ::= SEQUENCE { tbsCertificate, … }` and
/// `TBSCertificate ::= SEQUENCE { [0] version OPTIONAL, serialNumber,
/// signature, issuer, validity, subject, subjectPublicKeyInfo, … }`.
#[must_use]
pub fn subject_public_key_info(cert_der: &[u8]) -> Option<&[u8]> {
    let (certificate, _) = tlv(cert_der, 0x30)?;
    let (tbs, _) = tlv(certificate, 0x30)?;
    let mut rest = tbs;
    if rest.first() == Some(&0xA0) {
        rest = skip(rest)?;
    }
    // serialNumber, signature, issuer, validity, subject
    for _ in 0..5 {
        rest = skip(rest)?;
    }
    let len = element_len(rest)?;
    (rest.first() == Some(&0x30)).then(|| &rest[..len])
}

/// Whether two certificates certify the same public key. Unreadable DER
/// compares as the same key: refusing a configuration is the safe answer.
#[must_use]
pub fn same_public_key(a: &asx::Certificate, b: &asx::Certificate) -> bool {
    match (
        subject_public_key_info(a.der()),
        subject_public_key_info(b.der()),
    ) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// The content of the TLV at the head of `der` when its tag is `tag`, and
/// what follows it.
fn tlv(der: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    if *der.first()? != tag {
        return None;
    }
    let (header, len) = header_and_len(der)?;
    let end = header.checked_add(len)?;
    Some((der.get(header..end)?, der.get(end..)?))
}

/// `der` without its first TLV.
fn skip(der: &[u8]) -> Option<&[u8]> {
    der.get(element_len(der)?..)
}

/// The total length (header and content) of the TLV at the head of `der`.
fn element_len(der: &[u8]) -> Option<usize> {
    let (header, len) = header_and_len(der)?;
    let total = header.checked_add(len)?;
    (total <= der.len()).then_some(total)
}

/// The header length and the content length of the TLV at the head of `der`.
fn header_and_len(der: &[u8]) -> Option<(usize, usize)> {
    let first = *der.get(1)?;
    if first & 0x80 == 0 {
        return Some((2, usize::from(first)));
    }
    let n = usize::from(first & 0x7F);
    if n == 0 || n > 4 {
        return None;
    }
    let mut len = 0usize;
    for b in der.get(2..2 + n)? {
        len = (len << 8) | usize::from(*b);
    }
    Some((2 + n, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DER TLV with a short or long length.
    fn tlv_of(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if content.len() < 0x80 {
            out.push(u8::try_from(content.len()).unwrap());
        } else {
            let len = u16::try_from(content.len()).unwrap().to_be_bytes();
            out.extend_from_slice(&[0x82, len[0], len[1]]);
        }
        out.extend_from_slice(content);
        out
    }

    /// A certificate-shaped DER with `spki` in its seventh TBS position.
    fn cert_with(spki: &[u8], with_version: bool) -> Vec<u8> {
        let mut tbs = Vec::new();
        if with_version {
            tbs.extend(tlv_of(0xA0, &tlv_of(0x02, &[2])));
        }
        tbs.extend(tlv_of(0x02, &[1, 2, 3]));
        tbs.extend(tlv_of(0x30, &[0x06, 0x01, 0x2A]));
        tbs.extend(tlv_of(0x30, &[0u8; 200]));
        tbs.extend(tlv_of(0x30, &[]));
        tbs.extend(tlv_of(0x30, &[0x31, 0x00]));
        tbs.extend(tlv_of(0x30, spki));
        let mut cert = tlv_of(0x30, &tbs);
        cert = tlv_of(
            0x30,
            &[cert, tlv_of(0x30, &[]), tlv_of(0x03, &[0])].concat(),
        );
        cert
    }

    #[test]
    fn the_spki_is_found_with_and_without_a_version() {
        for v in [true, false] {
            let der = cert_with(&[0x05, 0x00, 0x03, 0x02, 0x00, 0x07], v);
            assert_eq!(
                subject_public_key_info(&der),
                Some(&tlv_of(0x30, &[0x05, 0x00, 0x03, 0x02, 0x00, 0x07])[..])
            );
        }
    }

    #[test]
    fn truncated_der_is_not_read() {
        let der = cert_with(&[1, 2, 3], true);
        assert_eq!(subject_public_key_info(&der[..der.len() - 20]), None);
        assert_eq!(subject_public_key_info(&[]), None);
    }

    /// Two certificates of one key pair are one key; two key pairs are two.
    #[test]
    fn certificates_compare_by_their_public_key() {
        let ca = asx::testkit::pki::TestCa::sm_pki();
        let party = ca.bdew_party("9900357000004");
        let sig = party.signing.certificate();
        let enc = party.encryption.certificate();
        assert!(same_public_key(&sig, &sig));
        assert!(!same_public_key(&sig, &enc));
        // A second party has key pairs of its own.
        let other = ca.bdew_party("9900357000004");
        assert!(!same_public_key(&sig, &other.signing.certificate()));
    }
}
