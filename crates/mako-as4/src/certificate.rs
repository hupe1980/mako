//! What a certificate of the AS4 Zertifikatstripel must carry.
//!
//! Regelungen zum Übertragungsweg für AS4 2.6, Kap. 5.2 (S. 6): „Das Feld
//! Organisational Unit („OU“) des Subject muss die MP-ID enthalten.“ and the
//! URI-type subject alternative name „muss vorhanden sein und mit der
//! Kommunikationsadresse des WebServices befüllt werden. Mehrere
//! Kommunikationsadressen in einem Zertifikat sind nicht zulässig.“
//!
//! `asx-rs` holds the first rule for a partner's signing certificates only;
//! the encryption and TLS certificates, the local party's certificates and the
//! subject alternative name are read here.

use crate::keys::{element_len, skip, tlv};

/// `id-at-organizationalUnitName` (2.5.4.11), DER-encoded with its tag.
const OU_OID: &[u8] = &[0x06, 0x03, 0x55, 0x04, 0x0B];
/// `id-ce-subjectAltName` (2.5.29.17), DER-encoded with its tag.
const SAN_OID: &[u8] = &[0x06, 0x03, 0x55, 0x1D, 0x11];
/// `GeneralName` `uniformResourceIdentifier` — `[6] IMPLICIT IA5String`.
const URI_TAG: u8 = 0x86;

/// One TLV: its tag, the whole encoding and its content.
type Tlv<'a> = (u8, &'a [u8], &'a [u8]);

/// The TLVs that make up `content`.
fn children(mut content: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let mut out = Vec::new();
    while !content.is_empty() {
        let tag = *content.first()?;
        let len = element_len(content)?;
        let whole = content.get(..len)?;
        let (inner, _) = tlv(whole, tag)?;
        out.push((tag, whole, inner));
        content = skip(content)?;
    }
    Some(out)
}

/// The `TBSCertificate` fields after `version`: serialNumber, signature,
/// issuer, validity, subject, subjectPublicKeyInfo and what follows.
fn tbs_fields(cert_der: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let (certificate, _) = tlv(cert_der, 0x30)?;
    let (tbs, _) = tlv(certificate, 0x30)?;
    let mut fields = children(tbs)?;
    if fields.first().map(|f| f.0) == Some(0xA0) {
        fields.remove(0);
    }
    (fields.len() >= 6).then_some(fields)
}

/// The subject's `OU` values, or `None` when the DER is not a certificate
/// this reader understands.
#[must_use]
pub fn subject_organisational_units(cert_der: &[u8]) -> Option<Vec<String>> {
    let fields = tbs_fields(cert_der)?;
    let (_, _, subject) = fields[4];
    let mut out = Vec::new();
    for (_, _, rdn) in children(subject)? {
        for (_, _, attribute) in children(rdn)? {
            let parts = children(attribute)?;
            let [(_, oid, _), (_, _, value)] = parts.as_slice() else {
                return None;
            };
            if *oid == OU_OID {
                out.push(String::from_utf8_lossy(value).into_owned());
            }
        }
    }
    Some(out)
}

/// Whether one subject `OU` of `cert` is `mp_id`. An unreadable certificate
/// carries nothing.
#[must_use]
pub fn carries_mp_id(cert: &asx::Certificate, mp_id: &str) -> bool {
    subject_organisational_units(cert.der())
        .is_some_and(|ous| ous.iter().any(|ou| ou.trim() == mp_id))
}

/// The URI-type subject alternative names, empty when the certificate has
/// none, or `None` when the DER is not a certificate this reader understands.
#[must_use]
pub fn uri_subject_alt_names(cert_der: &[u8]) -> Option<Vec<String>> {
    let fields = tbs_fields(cert_der)?;
    let mut out = Vec::new();
    for (tag, _, content) in &fields[6..] {
        if *tag != 0xA3 {
            continue;
        }
        let (extensions, _) = tlv(content, 0x30)?;
        for (_, _, extension) in children(extensions)? {
            let parts = children(extension)?;
            let (_, oid, _) = parts.first()?;
            if *oid != SAN_OID {
                continue;
            }
            let (_, _, value) = parts.last()?;
            let (names, _) = tlv(value, 0x30)?;
            for (tag, _, name) in children(names)? {
                if tag == URI_TAG {
                    out.push(String::from_utf8_lossy(name).into_owned());
                }
            }
        }
    }
    Some(out)
}

/// Test certificates with a chosen subject `OU` and URI subject alternative
/// names, which `asx::testkit::pki` does not issue. The signature is a
/// placeholder: nothing here verifies it.
#[cfg(test)]
pub(crate) mod test_cert {
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

    fn name(ou: &str) -> Vec<u8> {
        let attr = |oid: &[u8], value: &str| {
            tlv_of(
                0x31,
                &tlv_of(0x30, &[oid, &tlv_of(0x0C, value.as_bytes())].concat()),
            )
        };
        tlv_of(
            0x30,
            &[
                attr(&[0x06, 0x03, 0x55, 0x04, 0x03], "test"),
                attr(super::OU_OID, ou),
            ]
            .concat(),
        )
    }

    fn base64(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(char::from(A[((n >> (18 - 6 * i)) & 0x3F) as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A certificate for `key`'s public key with subject `OU` = `ou` and one
    /// URI subject alternative name per entry of `uris` (none: no extension).
    pub(crate) fn with(key: &asx::Certificate, ou: &str, uris: &[&str]) -> asx::Certificate {
        let spki = crate::keys::subject_public_key_info(key.der()).unwrap();
        let ecdsa_sha256 = tlv_of(
            0x30,
            &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02],
        );
        let validity = tlv_of(
            0x30,
            &[
                tlv_of(0x17, b"250101000000Z"),
                tlv_of(0x18, b"20991231235959Z"),
            ]
            .concat(),
        );
        let mut tbs = [
            tlv_of(0xA0, &tlv_of(0x02, &[2])),
            tlv_of(0x02, &[0x01, 0x23]),
            ecdsa_sha256.clone(),
            name("issuer"),
            validity,
            name(ou),
            spki.to_vec(),
        ]
        .concat();
        if !uris.is_empty() {
            let names: Vec<u8> = uris
                .iter()
                .flat_map(|u| tlv_of(super::URI_TAG, u.as_bytes()))
                .collect();
            let san = tlv_of(
                0x30,
                &[super::SAN_OID, &tlv_of(0x04, &tlv_of(0x30, &names))].concat(),
            );
            tbs.extend(tlv_of(0xA3, &tlv_of(0x30, &san)));
        }
        let signature = tlv_of(
            0x03,
            &[
                &[0x00][..],
                &[0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01],
            ]
            .concat(),
        );
        let der = tlv_of(
            0x30,
            &[tlv_of(0x30, &tbs), ecdsa_sha256, signature].concat(),
        );
        let b64 = base64(&der);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for line in b64.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap());
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        asx::Certificate::from_pem(pem.as_bytes()).expect("a parseable test certificate")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asx::testkit::pki::TestCa;

    #[test]
    fn the_subject_ou_of_a_bdew_party_certificate_is_its_mp_id() {
        let party = TestCa::sm_pki().bdew_party("9900357000004");
        for cert in [
            party.signing.certificate(),
            party.encryption.certificate(),
            party.tls.certificate(),
        ] {
            assert_eq!(
                subject_organisational_units(cert.der()),
                Some(vec!["9900357000004".to_owned()])
            );
            assert!(carries_mp_id(&cert, "9900357000004"));
            assert!(!carries_mp_id(&cert, "9900357000005"));
        }
    }

    #[test]
    fn uri_subject_alt_names_are_read() {
        let key = TestCa::sm_pki()
            .bdew_party("9900357000004")
            .encryption
            .certificate();
        let none = test_cert::with(&key, "9900357000004", &[]);
        assert_eq!(uri_subject_alt_names(none.der()), Some(Vec::new()));
        assert_eq!(uri_subject_alt_names(key.der()), Some(Vec::new()));
        let two = test_cert::with(
            &key,
            "9900357000004",
            &["https://a.example/as4", "https://b.example/as4"],
        );
        assert_eq!(
            uri_subject_alt_names(two.der()),
            Some(vec![
                "https://a.example/as4".to_owned(),
                "https://b.example/as4".to_owned()
            ])
        );
        assert!(carries_mp_id(&two, "9900357000004"));
    }

    #[test]
    fn unreadable_der_is_not_read() {
        assert_eq!(subject_organisational_units(&[0x30, 0x00]), None);
        assert_eq!(uri_subject_alt_names(&[]), None);
    }
}
