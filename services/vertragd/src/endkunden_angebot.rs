//! Reading the frozen Endkunden-Angebot an online order accepts from
//! `productd` (`GET /api/v1/endkunden-angebote/{id}`).
//!
//! `productd` prices and freezes the offer; `vertragd` only checks an order
//! against it and keeps the copy it accepted on the Bestellung, so a later
//! acceptance (`mit_annahme`) concludes exactly what was ordered.

use anyhow::{Context as _, Result};
use mako_service::http::Upstream;
use serde::Deserialize;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// One price line of the offer.
#[derive(Debug, Clone, Deserialize)]
pub struct Preis {
    pub art: String,
    pub bezeichnung: String,
    pub einheit: String,
    pub wert: String,
}

/// The fields of a frozen offer an order is checked against. A response, so
/// it reads tolerantly: a field `productd` adds is not an error here.
#[derive(Debug, Clone, Deserialize)]
pub struct EndkundenAngebot {
    pub id: Uuid,
    pub product_code: String,
    pub product_version: String,
    /// `STROM` | `GAS`.
    pub sparte: String,
    pub malo_id: Option<String>,
    pub nb_mp_id: String,
    pub verbrauch_kwh: String,
    pub lieferbeginn_fruehestens: Date,
    pub preise: Vec<Preis>,
    #[serde(with = "time::serde::rfc3339")]
    pub gueltig_bis: OffsetDateTime,
    pub content_sha256: String,
}

/// Client for `productd`.
pub struct ProductdClient {
    up: Upstream,
}

impl ProductdClient {
    #[must_use]
    pub fn new(base_url: &str, api_key: Option<String>, http: reqwest::Client) -> Self {
        Self {
            up: Upstream::new(
                "productd",
                base_url,
                api_key.map(secrecy::SecretString::from),
                http,
            ),
        }
    }

    /// The offer as stored, and its raw JSON; `None` for an unknown id.
    ///
    /// # Errors
    ///
    /// Propagates transport failures and an answer that is not an offer.
    pub async fn angebot(&self, id: Uuid) -> Result<Option<(EndkundenAngebot, serde_json::Value)>> {
        let raw: Option<serde_json::Value> = self
            .up
            .json(self.up.get(&format!("/api/v1/endkunden-angebote/{id}")))
            .await
            .context("productd GET endkunden-angebot")?;
        raw.map(|raw| {
            let typed = serde_json::from_value(raw.clone())
                .context("productd answered with no Endkunden-Angebot")?;
            Ok((typed, raw))
        })
        .transpose()
    }
}

/// Every way an order disagrees with the offer it accepts, as stable codes —
/// the order's MaLo-ID, the hash, the validity, the Lieferbeginn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Abweichung {
    /// `angebot_hash_abweichend`.
    Hash,
    /// `angebot_abgelaufen`.
    Abgelaufen,
    /// `malo_id_abweichend`.
    MaloId,
    /// `lieferbeginn_zu_frueh` with the earliest date.
    LieferbeginnZuFrueh(Date),
}

/// Check an order against its offer.
#[must_use]
pub fn pruefe(
    angebot: &EndkundenAngebot,
    sha256: &str,
    malo_id: Option<&str>,
    lieferbeginn: Date,
    jetzt: OffsetDateTime,
) -> Option<Abweichung> {
    if angebot.content_sha256 != sha256 {
        return Some(Abweichung::Hash);
    }
    if jetzt > angebot.gueltig_bis {
        return Some(Abweichung::Abgelaufen);
    }
    if angebot.malo_id.as_deref() != malo_id {
        return Some(Abweichung::MaloId);
    }
    (lieferbeginn < angebot.lieferbeginn_fruehestens).then_some(Abweichung::LieferbeginnZuFrueh(
        angebot.lieferbeginn_fruehestens,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    fn angebot() -> EndkundenAngebot {
        EndkundenAngebot {
            id: Uuid::nil(),
            product_code: "STROM-OEKO-24".to_owned(),
            product_version: "v".to_owned(),
            sparte: "STROM".to_owned(),
            malo_id: Some("51238696012".to_owned()),
            nb_mp_id: "9900000000001".to_owned(),
            verbrauch_kwh: "3500".to_owned(),
            lieferbeginn_fruehestens: date!(2027 - 02 - 01),
            preise: Vec::new(),
            gueltig_bis: datetime!(2026-11-29 23:59:59 +01:00),
            content_sha256: "aa".to_owned(),
        }
    }

    const JETZT: OffsetDateTime = datetime!(2026-11-15 19:00 +01:00);

    #[test]
    fn a_matching_order_passes() {
        let a = angebot();
        assert_eq!(
            pruefe(&a, "aa", Some("51238696012"), date!(2027 - 02 - 01), JETZT),
            None
        );
    }

    #[test]
    fn an_order_for_another_malo_than_its_offer_is_refused() {
        let a = angebot();
        assert_eq!(
            pruefe(
                &a,
                "aa",
                Some("51238696012".replace('0', "1").as_str()),
                date!(2027 - 02 - 01),
                JETZT
            ),
            Some(Abweichung::MaloId)
        );
        assert_eq!(
            pruefe(&a, "aa", None, date!(2027 - 02 - 01), JETZT),
            Some(Abweichung::MaloId)
        );
    }

    #[test]
    fn an_expired_or_altered_offer_is_refused() {
        let a = angebot();
        assert_eq!(
            pruefe(&a, "bb", Some("51238696012"), date!(2027 - 02 - 01), JETZT),
            Some(Abweichung::Hash)
        );
        assert_eq!(
            pruefe(
                &a,
                "aa",
                Some("51238696012"),
                date!(2027 - 02 - 01),
                datetime!(2026-11-30 00:00 +01:00)
            ),
            Some(Abweichung::Abgelaufen)
        );
    }

    #[test]
    fn a_lieferbeginn_before_the_offers_earliest_names_it() {
        let a = angebot();
        assert_eq!(
            pruefe(&a, "aa", Some("51238696012"), date!(2027 - 01 - 31), JETZT),
            Some(Abweichung::LieferbeginnZuFrueh(date!(2027 - 02 - 01)))
        );
    }
}
