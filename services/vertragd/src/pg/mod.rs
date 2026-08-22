//! PostgreSQL data access for `vertragd`.
//!
//! | Module | Contents |
//! |---|---|
//! | [`kunden`] | Kunde, KundenIdentitaet, Person, Zahlungsinformation, GGV-Betreiber, BG-7 buyer projection |
//! | [`vertraege`] | Rahmenvertrag, Versorgungsvertrag, Vertragskomponente, Kündigung, Tarifwechsel |
//! | [`lifecycle`] | The queries the daily workers run: renewal, expiry, price-change notice, stuck dispatches |
//! | [`aggregator`] | § 41e EnWG Aggregatorverträge |
//! | [`gdpr`] | DSGVO Art. 15 export and Art. 17 pseudonymisation |

pub mod aggregator;
pub mod gdpr;
pub mod kunden;
pub mod lifecycle;
pub mod messstellenvertrag;
pub mod produkte;
pub mod vertraege;

use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

pub use aggregator::{
    AggregatorvertragRow, UpsertAggregatorvertragInput, find_active_aggregatorvertrag,
    list_aggregatorvertraege, upsert_aggregatorvertrag,
};
pub use gdpr::{GdprExportRow, anonymize_kunde, gdpr_export};
pub use kunden::{
    CreateKundeInput, KundeListRow, RechnungsempfaengerRow, UpdateKundeInput,
    UpsertIdentitaetInput, count_active_identitaeten, deactivate_identitaet_by_sub,
    fetch_identitaet_by_sub, fetch_kunde, fetch_kunde_by_sub, fetch_person,
    fetch_rechnungsempfaenger_by_ggv, fetch_rechnungsempfaenger_by_malo,
    fetch_rechnungsempfaenger_by_rahmenvertrag, fetch_zahlungsinformation, list_identitaeten,
    list_kunden, update_kunde, update_letzter_login, upsert_ggv_betreiber, upsert_identitaet,
    upsert_kunde, upsert_person, upsert_zahlungsinformation,
};
pub use lifecycle::{
    AutoRenewalRow, ExpiringVertragRow, StuckKomponenteRow, apply_auto_renewal,
    find_auto_renewal_due, find_auto_renewal_overdue, find_expiring_vertraege,
    find_stuck_komponents, mark_ablauf_notified, mark_auto_renewal_notified,
};
pub use messstellenvertrag::{
    MessstellenvertragRow, MessstellenvertragView, UpsertMessstellenvertragInput,
    find_messstellenvertrag, record_kuendigung, upsert_messstellenvertrag,
};
pub use produkte::{
    AnzupassenderPreis, MaloProduktSlice, ProduktSlice, malo_slices, offene_preisanpassungen,
};
pub use vertraege::{
    BillingCandidateRow, CreateKomponenteInput, CreateRahmenvertragInput,
    CreateVersorgungsvertragInput, InsertedKomponente, InsertedVertrag, KuendigungInput,
    KuendigungResult, PortfolioItemRow, RahmenvertragMaloRow, TarifwechselInput, close_due_supply,
    derive_vertrag_status, fetch_komponente, fetch_preisgarantie, fetch_rahmenvertrag,
    fetch_vertrag, fetch_vertrag_by_malo, insert_rahmenvertrag, insert_versorgungsvertrag,
    kuendige_vertrag, list_aktive_malo_ids, list_all_rahmenvertraege, list_billing_candidates,
    list_komponenten, list_offene_vertraege, list_pending_kuendigungen, list_portfolio_by_kunde,
    list_rahmenvertraege_by_kunde, list_rahmenvertrag_malos,
    list_versorgungsvertraege_by_rahmenvertrag, list_vertraege_by_kunde,
    mark_kuendigung_bestaetigt, storniere_vertrag, update_komponente_status, update_vertrag_status,
    upsert_preisgarantie, widerruf_kuendigung,
};

// ── Row types ─────────────────────────────────────────────────────────────────

/// A customer — the legal entity, never the portal user.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KundeRow {
    pub id: Uuid,
    pub tenant: String,
    pub kunden_nr: Option<String>,
    pub kundentyp: String,
    /// § 3 Nr. 57 EnWG — decides three statutory deadlines, see [`crate::domain`].
    pub haushaltskunde: bool,
    pub geschaeftspartner: Option<serde_json::Value>,
    pub organisations_id: Option<String>,
    pub umsatzsteuer_id: Option<String>,
    pub zahlungsziel_tage: i32,
    /// § 13b Abs. 2 Nr. 5 lit. b UStG reverse-charge master data.
    pub stromwiederverkaeufer: bool,
    pub sepa_erlaubt: bool,
    pub erp_kunde_id: Option<String>,
    pub notizen: Option<String>,
    pub created_at: time::OffsetDateTime,
}

/// One portal user (OIDC identity) for a Kunde.
/// B2C: 1:1 with Kunde.  B2B: 1:N — multiple users share one company account.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KundenIdentitaetRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub tenant: String,
    pub oidc_sub: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub rolle: String,
    pub standort_filter: Option<String>,
    pub aktiv: bool,
    pub letzter_login: Option<time::OffsetDateTime>,
    pub created_at: time::OffsetDateTime,
}

/// A B2B framework contract: shared terms for N Versorgungsverträge.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RahmenvertragRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub tenant: String,
    pub rahmenvertrag_nr: String,
    pub status: String,
    pub gueltig_von: Date,
    pub gueltig_bis: Option<Date>,
    pub kuendigungsfrist_monate: i32,
    pub auto_renewal: bool,
    pub renewal_monate: i32,
    pub preisanpassungsformel: Option<String>,
    pub portfolio_rabatt_prozent: Option<rust_decimal::Decimal>,
    pub angebot_id: Option<Uuid>,
    pub rechnungsstellung: String,
    pub sammelrechnung_intervall: Option<String>,
    pub erp_rahmenvertrag_id: Option<String>,
    pub notizen: Option<String>,
    pub created_at: time::OffsetDateTime,
}

/// A supply contract for one site.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VersorgungsvertragRow {
    pub id: Uuid,
    pub kunden_id: Uuid,
    pub rahmenvertrag_id: Option<Uuid>,
    pub tenant: String,
    pub vertrags_nr: String,
    /// GRUNDVERSORGUNG | ERSATZVERSORGUNG | SONDERVERTRAG — see
    /// [`crate::domain::Vertragsart`]; every notice period branches on it.
    pub vertragsart: String,
    pub status: String,
    pub vertragsbeginn: Date,
    /// `None` = unbefristet (the only lawful shape after a consumer contract's
    /// tacit extension, § 309 Nr. 9 lit. b BGB).
    pub vertragsende: Option<Date>,
    pub kundentyp: String,
    pub preisgarantie_bis: Option<Date>,
    pub kuendigungsfrist_monate: i32,
    /// §40b EnWG billing cadence: MONATLICH / VIERTELJAEHRLICH /
    /// HALBJAEHRLICH / JAEHRLICH.
    pub abrechnungszyklus: String,
    pub auto_renewal: bool,
    pub renewal_monate: i32,
    pub kuendigung_grund: Option<String>,
    pub kuendigung_eingang: Option<Date>,
    pub kuendigung_zum: Option<Date>,
    /// § 41 Abs. 8 Nr. 2 EnWG: when the Textform confirmation went out.
    pub kuendigungsbestaetigung_am: Option<time::OffsetDateTime>,
    pub bundle_code: Option<String>,
    pub standort_bezeichnung: Option<String>,
    pub standort_adresse: Option<serde_json::Value>,
    pub zahlungsziel_tage: Option<i32>,
    pub erp_contract_id: Option<String>,
    pub notizen: Option<String>,
    pub created_at: time::OffsetDateTime,
    pub completed_at: Option<time::OffsetDateTime>,
}

/// One supply position of a contract — a commodity at a market location.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VertragskomponenteRow {
    pub id: Uuid,
    pub vertrag_id: Uuid,
    pub tenant: String,
    pub sparte: String,
    pub malo_id: Option<String>,
    pub melo_id: Option<String>,
    pub lf_mp_id: String,
    pub nb_mp_id: Option<String>,
    pub lieferbeginn: Date,
    pub lieferende: Option<Date>,
    pub status: String,
    pub mako_process_id: Option<String>,
    pub abgelehnt_erc: Option<String>,
    pub abgelehnt_reason: Option<String>,
    pub ablese_auftrag_id: Option<Uuid>,
    pub fulfillment_data: Option<serde_json::Value>,
}

impl VertragskomponenteRow {
    /// Commodities whose supply start and end run through MaKo (GPKE /
    /// GeLi Gas). Everything else — HEMS, e-mobility, services — is fulfilled
    /// directly and never produces a UTILMD.
    #[must_use]
    pub fn requires_mako_workflow(sparte: &str) -> bool {
        matches!(
            sparte,
            "STROM"
                | "GAS"
                | "WAERME"
                | "SOLAR"
                | "EEG"
                | "EINSPEISUNG"
                | "WAERMEPUMPE"
                | "WALLBOX"
        )
    }

    /// `true` when this component's supply runs through MaKo.
    #[must_use]
    pub fn is_mako(&self) -> bool {
        Self::requires_mako_workflow(&self.sparte)
    }
}

// ── Shared idempotency helper ────────────────────────────────────────────────

/// Record an inbound CloudEvent; `false` when it was already seen.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn idempotent_event(
    executor: impl sqlx::PgExecutor<'_>,
    event_id: &str,
    event_type: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        "INSERT INTO received_events (event_id,event_type,payload)
         VALUES ($1,$2,$3) ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(event_id)
    .bind(event_type)
    .bind(payload)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(rows > 0)
}

/// A `Deserialize` helper for the many optional date fields on request bodies.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(transparent)]
pub struct IsoDate(pub Date);
