//! Path validation of a certificate against the SM-PKI trust anchors.
//!
//! A counterparty's EMT.API signature certificate is what a Verzeichnisdienst
//! record is verified against, and „bei der Validierung der Gültigkeit von
//! Zertifikaten aus der SM-PKI sind die Vorgaben aus CP einzuhalten"
//! (Regelungen zum Verzeichnisdienst 1.1, Kap. 3.2, S. 7). A pinned leaf that
//! is not checked to chain to the SM-PKI root is trusted because a file says
//! so, so a configured certificate is checked here — leaf, then its issuers
//! in the order given, up to a configured anchor:
//!
//! - each certificate is inside its validity period at the time checked;
//! - each is named as issued by the next (issuer = the next one's subject) and
//!   its signature verifies under the next one's key;
//! - every issuer is a CA (`basicConstraints cA`);
//! - the last issuer is one of the anchors, byte for byte, or is signed by one.
//!
//! The SM-PKI Certificate Policy is not mirrored, so revocation (the CRL the CP
//! prescribes) is not checked here. The issuer curves are brainpoolP256r1,
//! brainpoolP384r1 and NIST P-256, with ECDSA over SHA-256/384/512; any other
//! algorithm is refused, not skipped.

use std::time::SystemTime;

use der::asn1::ObjectIdentifier;
use der::{Decode as _, Encode as _};
use ecdsa::signature::hazmat::PrehashVerifier as _;
use x509_cert::Certificate;
use x509_cert::ext::pkix::BasicConstraints;

const EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
const P256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
const BRAINPOOL_P256R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.36.3.3.2.8.1.1.7");
const BRAINPOOL_P384R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.36.3.3.2.8.1.1.11");
const ECDSA_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
const ECDSA_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");
const ECDSA_SHA512: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.4");

/// Every `CERTIFICATE` block of a PEM text, as DER.
///
/// # Errors
///
/// A block that is not Base64, or a text with none.
pub fn pem_certificates(pem: &str) -> Result<Vec<Vec<u8>>, String> {
    use base64::Engine as _;
    let mut out = Vec::new();
    let mut body: Option<String> = None;
    for line in pem.lines().map(str::trim) {
        match (line, body.as_mut()) {
            ("-----BEGIN CERTIFICATE-----", None) => body = Some(String::new()),
            ("-----END CERTIFICATE-----", Some(b)) => {
                out.push(
                    base64::engine::general_purpose::STANDARD
                        .decode(b.as_bytes())
                        .map_err(|e| format!("not a PEM CERTIFICATE: {e}"))?,
                );
                body = None;
            }
            (l, Some(b)) => b.push_str(l),
            _ => {}
        }
    }
    if out.is_empty() {
        return Err("no PEM CERTIFICATE block".to_owned());
    }
    Ok(out)
}

/// Check that `chain` — the leaf first, then its issuers — chains to one of
/// `anchors` at `now`.
///
/// # Errors
///
/// What breaks the path, named.
pub fn chains_to(chain: &[Vec<u8>], anchors: &[Vec<u8>], now: SystemTime) -> Result<(), String> {
    if anchors.is_empty() {
        return Err("no SM-PKI trust anchor is configured".to_owned());
    }
    let parse = |der: &[u8]| {
        Certificate::from_der(der).map_err(|e| format!("not an X.509 certificate: {e}"))
    };
    let certs: Vec<Certificate> = chain.iter().map(|d| parse(d)).collect::<Result<_, _>>()?;
    let anchors: Vec<Certificate> = anchors.iter().map(|d| parse(d)).collect::<Result<_, _>>()?;
    let Some(leaf) = certs.first() else {
        return Err("the certificate file is empty".to_owned());
    };
    let is_anchor = |c: &Certificate| anchors.iter().any(|a| a == c);
    if is_anchor(leaf) {
        return Err("the leaf is a trust anchor itself, not a certificate issued under one".into());
    }
    for (i, cert) in certs.iter().enumerate() {
        within_validity(cert, now)?;
        if is_anchor(cert) {
            return Ok(());
        }
        let issuer = match certs.get(i + 1) {
            Some(next) => next,
            None => anchors
                .iter()
                .find(|a| a.tbs_certificate.subject == cert.tbs_certificate.issuer)
                .ok_or_else(|| {
                    format!(
                        "{} is issued by {}, which is neither in the file nor a configured \
                         trust anchor",
                        cert.tbs_certificate.subject, cert.tbs_certificate.issuer
                    )
                })?,
        };
        issued_by(cert, issuer)?;
        if certs.get(i + 1).is_none() {
            within_validity(issuer, now)?;
            return Ok(());
        }
    }
    Err("the chain ends before a trust anchor".to_owned())
}

/// The pinned EMT.API certificate of `mp_id` in `pem` (leaf first, then its
/// issuers): it chains to `anchors` at `now` and its subject's OU carries
/// `mp_id` — the attribute an EMT.API certificate is assigned to its
/// Marktpartner by (Regelungen zum Verzeichnisdienst 1.1, Kap. 3.2, S. 7).
///
/// # Errors
///
/// What disqualifies it, named.
pub fn check_emt_api(
    pem: &str,
    mp_id: &str,
    anchors: &[Vec<u8>],
    now: SystemTime,
) -> Result<(), String> {
    let chain = pem_certificates(pem)?;
    chains_to(&chain, anchors, now)?;
    let leaf =
        Certificate::from_der(&chain[0]).map_err(|e| format!("not an X.509 certificate: {e}"))?;
    let subject = leaf.tbs_certificate.subject.to_string();
    if !subject
        .split(',')
        .any(|rdn| rdn.trim() == format!("OU={mp_id}"))
    {
        return Err(format!(
            "the certificate's subject {subject} carries no OU={mp_id}"
        ));
    }
    Ok(())
}

fn within_validity(cert: &Certificate, now: SystemTime) -> Result<(), String> {
    let v = &cert.tbs_certificate.validity;
    if now < v.not_before.to_system_time() || now > v.not_after.to_system_time() {
        return Err(format!(
            "{} is outside its validity ({} – {})",
            cert.tbs_certificate.subject, v.not_before, v.not_after
        ));
    }
    Ok(())
}

/// `cert` names `issuer` as its issuer, `issuer` is a CA, and `issuer`'s key
/// verifies `cert`'s signature.
fn issued_by(cert: &Certificate, issuer: &Certificate) -> Result<(), String> {
    let tbs = &cert.tbs_certificate;
    if tbs.issuer != issuer.tbs_certificate.subject {
        return Err(format!(
            "{} names {} as issuer, but the next certificate is {}",
            tbs.subject, tbs.issuer, issuer.tbs_certificate.subject
        ));
    }
    let ca = issuer
        .tbs_certificate
        .get::<BasicConstraints>()
        .map_err(|e| {
            format!(
                "basicConstraints of {}: {e}",
                issuer.tbs_certificate.subject
            )
        })?
        .is_some_and(|(_, bc)| bc.ca);
    if !ca {
        return Err(format!(
            "{} signs {} but is no CA (basicConstraints cA)",
            issuer.tbs_certificate.subject, tbs.subject
        ));
    }
    let tbs_der = tbs
        .to_der()
        .map_err(|e| format!("re-encoding the TBSCertificate: {e}"))?;
    let digest = match cert.signature_algorithm.oid {
        o if o == ECDSA_SHA256 => sha2_digest::<sha2::Sha256>(&tbs_der),
        o if o == ECDSA_SHA384 => sha2_digest::<sha2::Sha384>(&tbs_der),
        o if o == ECDSA_SHA512 => sha2_digest::<sha2::Sha512>(&tbs_der),
        o => return Err(format!("signature algorithm {o} is not ECDSA with SHA-2")),
    };
    let sig = cert
        .signature
        .as_bytes()
        .ok_or("the signature is not a whole number of bytes")?;
    let spki = &issuer.tbs_certificate.subject_public_key_info;
    if spki.algorithm.oid != EC_PUBLIC_KEY {
        return Err(format!(
            "the issuer key of {} is not an EC key",
            tbs.subject
        ));
    }
    let curve = spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.decode_as::<ObjectIdentifier>().ok())
        .ok_or("the issuer key names no named curve")?;
    let key = spki.subject_public_key.raw_bytes();
    macro_rules! verify {
        ($curve:ty) => {{
            let vk = ecdsa::VerifyingKey::<$curve>::from_sec1_bytes(key)
                .map_err(|e| format!("the issuer key: {e}"))?;
            let sig = ecdsa::der::Signature::<$curve>::from_bytes(sig)
                .map_err(|e| format!("the signature encoding: {e}"))?;
            vk.verify_prehash(&digest, &sig)
        }};
    }
    let verified = match curve {
        c if c == P256 => verify!(p256::NistP256),
        c if c == BRAINPOOL_P256R1 => verify!(bp256::BrainpoolP256r1),
        c if c == BRAINPOOL_P384R1 => verify!(bp384::BrainpoolP384r1),
        c => return Err(format!("the issuer's curve {c} is not one the SM-PKI uses")),
    };
    verified.map_err(|_| {
        format!(
            "the signature of {} does not verify under {}",
            tbs.subject, issuer.tbs_certificate.subject
        )
    })
}

fn sha2_digest<D: sha2::Digest>(data: &[u8]) -> Vec<u8> {
    D::digest(data).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use asx::testkit::pki::{KeyAlg, TestCa};

    fn der(pem: &[u8]) -> Vec<u8> {
        pem_certificates(std::str::from_utf8(pem).unwrap())
            .unwrap()
            .remove(0)
    }

    /// Leaf → Sub-CA → root, on the SM-PKI's Brainpool curve and on P-256.
    #[test]
    fn a_leaf_issued_under_the_anchor_chains() {
        for alg in [KeyAlg::EcBrainpoolP256r1, KeyAlg::EcP256] {
            let root = TestCa::new("SM-Root", alg);
            let sub = root.sub_ca("SM-Sub", alg);
            let leaf = sub.issue("9900001000002", KeyAlg::EcP256);
            let chain = vec![der(&leaf.cert_pem), der(sub.cert_pem())];
            chains_to(&chain, &[root.cert_der()], SystemTime::now())
                .unwrap_or_else(|e| panic!("{alg:?}: {e}"));
            // Without the Sub-CA in the file, the root alone does not reach it.
            assert!(chains_to(&chain[..1], &[root.cert_der()], SystemTime::now()).is_err());
        }
    }

    /// A self-signed leaf, or one issued under another root, is refused.
    #[test]
    fn a_leaf_outside_the_anchor_is_refused() {
        let root = TestCa::new("SM-Root", KeyAlg::EcBrainpoolP256r1);
        let other = TestCa::new("Other-Root", KeyAlg::EcBrainpoolP256r1);
        let sub = other.sub_ca("Other-Sub", KeyAlg::EcBrainpoolP256r1);
        let leaf = sub.issue("9900001000002", KeyAlg::EcP256);
        let chain = vec![der(&leaf.cert_pem), der(sub.cert_pem())];
        let err = chains_to(&chain, &[root.cert_der()], SystemTime::now()).unwrap_err();
        assert!(err.contains("neither in the file nor"), "{err}");
        let selbst = asx::testkit::pki::KeyPair::self_signed("9900001000002", KeyAlg::EcP256);
        assert!(
            chains_to(
                &[der(&selbst.cert_pem)],
                &[root.cert_der()],
                SystemTime::now()
            )
            .is_err()
        );
        assert!(chains_to(&chain, &[], SystemTime::now()).is_err());
    }

    /// The OU names the MP-ID the certificate is pinned for.
    #[test]
    fn the_leaf_ou_must_name_the_pinned_mp_id() {
        use asx::testkit::pki::Nid;
        let root = TestCa::new("SM-Root", KeyAlg::EcBrainpoolP256r1);
        let sub = root.sub_ca("SM-Sub", KeyAlg::EcBrainpoolP256r1);
        let leaf = sub.issue_named(
            &[
                (Nid::COMMONNAME, "EMT.API sig"),
                (Nid::ORGANIZATIONALUNITNAME, "9900001000002"),
            ],
            KeyAlg::EcP256,
        );
        let pem = format!(
            "{}{}",
            String::from_utf8_lossy(&leaf.cert_pem),
            String::from_utf8_lossy(sub.cert_pem())
        );
        let anchors = [root.cert_der()];
        check_emt_api(&pem, "9900001000002", &anchors, SystemTime::now()).expect("its own");
        let err = check_emt_api(&pem, "9900001000003", &anchors, SystemTime::now()).unwrap_err();
        assert!(err.contains("OU=9900001000003"), "{err}");
    }

    /// An expired path is refused.
    #[test]
    fn a_path_outside_its_validity_is_refused() {
        let root = TestCa::new("SM-Root", KeyAlg::EcP256);
        let sub = root.sub_ca("SM-Sub", KeyAlg::EcP256);
        let leaf = sub.issue("9900001000002", KeyAlg::EcP256);
        let chain = vec![der(&leaf.cert_pem), der(sub.cert_pem())];
        let later = SystemTime::now() + std::time::Duration::from_secs(100 * 365 * 86_400);
        let err = chains_to(&chain, &[root.cert_der()], later).unwrap_err();
        assert!(err.contains("validity"), "{err}");
    }
}
