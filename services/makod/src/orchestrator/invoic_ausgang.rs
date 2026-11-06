//! The outbound INVOIC: a BO4E `Rechnung` as the message its Prüfidentifikator
//! requires.
//!
//! Built at the command, before the issuing process is spawned, so a Rechnung
//! that cannot go out is refused to its caller rather than dead-lettered after
//! it was recorded as sent. The steps are fixed:
//!
//! 1. **Seed by place.** Every value the `Rechnung` states is written to the
//!    MIG place that carries it, addressed by its `Nr`;
//!    [`edi_energy::Profile::seed_segment`] supplies the place's qualifier, so
//!    this module never spells one.
//! 2. **Complete.** [`edi_energy::Profile::completion`] adds what the
//!    Prüfschablone fixes (`CUX+2:EUR:4`, `PYT+3`, `DTM` format `303`).
//! 3. **Refuse what nobody stated.** A place the AHB requires and the
//!    `Rechnung` does not fill — a Firmenname, a Marktlokation, an Artikel-ID —
//!    would be completed with a made-up value, so it is a refusal naming the
//!    place ([`InvoicAusgangError::Unstated`]).
//! 4. **Validate** against the profile of the PID; an error-severity finding
//!    refuses with its place ([`InvoicAusgangError::Invalid`]).
//!
//! Amounts go out as the `Rechnung` states them: `rust_decimal`'s notation,
//! trailing zeros dropped (the value unchanged), never re-rounded — a `Betrag`
//! with more places than `[930]` admits is refused by step 4, not cut.
//!
//! The builder lives here rather than in `edi-energy`: the period bounds are
//! instants at 00:00 gesetzlicher Zeit (INVOIC MIG 2.8e, 00007/00008, S. 11 f.),
//! which needs the Europe/Berlin rules `mako-fristen` holds and `edi-energy`
//! deliberately does not, and putting it in `mako-invoic` would make every
//! workflow crate link the EDIFACT profiles.

use edi_energy::{Completion, MessageType, Profile, ReleaseRegistry, SkeletonParties};
use edifact_rs::{OwnedSegment, ValidationSeverity};
use rubo4e::current::{
    Adresse, Energierichtung, Geschaeftspartner, Mengeneinheit, NetznutzungRechnungsart,
    NetznutzungRechnungstyp, Rechnung, Rechnungsposition, Steuerart, Steuerbetrag, Waehrungscode,
};
use rust_decimal::Decimal;
use time::OffsetDateTime;

/// The Prüfidentifikator of the Stornorechnung (INVOIC AHB 1.0b Kap. 3.1.2).
pub const STORNO_PID: u32 = 31004;

/// BDEW-Artikelnummer Mehrmenge (Codeliste der Artikelnummern und Artikel-ID
/// 5.6, Kap. 2, S. 7) — the code `[14]` names.
const MEHRMENGE: &str = "9990001000748";

/// What the message needs beyond the `Rechnung`.
#[derive(Debug, Clone)]
pub struct Kopf<'a> {
    /// `SG1 RFF+Z13`.
    pub pid: u32,
    /// `NAD+MS` / `UNB` sender MP-ID.
    pub sender: &'a str,
    /// `NAD+MR` / `UNB` receiver MP-ID.
    pub receiver: &'a str,
    /// `UNH` / `UNT` DE 0062.
    pub message_ref: &'a str,
    /// `DTM+9` — the Buchungsdatum of the receivable (MIG 2.8e 00006, S. 11).
    pub buchungsdatum: OffsetDateTime,
    /// `SG1 RFF+ACE` „Nummer des zugehörigen Dokuments" (MIG 2.8e 00020,
    /// S. 27): the Lieferschein-MSCONS of an NN-Rechnung Strom, the ORDERS of
    /// a Rechnung für Sperren, the QUOTES or ORDERS of a MSB-Rechnung. BO4E
    /// has no field for it.
    pub zugehoeriges_dokument: Option<&'a str>,
    /// `SG1 RFF+Z56` — the MP-ID of the NB „von welchem die Preise im
    /// Preisblatt vergeben wurden" (MIG 2.8e 00021, S. 28).
    pub preise_mp_id: Option<&'a str>,
    /// `IMD` DE 7081 where BO4E's `NetznutzungRechnungstyp` has no variant for
    /// the code: `MSB`, `KON`, `TEC` (31009), `Z43`–`Z45` (31011), `SOR`
    /// (MIG 2.8e 00014). A Rechnung that states its type itself must state
    /// the same one.
    pub rechnungstyp: Option<&'a str>,
    /// `DTM+203` Ausführungsdatum (MIG 2.8e 00011, 00040): the day the
    /// beauftragte Leistung was performed — on a Rechnung für Sperren `[518]`
    /// the day of the Sperrung, of the last Sperrversuch, or of the
    /// Stornierung's receipt. One `RFF+ACE` names one Auftrag, so the day is
    /// the message's and every position's. BO4E has no field for it.
    pub ausfuehrungsdatum: Option<time::Date>,
}

/// One finding the profile refused the built message with.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Befund {
    /// The MIG place, where the finding names one.
    pub nr: Option<String>,
    /// The rule id (`AHB-31002-00041-MOA-5004-FORMAT`, …).
    pub regel: Option<String>,
    /// The validator's sentence.
    pub text: String,
}

/// Why a `Rechnung` cannot go out as an INVOIC.
#[derive(Debug, Clone)]
pub enum InvoicAusgangError {
    /// No INVOIC profile is in force on the day.
    KeinProfil(time::Date),
    /// The profile has no Prüfschablone for the PID.
    KeinePruefschablone {
        /// The PID asked for.
        pid: u32,
        /// The profile's release.
        release: String,
    },
    /// The `Rechnung` states something the profile has no place or code for.
    NichtDarstellbar {
        /// The MIG place.
        nr: &'static str,
        /// What the `Rechnung` states and why it cannot be written.
        grund: String,
    },
    /// A value the builder took from the `Rechnung` does not fit its place.
    Seed(edi_energy::SeedError),
    /// Places the AHB requires and the `Rechnung` does not fill.
    Unstated(Vec<edi_energy::Unstated>),
    /// The completed message fails its Prüfschablone.
    Invalid(Vec<Befund>),
}

impl std::fmt::Display for InvoicAusgangError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeinProfil(d) => write!(f, "no INVOIC profile is in force on {d}"),
            Self::KeinePruefschablone { pid, release } => {
                write!(f, "INVOIC {release} has no Prüfschablone for PID {pid}")
            }
            Self::NichtDarstellbar { nr, grund } => write!(f, "place {nr}: {grund}"),
            Self::Seed(e) => write!(f, "{e}"),
            Self::Unstated(u) => write!(
                f,
                "the Rechnung does not state what the AHB requires at {}",
                list(u)
            ),
            Self::Invalid(b) => write!(
                f,
                "the INVOIC fails its Prüfschablone: {}",
                b.iter()
                    .map(|b| b.text.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        }
    }
}

impl std::error::Error for InvoicAusgangError {}

impl From<edi_energy::SeedError> for InvoicAusgangError {
    fn from(e: edi_energy::SeedError) -> Self {
        Self::Seed(e)
    }
}

fn list(u: &[edi_energy::Unstated]) -> String {
    u.iter()
        .map(|u| format!("{} {} DE {}", u.nr, u.tag, u.de))
        .collect::<Vec<_>>()
        .join(", ")
}

impl InvoicAusgangError {
    /// The places the refusal names, for the caller's response body.
    #[must_use]
    pub fn places(&self) -> Vec<String> {
        match self {
            Self::NichtDarstellbar { nr, .. } => vec![(*nr).to_owned()],
            Self::Seed(
                edi_energy::SeedError::UnknownPlace { nr }
                | edi_energy::SeedError::UnknownElement { nr, .. }
                | edi_energy::SeedError::NotAdmitted { nr, .. }
                | edi_energy::SeedError::Undecided { nr, .. },
            ) => vec![nr.clone()],
            Self::Unstated(u) => u.iter().map(|u| u.nr.clone()).collect(),
            Self::Invalid(b) => b.iter().filter_map(|b| b.nr.clone()).collect(),
            Self::KeinProfil(_) | Self::KeinePruefschablone { .. } => Vec::new(),
        }
    }
}

/// The built message, `UNH` … `UNT`, validated against its Prüfschablone.
#[derive(Debug, Clone)]
pub struct AusgehendeInvoic {
    /// The message as EDIFACT text, the outbox payload's form.
    pub nachricht: String,
}

/// The INVOIC profile in force on `am`.
#[must_use]
pub fn profil(am: time::Date) -> Option<&'static Profile> {
    ReleaseRegistry::global().profile_for_date_and_track(
        MessageType::Invoic,
        am,
        edi_energy::ReleaseTrack::Short,
    )
}

/// Build the INVOIC of `rechnung` under `kopf.pid`.
///
/// # Errors
///
/// [`InvoicAusgangError`], naming the place.
pub fn rechnung(
    profile: &Profile,
    rechnung: &Rechnung,
    kopf: &Kopf<'_>,
) -> Result<AusgehendeInvoic, InvoicAusgangError> {
    let mut b = Seeder::new(profile, kopf)?;
    let nummer = rechnung.rechnungsnummer.as_deref().unwrap_or_default();
    b.header(
        rechnung,
        nummer,
        bgm_code(rechnung, false),
        rechnung.rechnungsdatum,
    )?;
    b.sg1(None)?;
    b.parteien(rechnung)?;
    b.zahlung(rechnung.faelligkeitsdatum)?;
    for (i, p) in rechnung.rechnungspositionen.iter().flatten().enumerate() {
        b.position(rechnung, p, i)?;
    }
    b.summen(rechnung)?;
    b.finish()
}

/// Build the Stornorechnung (PID 31004) `storno` states.
///
/// `storno` is the Stornorechnung as its producer computed it: its own
/// number, dates and Summenteil, with `ist_storno` set and the
/// `original_rechnungsnummer` it cancels. Its amounts are written as stated —
/// „Bei einer Rechnungsstornierung sind die Vorzeichen im Verhältnis zur
/// Ursprungsrechnung zu negieren" (MIG 2.8e, 00052–00057, S. 59 ff.) is the
/// producer's to have done, as `grid-billing`'s reversal does. The message
/// carries `BGM` 457 (`Z25` for a selbst ausgestellte Rechnung), `SG1 RFF+OI`
/// with the original's number and `DTM+171` with the original's `DTM+137`
/// (`ursprungsdatum`, MIG 2.8e 00019). The 31004 column lists no `SG26`, so
/// no position is repeated.
///
/// # Errors
///
/// [`InvoicAusgangError`], naming the place.
pub fn storno(
    profile: &Profile,
    storno: &Rechnung,
    ursprungsdatum: Option<OffsetDateTime>,
    kopf: &Kopf<'_>,
) -> Result<AusgehendeInvoic, InvoicAusgangError> {
    // The 31004 column has no `RFF+ACE` and no `RFF+Z56`: the Storno refers
    // to the Ursprungsrechnung, not to what it settled. It keeps the
    // Ausführungsdatum, which it lists Muss `[47]` on a Rechnung für Sperren.
    let kopf = Kopf {
        pid: STORNO_PID,
        zugehoeriges_dokument: None,
        preise_mp_id: None,
        ..kopf.clone()
    };
    let mut b = Seeder::new(profile, &kopf)?;
    let nummer = storno.rechnungsnummer.as_deref().unwrap_or_default();
    b.header(
        storno,
        nummer,
        bgm_code(storno, true),
        storno.rechnungsdatum,
    )?;
    let ursprung = storno
        .original_rechnungsnummer
        .as_deref()
        .unwrap_or_default();
    b.sg1(Some((ursprung, ursprungsdatum)))?;
    b.parteien(storno)?;
    b.zahlung(storno.faelligkeitsdatum)?;
    b.summen(storno)?;
    b.finish()
}

/// `BGM` DE 1001: `389` / `Z25` for a selbst ausgestellte Rechnung (and its
/// Storno), `380` / `457` otherwise (MIG 2.8e 00004).
fn bgm_code(r: &Rechnung, storno: bool) -> &'static str {
    let selbst = r.netznutzungrechnungsart == Some(NetznutzungRechnungsart::Selbstausgestellt);
    match (storno, selbst) {
        (false, false) => "380",
        (false, true) => "389",
        (true, false) => "457",
        (true, true) => "Z25",
    }
}

/// `IMD` DE 7081, by the code's own MIG name (00014).
fn rechnungstyp(t: NetznutzungRechnungstyp) -> Option<&'static str> {
    Some(match t {
        NetznutzungRechnungstyp::Abschlussrechnung => "ABR",
        NetznutzungRechnungstyp::Abschlagsrechnung => "ABS",
        NetznutzungRechnungstyp::Turnusrechnung => "JVR",
        NetznutzungRechnungstyp::Monatsrechnung => "MVR",
        NetznutzungRechnungstyp::Wimrechnung => "WIM",
        NetznutzungRechnungstyp::Zwischenrechnung => "ZVR",
        NetznutzungRechnungstyp::Integrierte13TeRechnung => "13I",
        NetznutzungRechnungstyp::Zusaetzliche13TeRechnung => "13R",
        NetznutzungRechnungstyp::Mehrmindermengenrechnung => "MMM",
        _ => return None,
    })
}

/// An instant as `DTM` format `303`, `CCYYMMDDHHMMZZZ` in UTC (`[931]`).
fn dtm303(at: OffsetDateTime) -> String {
    let t = at.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}{:02}{:02}+00",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute()
    )
}

/// A Zeitraum's inclusive first day as its `DTM` Beginn: 00:00 gesetzlicher
/// Zeit of that day. The Ende is 00:00 of the day after the inclusive last
/// day — MIG 2.8e 00007/00008: `DTM+155:202107312200?+00:303` and
/// `DTM+156:202108312200?+00:303` are one month, August 2021.
fn beginn(d: time::Date) -> String {
    dtm303(mako_fristen::berlin_midnight(d))
}

fn ende(d: time::Date) -> Option<String> {
    d.next_day()
        .map(|n| dtm303(mako_fristen::berlin_midnight(n)))
}

/// A decimal in EDIFACT notation, its value unchanged.
fn zahl(d: Decimal) -> String {
    d.normalize().to_string()
}

fn landescode(a: &Adresse) -> Option<String> {
    a.landescode
        .as_ref()
        .and_then(|l| serde_json::to_value(l).ok())
        .and_then(|v| v.as_str().map(str::to_owned))
}

/// `DE` 3042: Straße und Hausnummer, or the Postfach. The column lists one
/// occurrence, so both travel in it.
fn strasse(a: &Adresse) -> Option<String> {
    match (&a.strasse, &a.hausnummer, &a.postfach) {
        (Some(s), Some(h), _) => Some(format!("{s} {h}")),
        (Some(s), None, _) => Some(s.clone()),
        (None, _, Some(p)) => Some(format!("Postfach {p}")),
        _ => None,
    }
}

struct Seeder<'p> {
    profile: &'p Profile,
    af: &'p edi_energy::profile::Anwendungsfall,
    pid: u32,
    sender: String,
    receiver: String,
    message_ref: String,
    buchungsdatum: OffsetDateTime,
    zugehoeriges_dokument: Option<String>,
    preise_mp_id: Option<String>,
    rechnungstyp: Option<String>,
    ausfuehrungsdatum: Option<time::Date>,
    out: Vec<OwnedSegment>,
}

impl<'p> Seeder<'p> {
    fn new(profile: &'p Profile, kopf: &Kopf<'_>) -> Result<Self, InvoicAusgangError> {
        let af = profile.anwendungsfall(kopf.pid).ok_or_else(|| {
            InvoicAusgangError::KeinePruefschablone {
                pid: kopf.pid,
                release: profile.release().as_str().to_owned(),
            }
        })?;
        Ok(Self {
            profile,
            af,
            pid: kopf.pid,
            sender: kopf.sender.to_owned(),
            receiver: kopf.receiver.to_owned(),
            message_ref: kopf.message_ref.to_owned(),
            buchungsdatum: kopf.buchungsdatum,
            zugehoeriges_dokument: kopf.zugehoeriges_dokument.map(str::to_owned),
            preise_mp_id: kopf.preise_mp_id.map(str::to_owned),
            rechnungstyp: kopf.rechnungstyp.map(str::to_owned),
            ausfuehrungsdatum: kopf.ausfuehrungsdatum,
            out: Vec::new(),
        })
    }

    /// Whether the column lists place `nr` at all.
    fn listed(&self, nr: &str) -> bool {
        self.af.segment_status(nr).is_some()
    }

    fn seed(&mut self, nr: &str, values: &[(&str, &str)]) -> Result<(), InvoicAusgangError> {
        self.out.push(self.profile.seed_segment(nr, values)?);
        Ok(())
    }

    /// `UNH` … `IMD`.
    fn header(
        &mut self,
        r: &Rechnung,
        nummer: &str,
        bgm: &str,
        rechnungsdatum: Option<OffsetDateTime>,
    ) -> Result<(), InvoicAusgangError> {
        let release = self.profile.release().as_str().to_owned();
        let msg_ref = self.message_ref.clone();
        self.seed(
            "00003",
            &[
                ("0062", &msg_ref),
                ("0065", "INVOIC"),
                ("0052", "D"),
                ("0054", "06A"),
                ("0051", "UN"),
                ("0057", &release),
            ],
        )?;
        // `1225` `9` Original: mako sends each document once and re-sends
        // nothing as a Duplikat.
        self.seed("00004", &[("1001", bgm), ("1004", nummer), ("1225", "9")])?;
        if let Some(datum) = rechnungsdatum.map(dtm303) {
            self.seed("00005", &[("2380", &datum), ("2379", "303")])?;
        }
        let buchung = dtm303(self.buchungsdatum);
        self.seed("00006", &[("2380", &buchung), ("2379", "303")])?;
        // The Abschlagsrechnung states the vorläufiger Abrechnungszeitraum
        // (Z42/Z43, 00009/00010); the others the Abrechnungszeitraum
        // (155/156, 00007/00008). Which one is the column's to say.
        let (von, bis) = if self.listed("00007") {
            ("00007", "00008")
        } else {
            ("00009", "00010")
        };
        let periode = r.rechnungsperiode.as_ref();
        if let Some(start) = periode.and_then(|z| z.startdatum).map(beginn) {
            self.seed(von, &[("2380", &start), ("2379", "303")])?;
        }
        if let Some(end) = periode.and_then(|z| z.enddatum).and_then(ende) {
            self.seed(bis, &[("2380", &end), ("2379", "303")])?;
        }
        self.ausfuehrung("00011")?;
        let aus_bo4e = r
            .netznutzungrechnungstyp
            .map(|t| {
                rechnungstyp(t).ok_or_else(|| InvoicAusgangError::NichtDarstellbar {
                    nr: "00014",
                    grund: format!("Rechnungstyp {t:?} has no IMD DE 7081 code"),
                })
            })
            .transpose()?;
        let typ = match (aus_bo4e, self.rechnungstyp.as_deref()) {
            (Some(a), Some(k)) if a != k => {
                return Err(InvoicAusgangError::NichtDarstellbar {
                    nr: "00014",
                    grund: format!(
                        "the Rechnung states Rechnungstyp {a} and the command {k}; IMD carries one"
                    ),
                });
            }
            (Some(a), _) => Some(a.to_owned()),
            (None, k) => k.map(str::to_owned),
        };
        // `7009` Z06 Erzeugung / Z07 Verbrauch, where the column lists it (the
        // MMM-Rechnung): the Marktlokation „liefert Energie ins Netz" or
        // „entnimmt Energie aus dem Netz" (MIG 2.8e 00014, S. 19) — its
        // Energierichtung, which only the Rechnung's Marktlokation states.
        let richtung = if self.af.element_rules("00014").any(|e| e.de == "7009") {
            match r.marktlokation.as_ref().and_then(|m| m.energierichtung) {
                Some(Energierichtung::Einsp) => "Z06",
                Some(Energierichtung::Aussp) => "Z07",
                _ => "",
            }
        } else {
            ""
        };
        if let Some(typ) = typ {
            self.seed("00014", &[("7081", &typ), ("7009", richtung)])?;
        }
        Ok(())
    }

    /// `DTM+203` Ausführungsdatum at place `nr`, format `102`, where the
    /// column lists it.
    fn ausfuehrung(&mut self, nr: &str) -> Result<(), InvoicAusgangError> {
        if let Some(d) = self.ausfuehrungsdatum.filter(|_| self.listed(nr)) {
            let tag = format!("{:04}{:02}{:02}", d.year(), u8::from(d.month()), d.day());
            self.seed(nr, &[("2380", &tag), ("2379", "102")])?;
        }
        Ok(())
    }

    /// `SG1`: the Prüfidentifikator, the Ursprungsrechnung of a Storno, the
    /// zugehöriges Dokument and the Preise des Netzbetreibers.
    fn sg1(
        &mut self,
        ursprung: Option<(&str, Option<OffsetDateTime>)>,
    ) -> Result<(), InvoicAusgangError> {
        let pid = self.pid.to_string();
        self.seed("00017", &[("1154", &pid)])?;
        if let Some((nummer, datum)) = ursprung {
            self.seed("00018", &[("1154", nummer)])?;
            if let Some(datum) = datum.map(dtm303) {
                self.seed("00019", &[("2380", &datum), ("2379", "303")])?;
            }
        }
        if let Some(ace) = self.zugehoeriges_dokument.clone() {
            self.seed("00020", &[("1154", &ace)])?;
        }
        if let Some(z56) = self.preise_mp_id.clone() {
            self.seed("00021", &[("1154", &z56)])?;
        }
        Ok(())
    }

    /// One `NAD` party: MP-ID, Firmenname, Anschrift.
    fn partei(
        &mut self,
        nr: &str,
        mp_id: &str,
        gp: Option<&Geschaeftspartner>,
    ) -> Result<(), InvoicAusgangError> {
        let agency = edi_energy::AgencyCode::for_mp_id(mp_id).as_str().to_owned();
        let name = gp
            .and_then(|g| g.organisationsname.clone())
            .unwrap_or_default();
        let adresse = gp.and_then(|g| g.adresse.as_ref());
        let str_ = adresse.and_then(strasse).unwrap_or_default();
        let ort = adresse.and_then(|a| a.ort.clone()).unwrap_or_default();
        let plz = adresse
            .and_then(|a| a.postleitzahl.clone())
            .unwrap_or_default();
        let land = adresse.and_then(landescode).unwrap_or_default();
        // `3045` `Z02` Struktur der Firmenbezeichnung: a Marktpartner is an
        // organisation, and the name is its `organisationsname`.
        let format = if name.is_empty() { "" } else { "Z02" };
        self.seed(
            nr,
            &[
                ("3039", mp_id),
                ("3055", &agency),
                ("3036", &name),
                ("3045", format),
                ("3042", &str_),
                ("3164", &ort),
                ("3251", &plz),
                ("3207", &land),
            ],
        )
    }

    /// `SG2`: Absender, Empfänger, Lieferanschrift with its Marktlokation.
    fn parteien(&mut self, r: &Rechnung) -> Result<(), InvoicAusgangError> {
        let ersteller = r.rechnungsersteller.as_deref();
        let empfaenger = r.rechnungsempfaenger.as_deref();
        let sender = self.sender.clone();
        let receiver = self.receiver.clone();
        self.partei("00022", &sender, ersteller)?;
        // `SG3 RFF` of the Absender: the Umsatzsteuer-Identifikationsnummer
        // (`VA`). BO4E carries no Steuernummer, so `FC` is not offered.
        if let Some(ust) = ersteller.and_then(|g| g.umsatzsteuer_id.clone()) {
            self.seed("00023", &[("1153", "VA"), ("1154", &ust)])?;
        }
        self.partei("00026", &receiver, empfaenger)?;
        // `SG3 RFF` of the Empfänger: Muss `[5]` (not in Germany) and Soll
        // `[4]` (Steuerschuldnerschaft des Leistungsempfängers); on the
        // selbst ausgestellte Rechnung Muss `[527]` without a condition — the
        // Empfänger of the message is the leistende Unternehmer whose number
        // the Gutschrift states.
        let selbst = r.netznutzungrechnungsart == Some(NetznutzungRechnungsart::Selbstausgestellt);
        let ausland = empfaenger
            .and_then(|g| g.adresse.as_ref())
            .and_then(landescode)
            .is_some_and(|l| l != "DE");
        let reverse = r
            .steuerbetraege
            .iter()
            .flatten()
            .any(|s| s.steuerart == Some(Steuerart::Rcv));
        if let Some(ust) = empfaenger.and_then(|g| g.umsatzsteuer_id.clone())
            && (ausland || reverse || selbst)
        {
            self.seed("00027", &[("1153", "VA"), ("1154", &ust)])?;
        }
        if self.listed("00028") {
            let malo = r.marktlokation.as_deref();
            if let Some(adresse) = malo.and_then(|m| m.lokationsadresse.as_ref()) {
                let str_ = strasse(adresse).unwrap_or_default();
                let ort = adresse.ort.clone().unwrap_or_default();
                let plz = adresse.postleitzahl.clone().unwrap_or_default();
                let land = landescode(adresse).unwrap_or_default();
                self.seed(
                    "00028",
                    &[
                        ("3042", &str_),
                        ("3164", &ort),
                        ("3251", &plz),
                        ("3207", &land),
                    ],
                )?;
            }
            if let Some(id) = malo.and_then(|m| m.marktlokations_id.as_ref()) {
                self.seed("00029", &[("3225", id.as_ref())])?;
            }
        }
        Ok(())
    }

    /// `SG7 CUX` and `SG8` Fälligkeit.
    fn zahlung(&mut self, faellig: Option<OffsetDateTime>) -> Result<(), InvoicAusgangError> {
        self.seed("00031", &[("6347", "2"), ("6345", "EUR"), ("6343", "4")])?;
        self.seed("00032", &[("4279", "3")])?;
        // `[UB1]`: the Fälligkeitsdatum is a day, stated as its Beginn — 00:00
        // gesetzlicher Zeit. BO4E types it `date-time`; the day is the date
        // the value carries in its own offset, as `rubo4e` reads it.
        if let Some(datum) = faellig.map(|t| beginn(t.date())) {
            self.seed("00033", &[("2380", &datum), ("2379", "303")])?;
        }
        Ok(())
    }

    /// The rate and category of a position: its own `steuerbetrag`, or the
    /// invoice's one rate when the invoice is taxed uniformly.
    fn steuer_der_position<'r>(
        r: &'r Rechnung,
        p: &'r Rechnungsposition,
    ) -> Option<&'r Steuerbetrag> {
        let einheitlich = match r.steuerbetraege.as_deref() {
            Some([only]) => Some(only),
            _ => None,
        };
        p.steuerbetrag.as_ref().or(einheitlich)
    }

    fn tax(
        &mut self,
        nr: &'static str,
        s: Option<&Steuerbetrag>,
    ) -> Result<(), InvoicAusgangError> {
        if s.is_none() {
            return Ok(());
        }
        let satz = s.and_then(|s| s.steuersatz).map(zahl).unwrap_or_default();
        let kategorie = match s.and_then(|s| s.steuerart) {
            Some(Steuerart::Ust) => "S",
            Some(Steuerart::Rcv) => "AE",
            Some(other) => {
                return Err(InvoicAusgangError::NichtDarstellbar {
                    nr,
                    grund: format!("Steuerart {other:?} has no TAX DE 5305 category"),
                });
            }
            None => "",
        };
        self.seed(nr, &[("5153", "VAT"), ("5278", &satz), ("5305", kategorie)])
    }

    /// One `SG26`.
    fn position(
        &mut self,
        r: &Rechnung,
        p: &Rechnungsposition,
        index: usize,
    ) -> Result<(), InvoicAusgangError> {
        // `[911]`: numbered from 1, ascending — the Rechnung's own numbering
        // where it states one.
        let nummer = p
            .positionsnummer
            .map_or_else(|| (index + 1).to_string(), |n| n.to_string());
        if p.artikel_id.is_none()
            && let Some(a) = p.artikelnummer
        {
            return Err(InvoicAusgangError::NichtDarstellbar {
                nr: "00034",
                grund: format!(
                    "position {nummer} names the BO4E Artikelnummer {a:?}, not its BDEW code; \
                     state the code of the Codeliste der Artikelnummern und Artikel-ID as artikel_id"
                ),
            });
        }
        let artikel = p.artikel_id.clone().unwrap_or_default();
        // `7143` follows the code's own form: an Artikel-ID is
        // `n1-n2-n1-…` (Codeliste der Artikelnummern und Artikel-ID 5.6,
        // Kap. 3, S. 12) and goes out as `Z09`; a BDEW-Artikelnummer is the
        // 13-digit code of Kap. 2 and goes out as `Z01`. Whether the column
        // admits that kind for the period is the Prüfschablone's to say.
        let art = match artikel.as_str() {
            "" => "",
            a if a.contains('-') => "Z09",
            _ => "Z01",
        };
        self.seed(
            "00034",
            &[("1082", &nummer), ("7140", &artikel), ("7143", art)],
        )?;

        if let Some(m) = &p.positions_menge {
            let menge = m.wert.map(zahl).unwrap_or_default();
            let einheit = match m.einheit {
                Some(Mengeneinheit::Kwh) => "KWH",
                Some(Mengeneinheit::Kw) => "KWT",
                Some(Mengeneinheit::Kvar) => "KVR",
                Some(Mengeneinheit::Kvarh) => "K3",
                Some(Mengeneinheit::Stueck) => "H87",
                None => "",
                Some(other) => {
                    return Err(InvoicAusgangError::NichtDarstellbar {
                        nr: "00035",
                        grund: format!(
                            "position {nummer}: {other:?} is not an energetische Mengeneinheit \
                                 (KWH, KWT, KVR, K3, H87)"
                        ),
                    });
                }
            };
            self.seed("00035", &[("6060", &menge), ("6411", einheit)])?;
        }
        // `[14]`: a Mehrmenge position of the MMM-Rechnung carries the
        // Korrekturfaktor `QTY+Z17:-1`, the one value MIG 2.8e 00037 (S. 44)
        // gives it — „(MOA+203) = (QTY+Z17) * (QTY+47) * (PRI+CAL)". The
        // credit's sign travels in the factor, so the price goes out as its
        // amount: BO4E's negative `einzelpreis` is `QTY+Z17:-1` with a positive
        // `PRI`, and the net stays as stated.
        let korrektur = artikel == MEHRMENGE && self.listed("00037");
        if korrektur {
            self.seed("00037", &[("6060", "-1")])?;
        }
        let zeiteinheit = |e: Option<Mengeneinheit>, nr: &'static str| match e {
            Some(Mengeneinheit::Tag) => Ok("DAY"),
            Some(Mengeneinheit::Monat) => Ok("MON"),
            Some(Mengeneinheit::Jahr) => Ok("ANN"),
            None => Ok(""),
            Some(other) => Err(InvoicAusgangError::NichtDarstellbar {
                nr,
                grund: format!("position {nummer}: {other:?} is not a Zeiteinheit (DAY, MON, ANN)"),
            }),
        };
        if let Some(z) = &p.zeitbezogene_menge {
            let wert = z.wert.map(zahl).unwrap_or_default();
            let einheit = zeiteinheit(z.einheit, "00036")?;
            self.seed("00036", &[("6060", &wert), ("6411", einheit)])?;
        }
        let lz = p.lieferungszeitraum.as_ref();
        if let Some(von) = lz.and_then(|z| z.startdatum).map(beginn) {
            self.seed("00038", &[("2380", &von), ("2379", "303")])?;
        }
        if let Some(bis) = lz.and_then(|z| z.enddatum).and_then(ende) {
            self.seed("00039", &[("2380", &bis), ("2379", "303")])?;
        }
        self.ausfuehrung("00040")?;
        waehrung("00041", p.gesamtpreis.as_ref())?;
        if let Some(netto) = p.gesamtpreis.as_ref().and_then(|b| b.wert).map(zahl) {
            self.seed("00041", &[("5004", &netto)])?;
        }
        if let Some(preis) = &p.einzelpreis {
            if preis.einheit.is_some_and(|e| {
                serde_json::to_value(e)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .as_deref()
                    != Some("EUR")
            }) {
                return Err(InvoicAusgangError::NichtDarstellbar {
                    nr: "00043",
                    grund: format!(
                        "position {nummer}: the price is not stated in EUR, the CUX currency"
                    ),
                });
            }
            let wert = preis
                .wert
                .map(|w| if korrektur { w.abs() } else { w })
                .map(zahl)
                .unwrap_or_default();
            let je = zeiteinheit(p.zeiteinheit, "00043")?;
            self.seed("00043", &[("5118", &wert), ("6411", je)])?;
        }
        self.tax("00044", Self::steuer_der_position(r, p))
    }

    /// `UNS` and the Summenteil.
    fn summen(&mut self, r: &Rechnung) -> Result<(), InvoicAusgangError> {
        self.seed("00051", &[("0081", "S")])?;
        waehrung("00052", r.gesamtbrutto.as_ref())?;
        waehrung("00057", r.zu_zahlen.as_ref())?;
        let betrag = |b: Option<Decimal>| b.map(zahl);
        if let Some(brutto) = betrag(r.gesamtbrutto.as_ref().and_then(|b| b.wert)) {
            self.seed("00052", &[("5004", &brutto)])?;
        }
        for v in r.vorauszahlungen.iter().flatten() {
            waehrung("00053", v.betrag.as_ref())?;
            if let Some(wert) = betrag(v.betrag.as_ref().and_then(|b| b.wert)) {
                self.seed("00053", &[("5004", &wert)])?;
            }
            if let Some(referenz) = &v.referenz {
                self.seed("00054", &[("1154", referenz)])?;
                if let Some(datum) = v.datum.map(dtm303) {
                    self.seed("00055", &[("2380", &datum), ("2379", "303")])?;
                }
            }
        }
        // MIG 2.8e 00057: the fällige Betrag is the Rechnungsbetrag less every
        // Vorausbezahlter Betrag. Without prepayments the two coincide, so a
        // Rechnung that states no `zu_zahlen` has stated it through its gross.
        let faellig = r.zu_zahlen.as_ref().and_then(|b| b.wert).or_else(|| {
            r.vorauszahlungen
                .as_ref()
                .is_none_or(Vec::is_empty)
                .then(|| r.gesamtbrutto.as_ref().and_then(|b| b.wert))
                .flatten()
        });
        if let Some(faellig) = betrag(faellig) {
            self.seed("00057", &[("5004", &faellig)])?;
        }
        for s in r.steuerbetraege.iter().flatten() {
            self.tax("00058", Some(s))?;
            if let Some(basis) = betrag(s.basiswert) {
                self.seed("00061", &[("5004", &basis)])?;
            }
            if let Some(steuer) = betrag(s.steuerwert) {
                self.seed("00062", &[("5004", &steuer)])?;
            }
        }
        Ok(())
    }

    /// `UNT`, completion, the refusal of anything unstated, validation.
    fn finish(mut self) -> Result<AusgehendeInvoic, InvoicAusgangError> {
        let msg_ref = self.message_ref.clone();
        self.seed("00063", &[("0074", "0"), ("0062", &msg_ref)])?;
        let parties = SkeletonParties {
            sender: self.sender.clone(),
            receiver: self.receiver.clone(),
        };
        let Completion { segments, unstated } =
            self.profile.completion(&self.out, self.af, &parties);
        if !unstated.is_empty() {
            return Err(InvoicAusgangError::Unstated(unstated));
        }
        let pid = edi_energy::Pruefidentifikator::new(self.pid).ok();
        let befunde: Vec<Befund> = self
            .profile
            .validate(&segments, pid)
            .into_iter()
            .filter(refuses)
            .map(|i| Befund {
                nr: i.context_get("nr").map(str::to_owned),
                regel: i.rule_id().map(str::to_owned),
                text: i.message.clone(),
            })
            .collect();
        if !befunde.is_empty() {
            return Err(InvoicAusgangError::Invalid(befunde));
        }
        let bytes = edifact_rs::segments_to_bytes(&segments).map_err(|e| {
            InvoicAusgangError::Invalid(vec![Befund {
                nr: None,
                regel: None,
                text: format!("the message cannot be written: {e}"),
            }])
        })?;
        let nachricht = String::from_utf8(bytes).map_err(|e| {
            InvoicAusgangError::Invalid(vec![Befund {
                nr: None,
                regel: None,
                text: format!("the message is not UTF-8: {e}"),
            }])
        })?;
        // The interchange goes out in UNOC (ISO 8859-1, Allgemeine
        // Festlegungen 6.1d, UNB DE0001); a character outside it is refused
        // here, to the caller, rather than at dispatch after the process began.
        edifact_rs::Charset::UnoC.encode(&nachricht).map_err(|e| {
            InvoicAusgangError::Invalid(vec![Befund {
                nr: None,
                regel: None,
                text: format!("the message cannot go out in UNOC: {e}"),
            }])
        })?;
        Ok(AusgehendeInvoic { nachricht })
    }
}

/// Whether a finding refuses the outbound message: every error, and every
/// Formatbedingung (`FMT-…`). The validator reports a Formatbedingung as a
/// warning because a receiver may still process the message; a sender has no
/// reason to send a value the AHB's format refuses — an amount with three
/// Nachkommastellen where `[930]` admits two is rounded by nobody.
fn refuses(i: &edifact_rs::ValidationIssue) -> bool {
    matches!(
        i.severity,
        ValidationSeverity::Error | ValidationSeverity::Critical
    ) || i
        .rule_id()
        .is_some_and(|r| r.starts_with(edi_energy::profile::validate::FMT_PREFIX))
}

/// `CUX` states EUR; an amount in another currency is not this invoice's.
fn waehrung(
    nr: &'static str,
    b: Option<&rubo4e::current::Betrag>,
) -> Result<(), InvoicAusgangError> {
    match b.and_then(|b| b.waehrung) {
        None | Some(Waehrungscode::Eur) => Ok(()),
        Some(other) => Err(InvoicAusgangError::NichtDarstellbar {
            nr,
            grund: format!("the amount is stated in {other:?}, and CUX states EUR"),
        }),
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! A representative NN-Rechnung and Abschlagsrechnung, shaped as
    //! `grid-billing`'s `into_rechnung` renders a settlement, with the parties
    //! and the Marktlokation the AHB requires beside it.

    use rubo4e::current::{
        Adresse, Betrag, Geschaeftspartner, Landescode, Marktlokation, Menge, Mengeneinheit,
        NetznutzungRechnungsart, NetznutzungRechnungstyp, Preis, Rechnung, Rechnungsposition,
        Rechnungstyp, Sparte, Steuerart, Steuerbetrag, Waehrungscode, Zeitraum,
    };
    use rust_decimal::Decimal;
    use time::macros::{date, datetime};

    pub(crate) const NB: &str = "9900357000004";
    pub(crate) const LF: &str = "9900012345678";
    pub(crate) const MALO: &str = "51238696012";
    /// DVGW-Codenummern (`98…`) of a Gasnetzbetreiber and a Lieferant.
    pub(crate) const GNB: &str = "9800357000004";
    pub(crate) const LFG: &str = "9800012345678";

    fn eur(cents: i64) -> Betrag {
        Betrag {
            wert: Some(Decimal::new(cents, 2)),
            waehrung: Some(Waehrungscode::Eur),
            ..Default::default()
        }
    }

    fn adresse(strasse: &str, nr: &str, plz: &str, ort: &str) -> Adresse {
        Adresse {
            strasse: Some(strasse.to_owned()),
            hausnummer: Some(nr.to_owned()),
            postleitzahl: Some(plz.to_owned()),
            ort: Some(ort.to_owned()),
            landescode: Some(Landescode::De),
            ..Default::default()
        }
    }

    fn partner(name: &str, adresse: Adresse, ust: &str) -> Box<Geschaeftspartner> {
        Box::new(Geschaeftspartner {
            organisationsname: Some(name.to_owned()),
            adresse: Some(adresse),
            umsatzsteuer_id: Some(ust.to_owned()),
            ..Default::default()
        })
    }

    fn januar() -> Zeitraum {
        Zeitraum {
            startdatum: Some(date!(2026 - 01 - 01)),
            enddatum: Some(date!(2026 - 01 - 31)),
            ..Default::default()
        }
    }

    fn kopf_der_rechnung(nummer: &str) -> Rechnung {
        Rechnung {
            rechnungsnummer: Some(nummer.to_owned()),
            rechnungsdatum: Some(datetime!(2026-02-15 00:00 UTC)),
            faelligkeitsdatum: Some(datetime!(2026-03-15 00:00 UTC)),
            rechnungsperiode: Some(januar()),
            rechnungstyp: Some(Rechnungstyp::Netznutzungsrechnung),
            netznutzungrechnungsart: Some(NetznutzungRechnungsart::Handelsrechnung),
            sparte: Some(Sparte::Strom),
            rechnungsersteller: Some(partner(
                "Stadtnetz Musterstadt GmbH",
                adresse("Netzweg", "1", "10115", "Berlin"),
                "DE123456789",
            )),
            rechnungsempfaenger: Some(partner(
                "Lieferant Beispiel AG",
                adresse("Handelsplatz", "7", "50667", "Köln"),
                "DE987654321",
            )),
            marktlokation: Some(Box::new(Marktlokation {
                marktlokations_id: Some(MALO.parse().expect("valid MaLo-ID")),
                lokationsadresse: Some(adresse("Lindenallee", "12", "10115", "Berlin")),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    fn steuer(basis: i64, steuer: i64) -> Steuerbetrag {
        Steuerbetrag {
            steuerart: Some(Steuerart::Ust),
            steuersatz: Some(Decimal::from(19)),
            basiswert: Some(Decimal::new(basis, 2)),
            steuerwert: Some(Decimal::new(steuer, 2)),
            waehrungscode: Some(Waehrungscode::Eur),
            ..Default::default()
        }
    }

    /// PID 31002, January 2026: an Arbeitspreis and a Grundpreis position
    /// (Codeliste der Artikelnummern und Artikel-ID 5.6, Kap. 3.1.2: `1-02-0-002`
    /// €/kWh, `1-02-0-001` €/Tag), both at 19 %.
    pub(crate) fn nn_rechnung() -> Rechnung {
        Rechnung {
            netznutzungrechnungstyp: Some(NetznutzungRechnungstyp::Monatsrechnung),
            rechnungspositionen: Some(vec![
                Rechnungsposition {
                    positionsnummer: Some(1),
                    artikel_id: Some("1-02-0-002".to_owned()),
                    lieferungszeitraum: Some(januar()),
                    positions_menge: Some(Menge {
                        wert: Some(Decimal::from(1000)),
                        einheit: Some(Mengeneinheit::Kwh),
                        ..Default::default()
                    }),
                    einzelpreis: Some(Preis {
                        wert: Some(Decimal::new(8, 2)),
                        ..Default::default()
                    }),
                    gesamtpreis: Some(eur(8000)),
                    ..Default::default()
                },
                Rechnungsposition {
                    positionsnummer: Some(2),
                    artikel_id: Some("1-02-0-001".to_owned()),
                    lieferungszeitraum: Some(januar()),
                    positions_menge: Some(Menge {
                        wert: Some(Decimal::ONE),
                        einheit: Some(Mengeneinheit::Stueck),
                        ..Default::default()
                    }),
                    zeitbezogene_menge: Some(Menge {
                        wert: Some(Decimal::from(31)),
                        einheit: Some(Mengeneinheit::Tag),
                        ..Default::default()
                    }),
                    zeiteinheit: Some(Mengeneinheit::Tag),
                    einzelpreis: Some(Preis {
                        wert: Some(Decimal::new(20, 2)),
                        ..Default::default()
                    }),
                    gesamtpreis: Some(eur(620)),
                    ..Default::default()
                },
            ]),
            gesamtnetto: Some(eur(8620)),
            gesamtsteuer: Some(eur(1638)),
            gesamtbrutto: Some(eur(10258)),
            zu_zahlen: Some(eur(10258)),
            steuerbetraege: Some(vec![steuer(8620, 1638)]),
            ..kopf_der_rechnung("NNE-2026-0001")
        }
    }

    /// The Stornorechnung of `original` as `grid-billing`'s reversal renders
    /// one: its own number and dates, `ist_storno`, the original's number,
    /// and every amount with its sign negated.
    pub(crate) fn stornorechnung(original: &Rechnung) -> Rechnung {
        let neg = |b: &Option<Betrag>| {
            b.clone().map(|b| Betrag {
                wert: b.wert.map(|d| -d),
                ..b
            })
        };
        Rechnung {
            rechnungsnummer: Some("STO-2026-0001".to_owned()),
            rechnungsdatum: Some(time::macros::datetime!(2026-02-20 00:00 UTC)),
            faelligkeitsdatum: Some(time::macros::datetime!(2026-03-20 00:00 UTC)),
            ist_storno: Some(true),
            original_rechnungsnummer: original.rechnungsnummer.clone(),
            gesamtnetto: neg(&original.gesamtnetto),
            gesamtsteuer: neg(&original.gesamtsteuer),
            gesamtbrutto: neg(&original.gesamtbrutto),
            zu_zahlen: neg(&original.zu_zahlen),
            steuerbetraege: original.steuerbetraege.clone().map(|v| {
                v.into_iter()
                    .map(|s| Steuerbetrag {
                        basiswert: s.basiswert.map(|d| -d),
                        steuerwert: s.steuerwert.map(|d| -d),
                        ..s
                    })
                    .collect()
            }),
            rechnungspositionen: None,
            ..original.clone()
        }
    }

    /// PID 31001: one position, the Abschlag — BDEW-Artikelnummer
    /// `9990001000376`, Mengenangabe `H87` (Codeliste der Artikelnummern und
    /// Artikel-ID 5.6, Kap. 2, S. 6).
    pub(crate) fn abschlagsrechnung() -> Rechnung {
        Rechnung {
            netznutzungrechnungstyp: Some(NetznutzungRechnungstyp::Abschlagsrechnung),
            rechnungspositionen: Some(vec![Rechnungsposition {
                positionsnummer: Some(1),
                artikel_id: Some("9990001000376".to_owned()),
                lieferungszeitraum: Some(januar()),
                positions_menge: Some(Menge {
                    wert: Some(Decimal::ONE),
                    einheit: Some(Mengeneinheit::Stueck),
                    ..Default::default()
                }),
                einzelpreis: Some(Preis {
                    wert: Some(Decimal::from(50)),
                    ..Default::default()
                }),
                gesamtpreis: Some(eur(5000)),
                ..Default::default()
            }]),
            gesamtnetto: Some(eur(5000)),
            gesamtsteuer: Some(eur(950)),
            gesamtbrutto: Some(eur(5950)),
            zu_zahlen: Some(eur(5950)),
            steuerbetraege: Some(vec![steuer(5000, 950)]),
            ..kopf_der_rechnung("ABS-2026-0001")
        }
    }

    /// The Marktlokation as an Entnahmestelle — what `IMD` DE 7009 states on
    /// the MMM-Rechnung.
    fn entnahme() -> Box<Marktlokation> {
        Box::new(Marktlokation {
            marktlokations_id: Some(MALO.parse().expect("valid MaLo-ID")),
            lokationsadresse: Some(adresse("Lindenallee", "12", "10115", "Berlin")),
            energierichtung: Some(rubo4e::current::Energierichtung::Aussp),
            ..Default::default()
        })
    }

    /// One position at 19 % and its Summenteil.
    fn eine_position(nummer: &str, p: Rechnungsposition, netto: i64, steuer_ct: i64) -> Rechnung {
        Rechnung {
            rechnungspositionen: Some(vec![p]),
            gesamtnetto: Some(eur(netto)),
            gesamtsteuer: Some(eur(steuer_ct)),
            gesamtbrutto: Some(eur(netto + steuer_ct)),
            zu_zahlen: Some(eur(netto + steuer_ct)),
            steuerbetraege: Some(vec![steuer(netto, steuer_ct)]),
            ..kopf_der_rechnung(nummer)
        }
    }

    fn kwh_position(artikel: &str, kwh: i64, ct: i64, netto: i64) -> Rechnungsposition {
        Rechnungsposition {
            positionsnummer: Some(1),
            artikel_id: Some(artikel.to_owned()),
            lieferungszeitraum: Some(januar()),
            positions_menge: Some(Menge {
                wert: Some(Decimal::from(kwh)),
                einheit: Some(Mengeneinheit::Kwh),
                ..Default::default()
            }),
            einzelpreis: Some(Preis {
                wert: Some(Decimal::new(ct, 2)),
                ..Default::default()
            }),
            gesamtpreis: Some(eur(netto)),
            ..Default::default()
        }
    }

    /// PID 31005: the Mindermenge of January — BDEW-Artikelnummer
    /// `9990001000756` (Codeliste der Artikelnummern und Artikel-ID 5.6,
    /// Kap. 2, S. 7), 400 kWh at 12 ct.
    pub(crate) fn mmm_rechnung() -> Rechnung {
        Rechnung {
            netznutzungrechnungstyp: Some(NetznutzungRechnungstyp::Mehrmindermengenrechnung),
            marktlokation: Some(entnahme()),
            ..eine_position(
                "MMM-2026-0001",
                kwh_position("9990001000756", 400, 12, 4800),
                4800,
                912,
            )
        }
    }

    /// PID 31006: the Mehrmenge of January as the selbst ausgestellte
    /// Rechnung — BDEW-Artikelnummer `9990001000748` (Kap. 2, S. 7), 500 kWh
    /// at 10 ct, `BGM` 389.
    pub(crate) fn mmm_selbst_ausgestellt() -> Rechnung {
        Rechnung {
            netznutzungrechnungstyp: Some(NetznutzungRechnungstyp::Mehrmindermengenrechnung),
            netznutzungrechnungsart: Some(NetznutzungRechnungsart::Selbstausgestellt),
            marktlokation: Some(entnahme()),
            ..eine_position(
                "MMS-2026-0001",
                kwh_position("9990001000748", 500, 10, 5000),
                5000,
                950,
            )
        }
    }

    /// PID 31009, MSB → NB: the Preisobergrenze of a verbrauchende
    /// Marktlokation `]6.000 kWh/a; 10.000 kWh/a]` mit iMS for January —
    /// Artikel-ID `4-03-0-006`, €/Tag (Codeliste der Artikelnummern und
    /// Artikel-ID 5.6, Kap. 3.5.2, S. 55), 31 days at 0,10 €.
    pub(crate) fn msb_rechnung() -> Rechnung {
        let p = Rechnungsposition {
            positionsnummer: Some(1),
            artikel_id: Some("4-03-0-006".to_owned()),
            lieferungszeitraum: Some(januar()),
            positions_menge: Some(Menge {
                wert: Some(Decimal::ONE),
                einheit: Some(Mengeneinheit::Stueck),
                ..Default::default()
            }),
            zeitbezogene_menge: Some(Menge {
                wert: Some(Decimal::from(31)),
                einheit: Some(Mengeneinheit::Tag),
                ..Default::default()
            }),
            zeiteinheit: Some(Mengeneinheit::Tag),
            einzelpreis: Some(Preis {
                wert: Some(Decimal::new(10, 2)),
                ..Default::default()
            }),
            gesamtpreis: Some(eur(310)),
            ..Default::default()
        };
        Rechnung {
            netznutzungrechnungsart: None,
            rechnungstyp: Some(Rechnungstyp::Messstellenbetriebsrechnung),
            ..eine_position("MSB-2026-0001", p, 310, 59)
        }
    }

    /// PID 31011, GNB → LF: one Unterbrechung der Anschlussnutzung in der
    /// regulären Arbeitszeit — Artikel-ID `2-01-7-001`, €/Auftrag (Codeliste
    /// der Artikelnummern und Artikel-ID 5.6, Kap. 3.2, S. 47), 60 €.
    pub(crate) fn sperr_rechnung() -> Rechnung {
        let p = Rechnungsposition {
            positionsnummer: Some(1),
            artikel_id: Some("2-01-7-001".to_owned()),
            positions_menge: Some(Menge {
                wert: Some(Decimal::ONE),
                einheit: Some(Mengeneinheit::Stueck),
                ..Default::default()
            }),
            einzelpreis: Some(Preis {
                wert: Some(Decimal::from(60)),
                ..Default::default()
            }),
            gesamtpreis: Some(eur(6000)),
            ..Default::default()
        };
        Rechnung {
            netznutzungrechnungsart: None,
            rechnungstyp: None,
            sparte: Some(Sparte::Gas),
            // A Rechnung für Sperren states the Ausführungsdatum, not a
            // Zeitraum: the column lists 00007/00008 and 00038/00039 only
            // `[53]`, on the Blindarbeitrechnung.
            rechnungsperiode: None,
            ..eine_position("AWH-2026-0001", p, 6000, 1140)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use time::macros::{date, datetime};

    fn kopf(pid: u32) -> Kopf<'static> {
        Kopf {
            pid,
            sender: NB,
            receiver: LF,
            message_ref: "INV1",
            buchungsdatum: datetime!(2026-02-15 09:00 UTC),
            zugehoeriges_dokument: Some("MSCONSLS0001"),
            preise_mp_id: Some(NB),
            rechnungstyp: None,
            ausfuehrungsdatum: None,
        }
    }

    fn fv20261001() -> &'static Profile {
        profil(date!(2026 - 11 - 06)).expect("INVOIC in force")
    }

    fn segments(nachricht: &str) -> Vec<OwnedSegment> {
        edifact_rs::from_bytes(nachricht.as_bytes())
            .map(|s| s.map(edifact_rs::Segment::into_owned))
            .collect::<Result<Vec<_>, _>>()
            .expect("the built message parses")
    }

    fn wert(b: Option<&rubo4e::current::Betrag>) -> Option<Decimal> {
        b.and_then(|b| b.wert)
    }

    /// What the inbound adapter reads back is what was issued.
    fn assert_round_trip(issued: &Rechnung, read: &Rechnung, vorzeichen: Decimal) {
        let neg = |d: Option<Decimal>| d.map(|d| d * vorzeichen);
        assert_eq!(
            read.rechnungsperiode
                .as_ref()
                .map(|z| (z.startdatum, z.enddatum)),
            issued
                .rechnungsperiode
                .as_ref()
                .map(|z| (z.startdatum, z.enddatum)),
            "period"
        );
        assert_eq!(
            wert(read.gesamtnetto.as_ref()),
            neg(wert(issued.gesamtnetto.as_ref())),
            "net"
        );
        assert_eq!(
            wert(read.gesamtsteuer.as_ref()),
            neg(wert(issued.gesamtsteuer.as_ref())),
            "tax"
        );
        assert_eq!(
            wert(read.gesamtbrutto.as_ref()),
            neg(wert(issued.gesamtbrutto.as_ref())),
            "gross"
        );
        assert_eq!(
            wert(read.zu_zahlen.as_ref()),
            neg(wert(issued.zu_zahlen.as_ref())),
            "fällig"
        );
        let steuer = |r: &Rechnung, v: Decimal| -> Vec<_> {
            r.steuerbetraege
                .iter()
                .flatten()
                .map(|s| {
                    (
                        s.steuerart,
                        s.steuersatz,
                        s.basiswert.map(|d| d * v),
                        s.steuerwert.map(|d| d * v),
                    )
                })
                .collect()
        };
        assert_eq!(
            steuer(read, Decimal::ONE),
            steuer(issued, vorzeichen),
            "Steuerbeträge"
        );
        assert_eq!(
            read.netznutzungrechnungstyp, issued.netznutzungrechnungstyp,
            "IMD"
        );
    }

    fn assert_positions(issued: &Rechnung, read: &Rechnung) {
        let pos = |r: &Rechnung| -> Vec<_> {
            r.rechnungspositionen
                .iter()
                .flatten()
                .map(|p| {
                    (
                        p.positionsnummer,
                        p.artikel_id.clone(),
                        p.lieferungszeitraum
                            .as_ref()
                            .map(|z| (z.startdatum, z.enddatum)),
                        p.positions_menge.as_ref().map(|m| (m.wert, m.einheit)),
                        p.zeitbezogene_menge.as_ref().map(|m| (m.wert, m.einheit)),
                        p.zeiteinheit,
                        p.einzelpreis.as_ref().and_then(|e| e.wert),
                        wert(p.gesamtpreis.as_ref()),
                    )
                })
                .collect()
        };
        assert_eq!(pos(read), pos(issued), "positions");
    }

    fn findings(m: &AusgehendeInvoic, pid: u32) -> Vec<String> {
        fv20261001()
            .validate(
                &segments(&m.nachricht),
                edi_energy::Pruefidentifikator::new(pid).ok(),
            )
            .into_iter()
            .filter(refuses)
            .map(|i| i.message)
            .collect()
    }

    /// 31002: an NN-Rechnung with two positions at 19 % renders,
    /// validates with no error against `invoic/fv20261001`, and reads back
    /// through makod's inbound adapter as the Rechnung that was issued.
    #[test]
    fn a_31002_with_two_positions_at_19_percent_validates_and_round_trips() {
        let issued = nn_rechnung();
        let m = rechnung(fv20261001(), &issued, &kopf(31002)).expect("renders");
        assert!(findings(&m, 31002).is_empty(), "{:?}", findings(&m, 31002));
        // The qualifiers are the profile's: the position's net is `MOA+203`,
        // the Rechnungsbetrag `MOA+77`.
        assert!(m.nachricht.contains("MOA+203:80'"), "{}", m.nachricht);
        assert!(m.nachricht.contains("MOA+77:102.58'"), "{}", m.nachricht);
        let read = crate::orchestrator::adapters::build_rechnung(&segments(&m.nachricht));
        assert_eq!(read.rechnungsnummer, issued.rechnungsnummer);
        assert_eq!(read.rechnungsdatum, issued.rechnungsdatum);
        assert_eq!(read.faelligkeitsdatum, issued.faelligkeitsdatum);
        assert!(
            m.nachricht.contains("DTM+265:202603142300?+00:303'"),
            "{}",
            m.nachricht
        );
        assert_round_trip(&issued, &read, Decimal::ONE);
        assert_positions(&issued, &read);
    }

    /// 31001: the Abschlag is one position, the Artikelnummer `Z01` the
    /// column admits alone, and the vorläufiger Abrechnungszeitraum
    /// (`DTM+Z42`/`Z43`).
    #[test]
    fn a_31001_abschlagsrechnung_validates_and_round_trips() {
        let issued = abschlagsrechnung();
        let k = Kopf {
            zugehoeriges_dokument: None,
            preise_mp_id: None,
            ..kopf(31001)
        };
        let m = rechnung(fv20261001(), &issued, &k).expect("renders");
        assert!(findings(&m, 31001).is_empty(), "{:?}", findings(&m, 31001));
        assert!(
            m.nachricht.contains("LIN+1++9990001000376:Z01'"),
            "{}",
            m.nachricht
        );
        assert!(
            m.nachricht.contains("DTM+Z42:202512312300?+00:303'"),
            "{}",
            m.nachricht
        );
        let read = crate::orchestrator::adapters::build_rechnung(&segments(&m.nachricht));
        assert_eq!(read.rechnungsnummer, issued.rechnungsnummer);
        assert_round_trip(&issued, &read, Decimal::ONE);
        assert_positions(&issued, &read);
    }

    /// Render `issued` under `k`, require no finding against
    /// `invoic/fv20261001`, and read it back through the inbound adapter.
    fn renders_validates_and_round_trips(issued: &Rechnung, k: &Kopf<'_>) -> String {
        let m = rechnung(fv20261001(), issued, k).unwrap_or_else(|e| panic!("renders: {e}"));
        assert!(findings(&m, k.pid).is_empty(), "{:?}", findings(&m, k.pid));
        let read = crate::orchestrator::adapters::build_rechnung(&segments(&m.nachricht));
        assert_eq!(read.rechnungsnummer, issued.rechnungsnummer);
        assert_eq!(read.rechnungsdatum, issued.rechnungsdatum);
        assert_eq!(read.faelligkeitsdatum, issued.faelligkeitsdatum);
        assert_round_trip(issued, &read, Decimal::ONE);
        assert_positions(issued, &read);
        m.nachricht
    }

    /// 31005: the Mindermenge goes out as its BDEW-Artikelnummer (`Z01`),
    /// `IMD++MMM`, with no `RFF+ACE` (the column asks for it only `[17]` with
    /// a `DTM+Z11`).
    #[test]
    fn a_31005_mmm_rechnung_validates_and_round_trips() {
        let k = Kopf {
            zugehoeriges_dokument: None,
            preise_mp_id: None,
            ..kopf(31005)
        };
        let n = renders_validates_and_round_trips(&mmm_rechnung(), &k);
        assert!(n.contains("LIN+1++9990001000756:Z01'"), "{n}");
        assert!(n.contains("IMD++MMM+Z07'"), "{n}");
    }

    /// 31005 with a Mehrmenge: `[14]` adds the Korrekturfaktor `QTY+Z17:-1`,
    /// the price goes out positive, the net negative — and reads back as the
    /// negative rate it was issued with.
    #[test]
    fn a_31005_mehrmenge_carries_the_korrekturfaktor() {
        let mut r = mmm_selbst_ausgestellt();
        r.netznutzungrechnungsart = Some(NetznutzungRechnungsart::Handelsrechnung);
        let neg = |b: Option<rubo4e::current::Betrag>| {
            b.map(|b| rubo4e::current::Betrag {
                wert: b.wert.map(|d| -d),
                ..b
            })
        };
        // As `grid-billing` states a credit: the rate negative, the net with it.
        if let Some(p) = r.rechnungspositionen.as_mut().and_then(|p| p.first_mut()) {
            p.gesamtpreis = neg(p.gesamtpreis.take());
            p.einzelpreis = p.einzelpreis.take().map(|e| rubo4e::current::Preis {
                wert: e.wert.map(|d| -d),
                ..e
            });
        }
        r.gesamtnetto = neg(r.gesamtnetto.take());
        r.gesamtsteuer = neg(r.gesamtsteuer.take());
        r.gesamtbrutto = neg(r.gesamtbrutto.take());
        r.zu_zahlen = neg(r.zu_zahlen.take());
        r.steuerbetraege = r.steuerbetraege.map(|v| {
            v.into_iter()
                .map(|s| Steuerbetrag {
                    basiswert: s.basiswert.map(|d| -d),
                    steuerwert: s.steuerwert.map(|d| -d),
                    ..s
                })
                .collect()
        });
        let k = Kopf {
            zugehoeriges_dokument: None,
            preise_mp_id: None,
            ..kopf(31005)
        };
        let n = renders_validates_and_round_trips(&r, &k);
        assert!(n.contains("QTY+47:500:KWH'QTY+Z17:-1'"), "{n}");
        assert!(n.contains("MOA+203:-50'PRI+CAL:0.1'"), "{n}");
    }

    /// 31006: `BGM+389`, and the Empfänger's Umsatzsteuer-ID, which the
    /// column requires on the selbst ausgestellte Rechnung unconditionally.
    #[test]
    fn a_31006_selbst_ausgestellte_mmm_rechnung_validates_and_round_trips() {
        let k = Kopf {
            zugehoeriges_dokument: None,
            preise_mp_id: None,
            ..kopf(31006)
        };
        let n = renders_validates_and_round_trips(&mmm_selbst_ausgestellt(), &k);
        assert!(n.contains("BGM+389+MMS-2026-0001+9'"), "{n}");
        assert!(n.contains("RFF+VA:DE987654321'"), "{n}");
        assert!(!n.contains("QTY+Z17"), "{n}");
    }

    /// 31009, MSB → NB: `IMD++MSB` as the command states it — BO4E has no
    /// Rechnungstyp for it — and the Artikel-ID as `Z09`.
    #[test]
    fn a_31009_msb_rechnung_validates_and_round_trips() {
        let k = Kopf {
            sender: LF,
            receiver: NB,
            zugehoeriges_dokument: None,
            preise_mp_id: None,
            rechnungstyp: Some("MSB"),
            ..kopf(31009)
        };
        let n = renders_validates_and_round_trips(&msb_rechnung(), &k);
        assert!(n.contains("IMD++MSB'"), "{n}");
        assert!(n.contains("LIN+1++4-03-0-006:Z09'"), "{n}");
        assert!(n.contains("PRI+CAL:0.1::::DAY'"), "{n}");
    }

    /// 31011: `IMD++Z43` and the Sperrauftrag's ORDERS in `RFF+ACE`
    /// (Muss `[47]`).
    #[test]
    fn a_31011_rechnung_fuer_sperren_validates_and_round_trips() {
        let k = Kopf {
            sender: GNB,
            receiver: LFG,
            zugehoeriges_dokument: Some("ORDERS0001"),
            preise_mp_id: None,
            rechnungstyp: Some("Z43"),
            ausfuehrungsdatum: Some(date!(2026 - 01 - 14)),
            ..kopf(31011)
        };
        let n = renders_validates_and_round_trips(&sperr_rechnung(), &k);
        assert!(n.contains("DTM+203:20260114:102'IMD++Z43'"), "{n}");
        assert!(n.contains("IMD++Z43'"), "{n}");
        assert!(n.contains("RFF+ACE:ORDERS0001'"), "{n}");
        assert!(n.contains("LIN+1++2-01-7-001:Z09'"), "{n}");
        assert!(n.contains("NAD+MS+9800357000004::332"), "{n}");
    }

    /// A Rechnungstyp the Rechnung states and a different one the command
    /// states are refused at 00014 — `IMD` carries one.
    #[test]
    fn two_rechnungstypen_are_refused_naming_00014() {
        let k = Kopf {
            rechnungstyp: Some("MSB"),
            ..kopf(31002)
        };
        let e = rechnung(fv20261001(), &nn_rechnung(), &k).expect_err("refused");
        assert_eq!(e.places(), vec!["00014"], "{e}");
    }

    /// 31004: `BGM+457`, the Ursprungsrechnung in `RFF+OI` with its
    /// `DTM+137` as `DTM+171`, the Summenteil the negation of the original's
    /// as the Stornorechnung states it, no position.
    #[test]
    fn the_storno_references_its_original_and_negates_every_amount() {
        let original = nn_rechnung();
        let s = stornorechnung(&original);
        let m = storno(fv20261001(), &s, original.rechnungsdatum, &kopf(31002)).expect("renders");
        assert!(
            findings(&m, STORNO_PID).is_empty(),
            "{:?}",
            findings(&m, STORNO_PID)
        );
        assert!(
            m.nachricht.contains("BGM+457+STO-2026-0001+9'"),
            "{}",
            m.nachricht
        );
        assert!(
            m.nachricht
                .contains("RFF+OI:NNE-2026-0001'DTM+171:202602150000?+00:303'"),
            "{}",
            m.nachricht
        );
        assert!(m.nachricht.contains("MOA+77:-102.58'"), "{}", m.nachricht);
        assert!(!m.nachricht.contains("LIN+"), "{}", m.nachricht);
        let read = crate::orchestrator::adapters::build_rechnung(&segments(&m.nachricht));
        assert_eq!(read.rechnungsnummer.as_deref(), Some("STO-2026-0001"));
        assert_eq!(read.ist_storno, Some(true));
        assert_eq!(read.original_rechnungsnummer, original.rechnungsnummer);
        // Read back, every amount is the original's with its sign negated.
        assert_round_trip(&original, &read, Decimal::NEGATIVE_ONE);
    }

    /// A Storno whose Ursprungsrechnung's date is not known is refused at
    /// 00019 rather than sent with an invented one.
    #[test]
    fn a_storno_without_the_originals_date_is_refused_naming_00019() {
        let e = storno(
            fv20261001(),
            &stornorechnung(&nn_rechnung()),
            None,
            &kopf(31002),
        )
        .expect_err("refused");
        assert_eq!(e.places(), vec!["00019"], "{e}");
    }

    /// A Rechnung without a Rechnungsnummer is refused naming 00004 —
    /// the completion would have made one up.
    #[test]
    fn a_rechnung_without_a_rechnungsnummer_is_refused_naming_00004() {
        let r = Rechnung {
            rechnungsnummer: None,
            ..nn_rechnung()
        };
        let e = rechnung(fv20261001(), &r, &kopf(31002)).expect_err("refused");
        assert!(matches!(e, InvoicAusgangError::Unstated(_)), "{e}");
        assert_eq!(e.places(), vec!["00004"], "{e}");
    }

    /// A Rechnung that names no Marktlokation is refused at 00028/00029, the
    /// Lieferanschrift the column requires — not sent with an invented one.
    #[test]
    fn a_rechnung_without_its_marktlokation_is_refused_naming_the_place() {
        let r = Rechnung {
            marktlokation: None,
            ..nn_rechnung()
        };
        let e = rechnung(fv20261001(), &r, &kopf(31002)).expect_err("refused");
        let places = e.places();
        assert!(
            places.contains(&"00028".to_owned()) && places.contains(&"00029".to_owned()),
            "{e}"
        );
    }

    /// A value longer than its data element is refused with its place, never
    /// truncated (`NAD` DE 3036 is `an..35`).
    #[test]
    fn a_value_longer_than_its_data_element_is_refused_not_truncated() {
        let mut r = nn_rechnung();
        if let Some(gp) = r.rechnungsersteller.as_deref_mut() {
            gp.organisationsname = Some("Stadtwerke Musterstadt Netzgesellschaft mbH".to_owned());
        }
        let e = rechnung(fv20261001(), &r, &kopf(31002)).expect_err("refused");
        assert!(matches!(e, InvoicAusgangError::Invalid(_)), "{e}");
        assert!(e.places().contains(&"00022".to_owned()), "{e}");
    }

    /// The interchange is UNOC: „Köln" goes out, a character outside
    /// ISO 8859-1 is refused at the command, not at dispatch.
    #[test]
    fn a_character_outside_unoc_is_refused_at_the_command() {
        let r = nn_rechnung();
        rechnung(fv20261001(), &r, &kopf(31002)).expect("Köln is in ISO 8859-1");
        let mut r = nn_rechnung();
        if let Some(gp) = r.rechnungsersteller.as_deref_mut() {
            gp.organisationsname = Some("Netz Łódź".to_owned());
        }
        let e = rechnung(fv20261001(), &r, &kopf(31002)).expect_err("refused");
        assert!(e.to_string().contains("UNOC"), "{e}");
    }

    /// An amount goes out as stated. One with more places than `[930]`
    /// admits is refused, not rounded to fit.
    #[test]
    fn an_amount_with_three_decimals_is_refused_not_rounded() {
        let mut r = nn_rechnung();
        if let Some(p) = r.rechnungspositionen.as_mut().and_then(|p| p.first_mut()) {
            p.gesamtpreis = Some(rubo4e::current::Betrag {
                wert: Some(Decimal::new(80_005, 3)),
                ..Default::default()
            });
        }
        let e = rechnung(fv20261001(), &r, &kopf(31002)).expect_err("refused");
        assert!(e.places().contains(&"00041".to_owned()), "{e}");
        // Trailing zeros are no extra precision: `80.00000` is `80`.
        let mut r = nn_rechnung();
        if let Some(p) = r.rechnungspositionen.as_mut().and_then(|p| p.first_mut()) {
            p.gesamtpreis = Some(rubo4e::current::Betrag {
                wert: Some(Decimal::new(8_000_000, 5)),
                ..Default::default()
            });
        }
        let m = rechnung(fv20261001(), &r, &kopf(31002)).expect("renders");
        assert!(m.nachricht.contains("MOA+203:80'"), "{}", m.nachricht);
    }

    /// A position that names only the BO4E Artikelnummer enum carries no
    /// BDEW code, so it is refused at 00034 rather than coded by guesswork.
    #[test]
    fn a_bo4e_artikelnummer_without_its_code_is_refused() {
        let mut r = nn_rechnung();
        if let Some(p) = r.rechnungspositionen.as_mut().and_then(|p| p.first_mut()) {
            p.artikel_id = None;
            p.artikelnummer = Some(rubo4e::current::BdewArtikelnummer::Abschlag);
        }
        let e = rechnung(fv20261001(), &r, &kopf(31002)).expect_err("refused");
        assert_eq!(e.places(), vec!["00034"], "{e}");
    }
}
