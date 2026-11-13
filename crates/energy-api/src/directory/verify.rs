//! Verification of a directory record — the one check the REST client and the
//! WebSocket client both apply.
//!
//! Regelungen zum Verzeichnisdienst 1.1, Kap. 2.2 (S. 6): „Die aktuelle Signatur
//! eines Eintrags MUSS jederzeit anhand eines gültigen Signaturzertifikats der
//! Klasse EMT.API des entsprechenden Marktpartners überprüft werden können".
//! Kap. 3 (S. 6) delivers the same information over both interfaces, so a
//! record pushed over the WebSocket is held to the same check as one fetched.

use std::collections::HashMap;

use crate::error::{Error, RecordRefusal};
use crate::models::directory::{ApiRecord, ApiRecordRef, DirectoryNotification, SignedApiRecord};
use crate::transport::jws::{self, RecordVerifyingKey};

/// Each provider's trusted EMT.API signature keys, by MP-ID — several for a
/// certificate rollover.
pub type TrustedSigners = HashMap<String, Vec<RecordVerifyingKey>>;

/// Verify `raw` — a record exactly as it arrived — against `signature` and
/// the provider's `signers`, and return it when it names `expected`.
///
/// The JWS covers the JCS form of the received JSON, not a re-serialised
/// [`ApiRecord`] (see [`jws::verify_value`]).
///
/// # Errors
/// - [`Error::UnverifiedRecord`]: `signers` is empty, `signature` is blank or
///   verifies against none of them, or the record names another entry.
/// - [`Error::Json`]: the verified record is not an [`ApiRecord`].
pub fn verify_record(
    raw: serde_json::Value,
    signature: &str,
    signers: &[RecordVerifyingKey],
    expected: &ApiRecordRef,
) -> Result<ApiRecord, Error> {
    if signers.is_empty() {
        return Err(Error::UnverifiedRecord(RecordRefusal::NoTrustedKey {
            provider_id: expected.provider_id.clone(),
        }));
    }
    let signature = signature.trim();
    if signature.is_empty() {
        return Err(Error::UnverifiedRecord(RecordRefusal::MissingSignature));
    }
    if !signers
        .iter()
        .any(|key| jws::verify_value(&raw, signature, key).is_ok())
    {
        return Err(Error::UnverifiedRecord(RecordRefusal::BadSignature {
            provider_id: expected.provider_id.clone(),
        }));
    }
    let record: ApiRecord = serde_json::from_value(raw)?;
    if record.provider_id != expected.provider_id
        || record.api_id != expected.api_id
        || record.major_version != expected.major_version
    {
        return Err(Error::UnverifiedRecord(RecordRefusal::WrongEntry {
            requested: format!(
                "{}/{}/{}",
                expected.provider_id, expected.api_id, expected.major_version
            ),
            received: format!(
                "{}/{}/{}",
                record.provider_id, record.api_id, record.major_version
            ),
        }));
    }
    Ok(record)
}

/// A WebSocket notification as received, every `modified` record verified.
///
/// Each record is checked by [`verify_record`] against the keys of the
/// provider its signed content names. A record that does not verify is
/// dropped from `modified` and reported as its own `Err`, ahead of the
/// notification carrying the verified rest — a refused record never reaches
/// the caller as data.
///
/// # Errors
/// The outer `Err` is a notification that is not JSON or not a
/// [`DirectoryNotification`].
pub fn verify_notification(
    text: &str,
    signers: &TrustedSigners,
) -> Result<(Vec<Error>, DirectoryNotification), Error> {
    let mut value: serde_json::Value = serde_json::from_str(text)?;
    let modified = value.as_object_mut().and_then(|o| o.remove("modified"));
    let mut refused = Vec::new();
    let mut verified = Vec::new();
    let had_modified = modified.is_some();
    for item in modified
        .and_then(|m| match m {
            serde_json::Value::Array(items) => Some(items),
            _ => None,
        })
        .unwrap_or_default()
    {
        match verify_signed(item, signers) {
            Ok(record) => verified.push(record),
            Err(e) => refused.push(e),
        }
    }
    let mut notification: DirectoryNotification = serde_json::from_value(value)?;
    if had_modified {
        notification.modified = Some(verified);
    }
    Ok((refused, notification))
}

/// One `SignedApiRecord` of a notification, verified.
fn verify_signed(
    mut item: serde_json::Value,
    signers: &TrustedSigners,
) -> Result<SignedApiRecord, Error> {
    let take = |item: &mut serde_json::Value, key: &str| {
        item.as_object_mut()
            .and_then(|o| o.remove(key))
            .unwrap_or(serde_json::Value::Null)
    };
    let content = take(&mut item, "content");
    let signature = take(&mut item, "signature");
    let signing_cert = take(&mut item, "signingCert");
    let names = |key: &str| content.get(key).cloned().unwrap_or_default();
    let expected = ApiRecordRef {
        provider_id: serde_json::from_value(names("providerId"))?,
        api_id: serde_json::from_value(names("apiId"))?,
        major_version: serde_json::from_value(names("majorVersion"))?,
    };
    let keys = signers
        .get(&expected.provider_id)
        .map_or(&[][..], Vec::as_slice);
    let signature = signature.as_str().unwrap_or_default().to_owned();
    let record = verify_record(content, &signature, keys, &expected)?;
    Ok(SignedApiRecord {
        content: record,
        signature,
        signing_cert: signing_cert.as_str().unwrap_or_default().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64ct::{Base64UrlUnpadded, Encoding as _};
    use p256::SecretKey;
    use p256::ecdsa::SigningKey;
    use p256::ecdsa::signature::Signer as _;

    const PROVIDER: &str = "9900357000004";

    fn signing_key() -> SigningKey {
        SigningKey::from(&SecretKey::from_slice(&[0x11; 32]).expect("a valid P-256 scalar"))
    }

    fn record(url: &str) -> serde_json::Value {
        serde_json::json!({
            "providerId": PROVIDER,
            "apiId": "maloIdV1",
            "majorVersion": 1,
            "url": url,
            "lastUpdated": "2026-10-01T00:00:00.5Z",
            "revision": 2,
            "status": "Online",
        })
    }

    fn signature_over(value: &serde_json::Value) -> String {
        let payload = serde_jcs::to_string(value).expect("JCS");
        let input = format!(
            "{}.{}",
            jws::JWS_PROTECTED_HEADER,
            Base64UrlUnpadded::encode_string(payload.as_bytes())
        );
        let sig: p256::ecdsa::Signature = signing_key().sign(input.as_bytes());
        Base64UrlUnpadded::encode_string(&sig.to_bytes())
    }

    fn signers() -> TrustedSigners {
        [(
            PROVIDER.to_owned(),
            vec![RecordVerifyingKey::from(&signing_key())],
        )]
        .into()
    }

    fn notification(content: &serde_json::Value, signature: &str) -> String {
        serde_json::json!({
            "timestamp": "2026-11-13T10:00:00Z",
            "modified": [{
                "content": content,
                "signature": signature,
                "signingCert": ":AAAA:",
            }],
            "deleted": [{ "providerId": PROVIDER, "apiId": "oldV1", "majorVersion": 1 }],
        })
        .to_string()
    }

    /// A pushed record whose endpoint changed after the provider signed it is
    /// refused, and the rest of the notification still arrives.
    #[test]
    fn a_tampered_pushed_record_is_refused() {
        let signature = signature_over(&record("https://lf.example/maloId/"));
        let text = notification(&record("https://attacker.example/maloId/"), &signature);
        let (refused, n) = verify_notification(&text, &signers()).expect("a notification");
        assert!(
            matches!(
                refused.as_slice(),
                [Error::UnverifiedRecord(RecordRefusal::BadSignature { .. })]
            ),
            "{refused:?}"
        );
        assert_eq!(n.modified.as_deref().map(<[_]>::len), Some(0));
        assert_eq!(n.deleted.as_deref().map(<[_]>::len), Some(1));
    }

    /// The acceptance twin: the signed record is delivered.
    #[test]
    fn a_signed_pushed_record_is_delivered() {
        let signed = record("https://lf.example/maloId/");
        let text = notification(&signed, &signature_over(&signed));
        let (refused, n) = verify_notification(&text, &signers()).expect("a notification");
        assert!(refused.is_empty(), "{refused:?}");
        let modified = n.modified.expect("modified");
        assert_eq!(
            modified[0].content.url.as_str(),
            "https://lf.example/maloId/"
        );
    }

    /// A provider without a trusted key has every pushed record refused.
    #[test]
    fn a_pushed_record_without_a_trusted_key_is_refused() {
        let signed = record("https://lf.example/maloId/");
        let text = notification(&signed, &signature_over(&signed));
        let (refused, _) =
            verify_notification(&text, &TrustedSigners::new()).expect("a notification");
        assert!(
            matches!(
                refused.as_slice(),
                [Error::UnverifiedRecord(RecordRefusal::NoTrustedKey { .. })]
            ),
            "{refused:?}"
        );
    }
}
