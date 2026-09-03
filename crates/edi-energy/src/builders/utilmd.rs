//! [`UtilmdBuilder`] — fluent type-safe builder for UTILMD messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::utilmd_codes::{
    AntwortStatus, IDE_VORGANG, Produktpaket, STS_STATUS_ANTWORT, STS_TRANSAKTIONSGRUND,
    Transaktionsgrund,
};
use crate::{Error, Lokationstyp, Pruefidentifikator, Release};

use super::{Set, Unset, bytes_to_segments};

// ── Inner fields structs ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct UtilmdTransactionSpec {
    /// `IDE` DE 7495 — [`IDE_VORGANG`] on every process message.
    ide_qualifier: String,
    /// `IDE` DE 7402 — the **Vorgangsnummer**, never a location ID.
    vorgangsnummer: String,
    process_dates: Vec<(String, String)>,
    transaktionsgrund: Option<Transaktionsgrund>,
    antwort: Option<AntwortStatus>,
    /// `SG4 STS+Z35` — the third market participant's answer, restated.
    antwort_dritter: Option<crate::utilmd_codes::DritterAntwortStatus>,
    free_texts: Vec<(String, String)>,
    agr: Option<(String, String)>,
    /// `SG5 LOC` — one entry per Lokation the Vorgang names.
    locations: Vec<(String, String)>,
    /// `SG8 SEQ+Z22` „Daten der Summenzeitreihe" with its `SG8 RFF+AUU`
    /// Version der Zeitreihe.
    ///
    /// The Clearinglisten head carries it: `MaBiS` versions a Summenzeitreihe by
    /// Erstellungszeitpunkt, and a list that does not say which version it
    /// reconciles cannot be matched to one (UTILMD AHB Strom 2.2 Kap. 13.4).
    summenzeitreihe_version: Option<String>,
    references: Vec<(String, String)>,
    /// `SG6 RFF+TN` — the Vorgangsnummer of the message being answered.
    referenz_vorgangsnummer: Option<String>,
    /// `SG6 RFF` with its `DTM`: (RFF qualifier, reference, DTM qualifier,
    /// value, format) — `RFF+Z18` with `DTM+Z20:2026:802`.
    dated_references: Vec<(String, String, String, String, String)>,
    /// `SG4 IMD` — (DE 7081 Produkt, DE 7009 Beschreibung).
    imd: Option<(String, String)>,
    /// `SG8 SEQ` blocks the typed fields do not cover, by SEQ code, in the
    /// order the MIG lists the places.
    stammdaten: Vec<Sg8Block>,
    /// `SG12 NAD` with an address.
    addressed_nads: Vec<Anschrift>,
    /// `SG8 SEQ+Z79` — the Produktpakete an Anmeldung and its Bestätigung
    /// carry. Muss on 55001, 55077, 55600, 55601, 55014 and 55608.
    produktpakete: Vec<Produktpaket>,
    /// `SG10 CCI` — Merkmale addressed by Klassentyp (DE 7059) with their value
    /// in DE 7037.
    merkmale: Vec<(String, String)>,
    /// `SG12 NAD` — the *beteiligte Marktpartner* a Vorgang names beside
    /// sender and receiver, `(DE 3035 qualifier, MP-ID)`.
    ///
    /// A list, not one entry: UTILMD AHB Strom 2.1/2.2 Bedingung [518] on PID
    /// 55036 reads „Es sind **alle** Altlieferanten anzugeben, an die eine
    /// Abmeldeanfrage gesendet wird" — Geschäftsvorfall 3 splits a Marktlokation
    /// across several Tranchen and so across several LFA.
    customer_nads: Vec<(String, String)>,
    /// `SG12 NAD` parties named by **name** rather than by MP-ID —
    /// `(DE 3035 qualifier, DE 3036 name parts, DE 3045 Namensformat)`.
    ///
    /// A different composite from [`Self::customer_nads`]: a Kunde des LF has
    /// no MP-ID, so it rides `C080` (element 4) with the Namensformat as that
    /// composite's sixth component, not `C082` (element 2). Writing a name into
    /// the party-identification slot states a Marktpartner code that does not
    /// exist.
    named_nads: Vec<(String, Vec<String>, String)>,
}

/// An `SG12 NAD` with an address: `NAD+Z04+++Name:::::Z01+Straße+Ort++PLZ+DE`.
#[derive(Debug, Clone)]
struct Anschrift {
    qualifier: String,
    name_parts: Vec<String>,
    name_format: String,
    strasse: String,
    ort: String,
    plz: String,
    land: String,
}

/// One `SG8 SEQ` with the `SG10 CCI`/`CAV`, `SG9 QTY` and `RFF` it carries.
#[derive(Debug, Clone, Default)]
pub struct Sg8Block {
    seq: String,
    items: Vec<Sg8Item>,
}

#[derive(Debug, Clone)]
enum Sg8Item {
    /// `CCI+<Klassentyp>++<Merkmal>`
    Cci { klassentyp: String, merkmal: String },
    /// `CAV+<Code>:::<Wert>`
    Cav { code: String, wert: String },
    /// `QTY+<Qualifier>:<Menge>:<Einheit>`
    Qty {
        qualifier: String,
        menge: String,
        einheit: String,
    },
    /// `RFF+<Qualifier>:<Referenz>`
    Rff { qualifier: String, referenz: String },
}

#[derive(Debug, Clone)]
struct UtilmdBuilderInner {
    release: Release,
    pruefidentifikator: Option<Pruefidentifikator>,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: Option<AgencyCode>,
    receiver_agency: Option<AgencyCode>,
    message_ref: String,
    document_code: String,
    /// `BGM` DE 1004; the message reference when not set.
    document_number: Option<String>,
    /// `DTM+157` Gültigkeit, Beginndatum — `CCYYMM`, DE 2379 `610`.
    ///
    /// The Bilanzierungsmonat a Clearingliste covers. A header date beside
    /// `DTM+137`, not a Vorgangs-date: the whole list is about one month.
    gueltigkeit_beginn: Option<String>,
    document_date: Option<String>,
    rff_entries: Vec<(String, String)>,
    transactions: Vec<UtilmdTransactionSpec>,
}

// ── UtilmdBuilder ─────────────────────────────────────────────────────────────

/// Fluent builder for `UTILMD` (Utilities Master Data) messages.
///
/// # Type-state
///
/// [`build`](UtilmdBuilder::build) is only available once both
/// [`sender`](UtilmdBuilder::sender) and [`receiver`](UtilmdBuilder::receiver)
/// have been called. The compiler enforces this at the call site.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::{Release, Pruefidentifikator};
/// use edi_energy::builders::UtilmdBuilder;
///
/// let msg = UtilmdBuilder::new(Release::new("5.5.3a"))
///     .pruefidentifikator(Pruefidentifikator::new(55001).unwrap())
///     .sender("9900987654321")
///     .receiver("9900123456789")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("9900987654321"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct UtilmdBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: UtilmdBuilderInner,
}

impl UtilmdBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: UtilmdBuilderInner {
                release,
                pruefidentifikator: None,
                sender_id: None,
                receiver_id: None,
                sender_agency: None,
                receiver_agency: None,
                message_ref: "1".to_owned(),
                document_code: "E01".to_owned(),
                document_number: None,
                gueltigkeit_beginn: None,
                document_date: None,
                rff_entries: Vec::new(),
                transactions: Vec::new(),
            },
        }
    }
}

impl<S, R> UtilmdBuilder<S, R> {
    fn transition<S2, R2>(self) -> UtilmdBuilder<S2, R2> {
        UtilmdBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier (DE 3039).
    pub fn sender(mut self, id: impl Into<String>) -> UtilmdBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier (DE 3039).
    pub fn receiver(mut self, id: impl Into<String>) -> UtilmdBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Override the agency code for the sender's party identifier.
    ///
    /// Leave unset and the agency is derived from the MP-ID itself —
    /// [`AgencyCode::for_mp_id`]: `99…` → BDEW `293`, `98…` → DVGW `332`, any
    /// other 13-digit code → GS1 `9`. Override only for a party whose
    /// registered code list differs from what its number implies.
    pub fn sender_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.sender_agency = Some(agency);
        self
    }

    /// Override the agency code for the receiver's party identifier.
    ///
    /// Derived from the MP-ID when unset — see
    /// [`sender_agency`](Self::sender_agency).
    pub fn receiver_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.receiver_agency = Some(agency);
        self
    }

    /// Set the Pruefidentifikator (process-variant code, e.g. `55001`).
    pub fn pruefidentifikator(mut self, pid: Pruefidentifikator) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Override the message reference number (UNH / DE 0062).  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Override the BGM document name code (DE 1001).  Defaults to `"E01"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
        self
    }

    /// Set the `BGM` Dokumentennummer (DE 1004). Defaults to the message
    /// reference, which is unique per message already.
    pub fn document_number(mut self, number: impl Into<String>) -> Self {
        self.inner.document_number = Some(number.into());
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Add a reference segment (RFF, SG1) to the message header.
    ///
    /// `qualifier` is the DE 1153 reference qualifier (e.g. `"ACE"`, `"Z13"`).
    /// `reference` is the reference identifier (DE 1154).
    ///
    /// UTILMD MIG 5.5.3a requires at least one `RFF` in SG1 (max 99).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// let msg = UtilmdBuilder::new(Release::new("5.5.3a"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55001).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .rff("ACE", "20230701")
    ///     .build()?;
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn rff(mut self, qualifier: impl Into<String>, reference: impl Into<String>) -> Self {
        self.inner
            .rff_entries
            .push((qualifier.into(), reference.into()));
        self
    }

    /// Start configuring a Vorgang (SG4 / IDE block).
    ///
    /// `vorgangsnummer` is `IDE` DE 7402 — the sender's own reference for this
    /// transaction, unique across `IDE+24` **and** `IDE+Z01`. It is *not* a
    /// location ID: the Marktlokation goes into `SG5 LOC+Z16` via
    /// [`marktlokation`](UtilmdTransactionBuilder::marktlokation).
    ///
    /// Returns a [`UtilmdTransactionBuilder`] sub-builder. Call
    /// [`done`](UtilmdTransactionBuilder::done) to finalize and return.
    pub fn transaction(self, vorgangsnummer: impl Into<String>) -> UtilmdTransactionBuilder<S, R> {
        self.transaction_with_qualifier(IDE_VORGANG, vorgangsnummer)
    }

    /// Set `DTM+157` Gültigkeit, Beginndatum — the Bilanzierungsmonat, `CCYYMM`.
    ///
    /// DE 2379 is `610` here, the only place UTILMD uses a month granularity:
    /// a Clearingliste is about one Bilanzierungsmonat and carries no day.
    pub fn gueltigkeit_beginn(mut self, ccyymm: impl Into<String>) -> Self {
        self.inner.gueltigkeit_beginn = Some(ccyymm.into());
        self
    }

    /// Start an `IDE+Z01` list block (`MaBiS` Summenzeitreihen).
    ///
    /// UTILMD DE 7495 has exactly two values; this is the other one. Every
    /// `IDE+24` that follows is a member of the list rather than a
    /// Geschäftsvorfall of its own („Das IDE+Z01 (Liste) definiert den
    /// Geschäftsvorfall. Alle aufgelisteten IDE+24 sind Bestandteil des
    /// Geschäftsvorfalls" — UTILMD AHB Strom 2.2 Bedingung `[564]`), which is
    /// why only the head carries `SG6 RFF+Z13`.
    pub fn list_transaction(self, list_id: impl Into<String>) -> UtilmdTransactionBuilder<S, R> {
        self.transaction_with_qualifier(crate::utilmd_codes::IDE_LISTE, list_id)
    }

    /// Start a Vorgang with an explicit DE 7495 qualifier.
    ///
    /// Reserved for round-tripping messages from counterparties that use a
    /// qualifier outside the MIG's `24` / `Z01` pair. New code should call
    /// [`transaction`](Self::transaction).
    pub fn transaction_with_qualifier(
        self,
        ide_qualifier: impl Into<String>,
        vorgangsnummer: impl Into<String>,
    ) -> UtilmdTransactionBuilder<S, R> {
        UtilmdTransactionBuilder {
            parent: self,
            spec: UtilmdTransactionSpec {
                ide_qualifier: ide_qualifier.into(),
                vorgangsnummer: vorgangsnummer.into(),
                ..Default::default()
            },
        }
    }
}

/// Emit the `SG6` references of one Vorgang, in AHB order.
///
/// `RFF+Z13` carries the **Prüfidentifikator** — DE 1154 format `R n5`, „genau
/// einmal je SG4 IDE (Vorgang) anzugeben". It belongs here and not in `BGM`
/// DE 1004, which every row of UTILMD AHB Strom 2.2 and Gas 1.2 names the
/// *Dokumentennummer*.
///
/// On a **Listennachricht** the Vorgang is the `IDE+Z01` head — „Alle
/// aufgelisteten IDE+24 sind Bestandteil des Geschäftsvorfalls" (Bedingung
/// `[564]`) — so only the head carries it and each `IDE+24` position carries
/// `RFF+TN` instead. `carries_pid` is that distinction.
///
/// `RFF+TN` carries „Referenz Vorgangsnummer (aus Anfragenachricht)", Muss on
/// every Antwortnachricht. It is what ties an answer to its request, because
/// `IDE+24` DE 7402 must be a fresh number: the MIG's „Hinweis zu DE7402" makes
/// a Vorgangsnummer unusable once it has been sent.
fn emit_sg6<W: std::io::Write>(
    w: &mut Writer<W>,
    pid_str: &str,
    tx: &UtilmdTransactionSpec,
    carries_pid: bool,
) -> Result<(), Error> {
    if carries_pid && !pid_str.is_empty() {
        emit_comp!(w, "RFF", ["Z13", pid_str]);
    }
    if let Some(referenz) = &tx.referenz_vorgangsnummer {
        emit_comp!(w, "RFF", ["TN", referenz]);
    }
    for (rff_q, rff_ref) in &tx.references {
        emit_comp!(w, "RFF", [rff_q, rff_ref]);
    }
    for (rff_q, rff_ref, dtm_q, value, format) in &tx.dated_references {
        emit_comp!(w, "RFF", [rff_q, rff_ref]);
        emit_comp!(w, "DTM", [dtm_q, value, format]);
    }
    Ok(())
}

/// Emit one `SG8` block.
fn emit_sg8_block<W: std::io::Write>(w: &mut Writer<W>, block: &Sg8Block) -> Result<(), Error> {
    emit_seg!(w, "SEQ", &block.seq);
    for item in &block.items {
        match item {
            // `CCI+<7059>++<7037>` — C502 is nicht benutzt and still occupies
            // element 2.
            Sg8Item::Cci {
                klassentyp,
                merkmal,
            } => emit_seg!(w, "CCI", klassentyp, "", merkmal),
            Sg8Item::Cav { code, wert } if wert.is_empty() => emit_seg!(w, "CAV", code),
            Sg8Item::Cav { code, wert } => emit_comp!(w, "CAV", [code, "", "", wert]),
            Sg8Item::Qty {
                qualifier,
                menge,
                einheit,
            } if einheit.is_empty() => {
                emit_comp!(w, "QTY", [qualifier, menge]);
            }
            Sg8Item::Qty {
                qualifier,
                menge,
                einheit,
            } => emit_comp!(w, "QTY", [qualifier, menge, einheit]),
            Sg8Item::Rff {
                qualifier,
                referenz,
            } => emit_comp!(w, "RFF", [qualifier, referenz]),
        }
    }
    Ok(())
}

/// The rank of every `SEQ` code in the MIG's list of `SG8` places, so blocks
/// go out in the order the Nachrichtenstruktur gives them; unknown releases
/// keep insertion order.
fn sg8_ranks(release: &Release) -> std::collections::HashMap<String, usize> {
    let mut ranks = std::collections::HashMap::new();
    let Some(profile) = crate::ReleaseRegistry::global()
        .profiles_for(crate::MessageType::Utilmd)
        .find(|p| p.release() == release)
    else {
        return ranks;
    };
    for (rank, layout) in profile
        .structure
        .layouts
        .iter()
        .filter(|l| l.tag == "SEQ")
        .enumerate()
    {
        if let Some(el) = layout.elements.first() {
            for code in &el.codes {
                ranks.entry(code.code.clone()).or_insert(rank);
            }
        }
    }
    ranks
}

/// Emit the `SG8` / `SG10` Produktpakete of one Vorgang.
///
/// ```text
/// SEQ+Z79+1
/// PIA+5+9991000002082:Z11
/// CCI+Z66
/// CAV+ZV4:::11XBK-EEG-----1
/// ```
///
/// The Anmeldung einer Zuordnung des LFN is not complete without one: the AHB
/// marks `SG8 SEQ+Z79` Muss on 55001, 55077, 55600, 55601, 55014 and 55608, and
/// the Codeliste der Konfigurationen 1.4 Kap. 6.1.1 makes the Bilanzkreis
/// product unconditional inside it („zwingend anzugeben").
///
/// `CAV+ZH9` is conditional (Bedingung `[36]`): it appears only where the
/// Codeliste gives the product a Code der Produkteigenschaft. The Bilanzkreis
/// has none, so its package is `CCI+Z66` followed by `CAV+ZV4` alone.
fn emit_sg8_produktpakete<W: std::io::Write>(
    w: &mut Writer<W>,
    tx: &UtilmdTransactionSpec,
) -> Result<(), Error> {
    use crate::utilmd_codes::produkt;

    // `SG8 SEQ+Z22` „Daten der Summenzeitreihe" with `SG8 RFF+AUU` — the
    // Clearinglisten head. Emitted before the Produktpakete because the AHB
    // numbers it 00015/00016, ahead of the SEQ+Z79 block.
    if let Some(version) = &tx.summenzeitreihe_version {
        emit_seg!(w, "SEQ", crate::utilmd_codes::SEQ_SUMMENZEITREIHE);
        emit_comp!(w, "RFF", [crate::utilmd_codes::RFF_ZEITREIHE, version]);
    }
    for paket in &tx.produktpakete {
        emit_seg!(
            w,
            "SEQ",
            produkt::SEQ_PRODUKTPAKET,
            &paket.paket_id.to_string()
        );
        for p in &paket.produkte {
            emit_comp!(
                w,
                "PIA",
                [produkt::PIA_ERFORDERLICHES_PRODUKT],
                [&p.produkt_code, produkt::PIA_TYP_PRODUKT]
            );
            emit_seg!(w, "CCI", produkt::CCI_PRODUKTEIGENSCHAFT);
            if let Some(eigenschaft) = &p.eigenschaft {
                emit_comp!(w, "CAV", [produkt::CAV_EIGENSCHAFT, "", "", eigenschaft]);
            }
            if let Some(wert) = &p.wert {
                emit_comp!(w, "CAV", [produkt::CAV_WERT, "", "", wert]);
            }
        }
    }
    // `SG8 SEQ+ZH0` — „so oft zu wiederholen, wie es Produktpaket-ID in einem
    // Geschäftsvorfall gibt" (AHB Kap. 5.3). The group is Muss wherever
    // `SEQ+Z79` is, so it follows every package block rather than being
    // optional: `CCI+Z65` DE 4051 tells the NB whether it may assign the LF on
    // a partial application of the package.
    //
    // The `CAV` Priorisierung (`Z75`…`Z79`) is Bedingung [42] — „wenn mehr als
    // ein SG8 SEQ+ZH0 vorhanden" — so a single package carries none.
    for (idx, paket) in tx.produktpakete.iter().enumerate() {
        emit_seg!(
            w,
            "SEQ",
            produkt::SEQ_PRIORISIERUNG,
            &paket.paket_id.to_string()
        );
        emit_seg!(
            w,
            "CCI",
            produkt::CCI_UMSETZUNGSGRAD,
            "",
            "",
            paket.umsetzung.code()
        );
        if tx.produktpakete.len() > 1
            && let Some(prio) = produkt::PRIORITAET.get(idx)
        {
            emit_seg!(w, "CAV", prio);
        }
    }
    Ok(())
}

/// Emit one `SG4 IDE` Vorgang and everything nested under it.
///
/// MIG Zähler order inside SG4: IDE (0190), DTM (0230), STS (0250), FTX (0280),
/// AGR (0290), then SG5 LOC (0330), SG6 RFF (0360), SG8/SG10 (Produktpakete)
/// and SG12 NAD. Layer 3.5 checks it, on both sides of the wire.
fn emit_sg4<W: std::io::Write>(
    w: &mut Writer<W>,
    pid_str: &str,
    tx: &UtilmdTransactionSpec,
    carries_pid: bool,
    ranks: &std::collections::HashMap<String, usize>,
) -> Result<(), Error> {
    // MIG Zähler order inside SG4: IDE (0190), DTM (0230), STS (0250),
    // FTX (0280), AGR (0290), then SG5 LOC (0330), SG6 RFF (0360) and
    // SG12 NAD. Layer 3.5 checks it, on both sides of the wire.
    emit_seg!(w, "IDE", &tx.ide_qualifier, &tx.vorgangsnummer);
    if let Some((produkt, beschreibung)) = &tx.imd {
        emit_comp!(w, "IMD", [""], [produkt], [beschreibung]);
    }
    for (qualifier, date_val) in &tx.process_dates {
        let fmt = sg4_dtm_format(qualifier);
        let value = if fmt == "303" {
            super::ccyymmddhhmm_utc(date_val)
        } else {
            date_val.clone()
        };
        emit_comp!(w, "DTM", [qualifier, &value, fmt]);
    }
    if let Some(grund) = &tx.transaktionsgrund {
        // `STS+7++<grund>+<ergaenzung>+<befristet>` — Statuskategorie 7
        // in C601, then one repeated C556 per code. C555 sits between
        // C601 and the first C556 and is *nicht benutzt*, so it is
        // written empty rather than omitted. MIG example:
        // `STS+7++E01+ZW4+E03'`.
        let ergaenzung = grund.ergaenzung.as_deref().unwrap_or("");
        match grund.befristet.as_deref() {
            Some(befristet) => emit_seg!(
                w,
                "STS",
                STS_TRANSAKTIONSGRUND,
                "",
                &grund.grund,
                ergaenzung,
                befristet
            ),
            None if !ergaenzung.is_empty() => {
                emit_seg!(
                    w,
                    "STS",
                    STS_TRANSAKTIONSGRUND,
                    "",
                    &grund.grund,
                    ergaenzung
                );
            }
            None => emit_seg!(w, "STS", STS_TRANSAKTIONSGRUND, "", &grund.grund),
        }
    }
    if let Some(antwort) = &tx.antwort {
        // `STS+E01++<code>:<codeliste>` — the Prüfschritt code in C556
        // DE 9013 and the Codeliste it comes from in DE 1131. The AHB
        // marks this Muss on every Bestätigung and Ablehnung and
        // constrains the code to that list's Zustimmungs- or
        // Ablehnungs-Cluster.
        if let Some(cl) = antwort.codeliste.as_deref() {
            emit_comp!(w, "STS", [STS_STATUS_ANTWORT], [""], [&antwort.code, cl]);
        } else {
            emit_seg!(w, "STS", STS_STATUS_ANTWORT, "", &antwort.code);
        }
    }
    // `SG4 STS+Z35` — „Status der Antwort des dritten Marktbeteiligten", the
    // second STS of an Ablehnung whose ground is the LFA's Widerspruch.
    //
    // `STS+Z35++<code>:E_0624'` on a 55003; the erzeugende form additionally
    // fills `C555` with the MaLo-ID the restated answer is about and a second
    // `C556` with `ZW3`/`ZW5`, because Geschäftsvorfall 3 has several LFA and
    // the LFN would otherwise not know whose refusal it is being told about.
    if let Some(dritter) = &tx.antwort_dritter {
        let referenz = dritter.referenz_lokation.as_deref().unwrap_or("");
        if let Some(objekt) = dritter.objekt.as_deref() {
            emit_comp!(
                w,
                "STS",
                [crate::utilmd_codes::STS_ANTWORT_DRITTER],
                [referenz],
                [&dritter.code, &dritter.codeliste],
                [objekt]
            );
        } else {
            emit_comp!(
                w,
                "STS",
                [crate::utilmd_codes::STS_ANTWORT_DRITTER],
                [referenz],
                [&dritter.code, &dritter.codeliste]
            );
        }
    }
    for (ftx_q, ftx_text) in &tx.free_texts {
        emit_comp!(w, "FTX", [ftx_q], [""], [""], [ftx_text]);
    }
    if let Some((svc_req, resp_type)) = &tx.agr {
        emit_comp!(w, "AGR", [svc_req, resp_type]);
    }
    for (loc_q, loc_id) in &tx.locations {
        emit_comp!(w, "LOC", [loc_q], [loc_id]);
    }
    emit_sg6(w, pid_str, tx, carries_pid)?;
    emit_sg8(w, tx, ranks)?;
    emit_sg12(w, tx)?;
    Ok(())
}

/// The `SG12 NAD`s of the Vorgang: named parties, addressed parties, and the
/// parties identified by MP-ID.
fn emit_sg12<W: std::io::Write>(
    w: &mut Writer<W>,
    tx: &UtilmdTransactionSpec,
) -> Result<(), Error> {
    for (nad_q, parts, format) in &tx.named_nads {
        // `NAD+<3035>++++<3036>:<3036>:…::<3045>` — C080 sits at element 4 and
        // repeats DE 3036 five times before DE 3045. C082 and C058 stay empty:
        // the party is identified by its name, not by a Marktpartner code.
        let mut c080: Vec<&str> = parts.iter().map(String::as_str).take(5).collect();
        c080.resize(5, "");
        c080.push(format.as_str());
        // `emit_comp!` takes fixed-arity composites; C080 is built at runtime,
        // so this writes the element slices directly.
        w.write_composites(
            "NAD",
            &[&[nad_q.as_str()][..], &[""][..], &[""][..], &c080[..]],
        )
        .map_err(Error::Parse)?;
    }
    for a in &tx.addressed_nads {
        let mut c080: Vec<&str> = a.name_parts.iter().map(String::as_str).take(5).collect();
        c080.resize(5, "");
        c080.push(a.name_format.as_str());
        w.write_composites(
            "NAD",
            &[
                &[a.qualifier.as_str()][..],
                &[""][..],
                &[""][..],
                &c080[..],
                &[a.strasse.as_str()][..],
                &[a.ort.as_str()][..],
                &[""][..],
                &[a.plz.as_str()][..],
                &[a.land.as_str()][..],
            ],
        )
        .map_err(Error::Parse)?;
    }
    for (nad_q, nad_id) in &tx.customer_nads {
        // SG12 NAD names a *third* party (the Altlieferant on a 55036, the
        // auslösender Marktpartner on a 55038), so its DE 3055 follows that
        // party's own MP-ID — not the sender's and not a fixed `293`.
        emit_comp!(
            w,
            "NAD",
            [nad_q],
            [nad_id, "", super::agency_for(None, nad_id)]
        );
    }
    Ok(())
}

/// Every `SG8` of the Vorgang — Produktpakete, Summenzeitreihe, Merkmale and
/// the generic blocks — in the MIG's order of `SEQ` places.
fn emit_sg8<W: std::io::Write>(
    w: &mut Writer<W>,
    tx: &UtilmdTransactionSpec,
    ranks: &std::collections::HashMap<String, usize>,
) -> Result<(), Error> {
    use crate::utilmd_codes::produkt;
    let rank = |code: &str| ranks.get(code).copied().unwrap_or(usize::MAX);
    // (rank, insertion order, block)
    let mut blocks: Vec<(usize, usize, Sg8Block)> = Vec::new();
    let mut z01 = Sg8Block {
        seq: crate::utilmd_codes::SEQ_DATEN_DER_MARKTLOKATION.to_owned(),
        items: Vec::new(),
    };
    for (klassentyp, wert) in &tx.merkmale {
        z01.items.push(Sg8Item::Cci {
            klassentyp: klassentyp.clone(),
            merkmal: wert.clone(),
        });
    }
    for block in &tx.stammdaten {
        if block.seq == z01.seq {
            z01.items.extend(block.items.iter().cloned());
        } else {
            blocks.push((rank(&block.seq), blocks.len(), block.clone()));
        }
    }
    if !z01.items.is_empty() {
        blocks.push((rank(&z01.seq), blocks.len(), z01));
    }
    let produkt_rank =
        rank(produkt::SEQ_PRODUKTPAKET).min(rank(crate::utilmd_codes::SEQ_SUMMENZEITREIHE));
    blocks.sort_by_key(|(r, i, _)| (*r, *i));
    let mut produkte_done = false;
    for (r, _, block) in &blocks {
        if !produkte_done && *r >= produkt_rank {
            emit_sg8_produktpakete(w, tx)?;
            produkte_done = true;
        }
        emit_sg8_block(w, block)?;
    }
    if !produkte_done {
        emit_sg8_produktpakete(w, tx)?;
    }
    Ok(())
}

impl<S, R> UtilmdBuilder<S, R> {
    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let pid_str = self
            .inner
            .pruefidentifikator
            .map(|p| format!("{:05}", p.as_u32()))
            .unwrap_or_default();
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(super::now_ccyymmddhhmm, str::to_owned);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["UTILMD", "D", "11A", "UN", self.inner.release.as_str()]
        );
        // `BGM` DE 1004 is the Dokumentennummer (UTILMD MIG S2.2 example
        // `BGM+E01+MKIDI5422'`), and the MIG defines no DE 1225 for UTILMD.
        let document_number = self
            .inner
            .document_number
            .as_deref()
            .unwrap_or(&self.inner.message_ref);
        emit_seg!(w, "BGM", &self.inner.document_code, document_number);
        // `DTM+137` Dokumentendatum. Every EDI@Energy AHB gives DE 2379 as
        // `303` (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to
        // `+00`; `[494]` requires the stamp to be the creation moment or
        // earlier. There is no Anwendungsfall in any AHB that takes `102`.
        emit_comp!(w, "DTM", ["137", &super::ccyymmddhhmm_utc(&dtm_val), "303"]);
        // `DTM+157` Gültigkeit, Beginndatum in `610` `CCYYMM` — the
        // Bilanzierungsmonat a Clearingliste covers.
        if let Some(monat) = &self.inner.gueltigkeit_beginn {
            emit_comp!(w, "DTM", ["157", monat, "610"]);
        }
        for (qualifier, reference) in &self.inner.rff_entries {
            emit_comp!(w, "RFF", [qualifier, reference]);
        }
        if let Some(id) = &self.inner.sender_id {
            emit_comp!(
                w,
                "NAD",
                ["MS"],
                [id, "", super::agency_for(self.inner.sender_agency, id)]
            );
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_comp!(
                w,
                "NAD",
                ["MR"],
                [id, "", super::agency_for(self.inner.receiver_agency, id)]
            );
        }
        // On a Listennachricht the `IDE+Z01` head is the Geschäftsvorfall and
        // carries `SG6 RFF+Z13`; its `IDE+24` members do not. A message without
        // a head is a single Vorgang and carries it there.
        let ist_liste = self
            .inner
            .transactions
            .iter()
            .any(|tx| tx.ide_qualifier == crate::utilmd_codes::IDE_LISTE);
        let ranks = sg8_ranks(&self.inner.release);
        for tx in &self.inner.transactions {
            let carries_pid = !ist_liste || tx.ide_qualifier == crate::utilmd_codes::IDE_LISTE;
            emit_sg4(&mut w, &pid_str, tx, carries_pid, &ranks)?;
        }
        w.finish_unt(&self.inner.message_ref)
            .map_err(Error::Parse)?;
        Ok(buf)
    }
    /// Build and serialize the message to EDIFACT bytes.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if serialization fails.
    pub fn serialize(self) -> Result<Vec<u8>, Error> {
        self.to_bytes()
    }
}

impl UtilmdBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::utilmd::UtilmdMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::utilmd::UtilmdMessage, Error> {
        let pid = self
            .inner
            .pruefidentifikator
            .map(super::super::pruefidentifikator::Pruefidentifikator::as_u32);
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::utilmd::UtilmdMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}

// ── UtilmdTransactionBuilder ──────────────────────────────────────────────────

/// Sub-builder for a transaction (SG4 / IDE block) in a UTILMD message.
///
/// Obtained via [`UtilmdBuilder::transaction`]. Call
/// [`done`](UtilmdTransactionBuilder::done) to finalize and return to the
/// parent builder.
#[derive(Debug)]
#[must_use = "Sub-builder must be finalized with .done()"]
pub struct UtilmdTransactionBuilder<S = Unset, R = Unset> {
    parent: UtilmdBuilder<S, R>,
    spec: UtilmdTransactionSpec,
}

impl<S, R> UtilmdTransactionBuilder<S, R> {
    /// Set the SG4 **Transaktionsgrund** (`STS+7`, MIG Nr. 00033).
    ///
    /// Takes the whole [`Transaktionsgrund`] rather than a bare code because
    /// the AHB marks the *Ergänzung* Muss alongside the Grund on the GPKE and
    /// `GeLi` Gas core processes: `ZW3`/`ZW4`/`ZW5`/`ZAP` is what tells the
    /// receiver whether the Vorgang is about a verbrauchende or erzeugende
    /// Marktlokation, a Tranche or a ruhende Marktlokation — and therefore
    /// which branch of the answering EBD applies.
    ///
    /// ```rust
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// # use edi_energy::utilmd_codes::{Transaktionsgrund, transaktionsgrund, dtm, loc};
    /// let edi = UtilmdBuilder::new(Release::new("S2.2"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55001).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .transaction("VORGANG-0001")
    ///     .date(dtm::BEGINN_ZUM, "20261101")
    ///     .transaktionsgrund(Transaktionsgrund::verbrauchende_malo(transaktionsgrund::WECHSEL))
    ///     .marktlokation("51238696012")
    ///     .done()
    ///     .serialize()?;
    /// let text = String::from_utf8(edi).unwrap();
    /// assert!(text.contains("IDE+24+VORGANG-0001"));
    /// assert!(text.contains("DTM+92:202611010000?+00:303"));
    /// assert!(text.contains("STS+7++E03+ZW4"));
    /// assert!(text.contains("LOC+Z16+51238696012"));
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn transaktionsgrund(mut self, grund: Transaktionsgrund) -> Self {
        self.spec.transaktionsgrund = Some(grund);
        self
    }

    /// Set the SG4 **Status der Antwort** (`STS+E01`, MIG Nr. 00034).
    ///
    /// Emits `STS+E01++<code>:<codeliste>`. Every Bestätigung and Ablehnung
    /// needs one — the AHB marks the segment Muss and restricts the code to the
    /// named Codeliste's Zustimmungs- or Ablehnungs-Cluster.
    ///
    /// DE 1131 is the **Codeliste**, which is the EBD number only where the AHB
    /// says „EBD-Nummer". Every `WiM` MSB-Wechsel answer names an `S_00xx` (Strom) or
    /// `G_00xx` (Gas) list instead — see [`AntwortStatus::codeliste`].
    ///
    /// ```rust
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// # use edi_energy::utilmd_codes::AntwortStatus;
    /// let edi = UtilmdBuilder::new(Release::new("S2.2"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55011).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .transaction("VORGANG-0001")
    ///     .antwort(AntwortStatus::from_codeliste("A36", "E_0624"))
    ///     .done()
    ///     .serialize()?;
    /// assert!(String::from_utf8(edi).unwrap().contains("STS+E01++A36:E_0624"));
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn antwort(mut self, antwort: AntwortStatus) -> Self {
        self.spec.antwort = Some(antwort);
        self
    }

    /// Add a SG4 process-date segment.
    ///
    /// `qualifier` is DE 2005 — use the [`dtm`](crate::utilmd_codes::dtm)
    /// constants (`92` Beginn zum, `93` Ende zum, `154` ÜT der
    /// Lieferanmeldung, …). `value` is `CCYYMMDD`, or `CCYYMMDDHHMM+00` when
    /// the process needs the UTC instant; the DE 2379 format code follows.
    pub fn date(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec
            .process_dates
            .push((qualifier.into(), value.into()));
        self
    }

    /// Add a `SG5 LOC` location segment.
    ///
    /// Prefer [`marktlokation`](Self::marktlokation) /
    /// [`messlokation`](Self::messlokation); this is the escape hatch for the
    /// rarer Lokationstypen.
    pub fn location(mut self, lokationstyp: Lokationstyp, id: impl Into<String>) -> Self {
        self.spec
            .locations
            .push((lokationstyp.qualifier_code().to_owned(), id.into()));
        self
    }

    /// Add `SG5 LOC+Z16` — the Marktlokation this Vorgang is about.
    pub fn marktlokation(self, malo_id: impl Into<String>) -> Self {
        self.location(Lokationstyp::Marktlokation, malo_id)
    }

    /// Add `SG5 LOC+Z17` — the Messlokation this Vorgang is about.
    pub fn messlokation(self, melo_id: impl Into<String>) -> Self {
        self.location(Lokationstyp::Messlokation, melo_id)
    }

    /// Add a SG6/RFF reference segment.
    /// Set `SG8 SEQ+Z22` „Daten der Summenzeitreihe" with its `SG8 RFF+AUU`
    /// Version der Zeitreihe.
    ///
    /// Muss on both Clearinglisten (UTILMD AHB Strom 2.2 Kap. 13.4). `MaBiS`
    /// versions a Summenzeitreihe by Erstellungszeitpunkt, so a list that names
    /// no version cannot be matched to the one it reconciles.
    pub fn summenzeitreihe_version(mut self, version: impl Into<String>) -> Self {
        self.spec.summenzeitreihe_version = Some(version.into());
        self
    }

    /// Set `SG6 RFF+TN` — „Referenz Vorgangsnummer (aus Anfragenachricht)".
    ///
    /// Pass the **request's** `IDE+24` DE 7402. The AHB marks the segment Muss
    /// on every Antwortnachricht, and it is the only correlation the answer
    /// carries: DE 7402 must be globally unique across every `IDE+24` and
    /// `IDE+Z01` ever sent (MIG S2.2, Hinweis zu DE7402), so an answer may not
    /// echo the request's number as its own.
    ///
    /// ```rust
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// # use edi_energy::utilmd_codes::dtm;
    /// let edi = UtilmdBuilder::new(Release::new("S2.2"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55017).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .transaction("ANTWORT-0001")
    ///     .date(dtm::ENDE_ZUM, "20261101")
    ///     .referenz_vorgangsnummer("NNV1234")
    ///     .marktlokation("51238696012")
    ///     .done()
    ///     .serialize()?;
    /// let text = String::from_utf8(edi).unwrap();
    /// assert!(text.contains("IDE+24+ANTWORT-0001"));
    /// assert!(text.contains("RFF+Z13:55017"));
    /// assert!(text.contains("RFF+TN:NNV1234"));
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn referenz_vorgangsnummer(mut self, vorgangsnummer: impl Into<String>) -> Self {
        self.spec.referenz_vorgangsnummer = Some(vorgangsnummer.into());
        self
    }

    /// Add a `SG10 CCI` Merkmal — Klassentyp in DE 7059, value in DE 7037.
    ///
    /// Emits `CCI+<klassentyp>++<wert>`. `GeLi` Gas carries the **Bilanzkreis**
    /// this way (`CCI+Z19`, Muss on 44001), where GPKE Strom uses the
    /// Produktpaket — see [`Self::produktpaket`]. The two Festlegungen model the
    /// same fact differently, so neither shape may be sent on the other's
    /// Sparte.
    pub fn merkmal(mut self, klassentyp: impl Into<String>, wert: impl Into<String>) -> Self {
        self.spec.merkmale.push((klassentyp.into(), wert.into()));
        self
    }

    /// `SG4 IMD` — Produkt (DE 7081) and Beschreibung (DE 7009):
    /// `IMD++Z36+Z12` on a Gas Anmeldung.
    pub fn imd(mut self, produkt: impl Into<String>, beschreibung: impl Into<String>) -> Self {
        self.spec.imd = Some((produkt.into(), beschreibung.into()));
        self
    }

    /// `SG6 RFF` with the `DTM` of its group: `RFF+Z18'DTM+Z20:2026:802'`.
    pub fn reference_dated(
        mut self,
        qualifier: impl Into<String>,
        referenz: impl Into<String>,
        dtm_qualifier: impl Into<String>,
        value: impl Into<String>,
        format: impl Into<String>,
    ) -> Self {
        self.spec.dated_references.push((
            qualifier.into(),
            referenz.into(),
            dtm_qualifier.into(),
            value.into(),
            format.into(),
        ));
        self
    }

    /// An `SG8 SEQ` block of Stammdaten the typed fields do not cover; the
    /// blocks go out in the MIG's order of `SEQ` places.
    pub fn stammdaten(self, seq: impl Into<String>) -> Sg8Builder<S, R> {
        Sg8Builder {
            parent: self,
            block: Sg8Block {
                seq: seq.into(),
                items: Vec::new(),
            },
        }
    }

    /// `SG12 NAD` with an address: `NAD+Z04+++Name:::::Z01+Straße+Ort++PLZ+DE`.
    #[allow(clippy::too_many_arguments)]
    pub fn anschrift(
        mut self,
        party_qualifier: impl Into<String>,
        name_parts: impl IntoIterator<Item = String>,
        name_format: impl Into<String>,
        strasse: impl Into<String>,
        ort: impl Into<String>,
        plz: impl Into<String>,
        land: impl Into<String>,
    ) -> Self {
        self.spec.addressed_nads.push(Anschrift {
            qualifier: party_qualifier.into(),
            name_parts: name_parts.into_iter().collect(),
            name_format: name_format.into(),
            strasse: strasse.into(),
            ort: ort.into(),
            plz: plz.into(),
            land: land.into(),
        });
        self
    }

    /// Add a `SG8 SEQ+Z79` Produktpaket to this Vorgang.
    ///
    /// The AHB marks the segment group Muss on every Anmeldung einer Zuordnung
    /// des LFN and on its Bestätigung — 55001, 55077, 55600, 55601, 55014 and
    /// 55608 — and the Codeliste der Konfigurationen 1.4 Kap. 6.1.1 makes the
    /// Bilanzkreis product unconditional inside it.
    ///
    /// ```rust
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// # use edi_energy::utilmd_codes::{Produktpaket, dtm};
    /// let edi = UtilmdBuilder::new(Release::new("S2.2"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55608).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .transaction("VORGANG-0001")
    ///     .date(dtm::BEGINN_ZUM, "20261101")
    ///     .produktpaket(Produktpaket::bilanzkreis("11XBK-EEG-----1"))
    ///     .marktlokation("51238696012")
    ///     .done()
    ///     .serialize()?;
    /// let text = String::from_utf8(edi).unwrap();
    /// assert!(text.contains("SEQ+Z79+1"));
    /// assert!(text.contains("PIA+5+9991000002082:Z11"));
    /// assert!(text.contains("CCI+Z66"));
    /// assert!(text.contains("CAV+ZV4:::11XBK-EEG-----1"));
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn produktpaket(mut self, paket: Produktpaket) -> Self {
        self.spec.produktpakete.push(paket);
        self
    }

    /// Add a SG6/RFF reference segment.
    pub fn reference(mut self, qualifier: impl Into<String>, ref_id: impl Into<String>) -> Self {
        self.spec.references.push((qualifier.into(), ref_id.into()));
        self
    }

    /// Add an `SG12 NAD` beteiligter Marktpartner.
    ///
    /// `party_qualifier` is DE 3035 — `Z09` „Kunde des LF", `VY` „andere
    /// zugehörige Partei". The DE 3055 code list is derived from `id`, because
    /// the party named here is a *third* one whose issuing office need not match
    /// the sender's.
    ///
    /// Repeatable: PID 55036 names every Altlieferant an Abmeldeanfrage went to.
    pub fn customer(mut self, party_qualifier: impl Into<String>, id: impl Into<String>) -> Self {
        self.spec
            .customer_nads
            .push((party_qualifier.into(), id.into()));
        self
    }

    /// Add an `SG12 NAD` party identified by **name**.
    ///
    /// `party_qualifier` is DE 3035 — `Z09` „Kunde des LF" is the one the GPKE
    /// core processes use. `name_parts` fills `C080`'s five interchangeable
    /// DE 3036 components (Nachname, Vorname, … under
    /// [`namensformat::PERSON`](crate::utilmd_codes::namensformat::PERSON);
    /// one line under [`FIRMA`](crate::utilmd_codes::namensformat::FIRMA)) and
    /// `name_format` is DE 3045, which the AHB marks Muss wherever a `NAD`
    /// carries a Kundenname — without it the five components cannot be read
    /// back into a person or a company.
    pub fn named_party(
        mut self,
        party_qualifier: impl Into<String>,
        name_parts: impl IntoIterator<Item = String>,
        name_format: impl Into<String>,
    ) -> Self {
        self.spec.named_nads.push((
            party_qualifier.into(),
            name_parts.into_iter().collect(),
            name_format.into(),
        ));
        self
    }

    /// Add an `SG12 NAD+Z09` „Kunde des LF".
    ///
    /// Muss on a 55010 whose Transaktionsgrundergänzung is `ZW4` / `ZAP`
    /// (UTILMD AHB Strom Bedingung `[279]`): it is „der Kundenname aus der
    /// Anmeldung Lieferant neu" (`[572]`), and it is how the LFA tells an
    /// Einzug from a Wechsel at `E_0624` Prüfschritt 30.
    pub fn kunde_des_lf(
        self,
        name_parts: impl IntoIterator<Item = String>,
        name_format: impl Into<String>,
    ) -> Self {
        self.named_party(
            crate::utilmd_codes::nad::KUNDE_DES_LF,
            name_parts,
            name_format,
        )
    }

    /// Add an `SG12 NAD+VY` „andere zugehörige Partei".
    ///
    /// The slot the Zuordnungs-Meldungen use: the Altlieferant on PID 55036 /
    /// 44036, the auslösender Marktpartner on 55038 / 44038.
    pub fn beteiligter_marktpartner(self, mp_id: impl Into<String>) -> Self {
        self.customer(crate::utilmd_codes::nad::ZUGEHOERIGE_PARTEI, mp_id)
    }

    /// Set the `SG4 STS+Z35` „Status der Antwort des dritten Marktbeteiligten".
    ///
    /// **Muss on an Ablehnung whose Antwortcode is `A50` or `A57`** — UTILMD
    /// AHB Strom 2.1/2.2 Bedingungen `[356]` and `[84]`. Those two codes say the
    /// LFA refused to release the Marktlokation, and GPKE Teil 2 § 2.1.2 Nr. 6
    /// requires the NB to state *that* refusal's ground alongside its own.
    pub fn antwort_dritter(mut self, dritter: crate::utilmd_codes::DritterAntwortStatus) -> Self {
        self.spec.antwort_dritter = Some(dritter);
        self
    }

    /// Add a free-text (FTX) segment inside SG4.
    ///
    /// `FTX+ACB` carries the Erläuterung the Gas Codelisten require whenever an
    /// Ablehnung uses the catch-all `E14` „Ablehnung Sonstiges".
    pub fn free_text(mut self, text_function: impl Into<String>, text: impl Into<String>) -> Self {
        self.spec
            .free_texts
            .push((text_function.into(), text.into()));
        self
    }

    /// Set the AGR (Agreement Identification) segment inside SG4.
    pub fn agr(
        mut self,
        service_requirement: impl Into<String>,
        response_type: impl Into<String>,
    ) -> Self {
        self.spec.agr = Some((service_requirement.into(), response_type.into()));
        self
    }

    /// Finalize this Vorgang and return to the parent [`UtilmdBuilder`].
    pub fn done(mut self) -> UtilmdBuilder<S, R> {
        self.parent.inner.transactions.push(self.spec);
        self.parent
    }
}

/// The DE 2379 format code an `SG4 DTM` qualifier takes.
///
/// Read off the Anwendungsfall tables of UTILMD AHB Strom 2.2 and Gas 1.2: in
/// SG4 every date qualifier is `303` (`CCYYMMDDHHMMZZZ`, zone `+00` by
/// condition `[931]`) except two — `154` „Annahmedatum eines Angebots" is
/// `102` and `Z10` „Kündigungstermin" is `106`. The date-only qualifiers those
/// AHBs do carry (`752`, `Z09`, `Z20`, `Z21`, `Z22`) all sit in **SG6**, which
/// this builder writes through a different path.
///
/// The code follows the qualifier, never the value's length: a `YYYYMMDD`
/// Vorgangsdatum is still `303`, padded and zoned on the way out.
fn sg4_dtm_format(qualifier: &str) -> &'static str {
    match qualifier {
        "154" => "102",
        "Z10" => "106",
        _ => "303",
    }
}

// ── Sg8Builder ────────────────────────────────────────────────────────────────

/// Builds one `SG8 SEQ` block; `done()` returns to the Vorgang.
#[derive(Debug)]
#[must_use = "Sub-builder must be finalized with .done()"]
pub struct Sg8Builder<S = Unset, R = Unset> {
    parent: UtilmdTransactionBuilder<S, R>,
    block: Sg8Block,
}

impl<S, R> Sg8Builder<S, R> {
    /// `SG10 CCI+<Klassentyp>++<Merkmal>` — an empty Klassentyp gives `CCI+++Z15`.
    pub fn cci(mut self, klassentyp: impl Into<String>, merkmal: impl Into<String>) -> Self {
        self.block.items.push(Sg8Item::Cci {
            klassentyp: klassentyp.into(),
            merkmal: merkmal.into(),
        });
        self
    }

    /// `SG10 CAV+<Code>`.
    pub fn cav(self, code: impl Into<String>) -> Self {
        self.cav_wert(code, "")
    }

    /// `SG10 CAV+<Code>:::<Wert>`.
    pub fn cav_wert(mut self, code: impl Into<String>, wert: impl Into<String>) -> Self {
        self.block.items.push(Sg8Item::Cav {
            code: code.into(),
            wert: wert.into(),
        });
        self
    }

    /// `SG9 QTY+<Qualifier>:<Menge>:<Einheit>`; an empty Einheit is left out.
    pub fn qty(
        mut self,
        qualifier: impl Into<String>,
        menge: impl Into<String>,
        einheit: impl Into<String>,
    ) -> Self {
        self.block.items.push(Sg8Item::Qty {
            qualifier: qualifier.into(),
            menge: menge.into(),
            einheit: einheit.into(),
        });
        self
    }

    /// `SG8 RFF+<Qualifier>:<Referenz>`.
    pub fn rff(mut self, qualifier: impl Into<String>, referenz: impl Into<String>) -> Self {
        self.block.items.push(Sg8Item::Rff {
            qualifier: qualifier.into(),
            referenz: referenz.into(),
        });
        self
    }

    /// Back to the Vorgang.
    pub fn done(mut self) -> UtilmdTransactionBuilder<S, R> {
        self.parent.spec.stammdaten.push(self.block);
        self.parent
    }
}
