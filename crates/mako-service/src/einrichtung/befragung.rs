//! The setup interview and the configuration it proposes.
//!
//! [`vorschlag`] turns the operator's [`Antworten`] into one file per daemon:
//! each line the configuration needs, marked against what the daemons report
//! as loaded ([`Art::Gleich`], [`Art::Neu`], [`Art::Weg`]), every line naming
//! the rule or document it satisfies, and every fact nobody gave a
//! [`Art::Fehlt`] gap — a `# FEHLT: …` comment, so the file still parses and
//! the daemon still refuses until the gap is filled. Nothing is guessed.
//!
//! The proposal holds **no secret**: a certificate, a key and an API key are
//! named by the path they live at, and an answer that carries key material
//! itself is refused rather than written. An answer that breaks an identifier
//! rule — a Gas Rolle under a `99…` BDEW-Codenummer — is refused naming the
//! rule and becomes a gap.
//!
//! The function is total and deterministic: the same answers and the same
//! loaded reports give the same files, byte for byte. Nobody writes them to a
//! server; the operator applies them.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Antwort, DienstBericht};
use crate::befund::{Befund, Bereich};

// ── Sources ──────────────────────────────────────────────────────────────────

/// One MP-ID per Rolle and Sparte.
const EINE_MP_ID: &str = "BDEW-AWH Identifikatoren V1.2, Kap. 2.1 (S. 4)";

/// Positions 1+2 of a Codenummer name its issuer and so its Sparte.
const CODENUMMER: &str = "BDEW-AWH Identifikatoren V1.2, Kap. 2.2 (S. 5): Stelle 1+2 \
     „99 = BDEW/Strom“, „98 = DVGW/Gas“";

/// A Codenummer names one Energieart.
const SPARTEN_GETRENNT: &str = "Allgemeine Festlegungen V6.1d, Kap. 2.13";

/// Each MP-ID signs, decrypts and connects with its own key pairs.
const AS4_SCHLUESSEL: &str = "BDEW AS4-Profil 1.2, Kap. 2.3.4.4; Regelungen zum \
     Übertragungsweg für AS4 2.6, Kap. 5.2";

/// The Zuordnungsermächtigung is granted per ZRT, BG, BK and LF.
const BILANZKREIS_STROM: &str = "BK6-24-174 Anlage 3 (MaBiS), Kap. 10.2.1";

const PRIMAER: &str = "makod [[party]] primary: genau eine MP-ID ist der Speicherschlüssel \
     aller Ereignisströme";
const ROLLEN: &str = "makod [deployment] rollen: jede erklärte Rolle braucht genau eine \
     [[party]]-MP-ID";
const EIGENE_MP_ID: &str = "processd [identity] own_mp_id: die MP-ID, die in makod \
     primary = true trägt";
const VERBUNDENE: &str = "processd [identity] verbundene_lieferanten: Pflichtangabe, [] \
     eingeschlossen; jede Rolle hat ihre eigene MP-ID (BDEW-AWH Identifikatoren V1.2, \
     Kap. 2.1)";
const BILANZKREIS_GAS: &str = "processd [[lf.bilanzkreise]]: eine Zeile je Sparte; eine \
     Gas-Zeile nennt nur standard";
const DIENST: &str = "makod [[einrichtung.dienst]]: die Checkliste fragt jeden Dienst nach \
     GET /admin/einrichtung";
const SCHLUESSELDATEI: &str = "nur der Pfad: der Schlüssel bleibt in seiner Datei";
const MESSAGE_ID: &str = "makod [as4] message_id_domain: jede eb:MessageId ist \
     <id>@<domain> (RFC 2822 §3.6.4)";
const VERTRAUENSANKER: &str = "makod [as4] trust_anchor_pem_file: die SM-PKI-Wurzel";
const SPERRLISTE: &str = "makod [as4] crl_signer_pem_file: der Aussteller der Sperrliste \
     (CCRL-S(Root))";

// ── The interview ────────────────────────────────────────────────────────────

/// A Sparte.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Sparte {
    /// Strom.
    Strom,
    /// Gas.
    Gas,
}

impl Sparte {
    /// `strom` / `gas`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strom => "strom",
            Self::Gas => "gas",
        }
    }
}

/// A Rolle a deployment plays, spelled as makod's `[deployment] rollen`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Rolle {
    /// Lieferant Strom.
    LfStrom,
    /// Lieferant Gas.
    LfGas,
    /// Netzbetreiber Strom.
    NbStrom,
    /// Gasnetzbetreiber.
    NbGas,
    /// Messstellenbetreiber Strom.
    MsbStrom,
    /// Messstellenbetreiber Gas.
    MsbGas,
    /// Energieserviceanbieter, Strom only.
    EsaStrom,
}

impl Rolle {
    /// Every Rolle, in the order a proposal lists them.
    pub const ALLE: [Self; 7] = [
        Self::LfStrom,
        Self::LfGas,
        Self::NbStrom,
        Self::NbGas,
        Self::MsbStrom,
        Self::MsbGas,
        Self::EsaStrom,
    ];

    /// The `[deployment] rollen` token.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::LfStrom => "lf-strom",
            Self::LfGas => "lf-gas",
            Self::NbStrom => "nb-strom",
            Self::NbGas => "nb-gas",
            Self::MsbStrom => "msb-strom",
            Self::MsbGas => "msb-gas",
            Self::EsaStrom => "esa-strom",
        }
    }

    /// The `[[party]] roles` abbreviation a proposal writes.
    #[must_use]
    pub const fn kuerzel(self) -> &'static str {
        match self {
            Self::LfStrom => "LF",
            Self::LfGas => "LFG",
            Self::NbStrom => "NB",
            Self::NbGas => "GNB",
            Self::MsbStrom => "MSB",
            Self::MsbGas => "GMSB",
            Self::EsaStrom => "ESA",
        }
    }

    /// The Rolle a `[[party]] roles` abbreviation names, the AHB
    /// sub-qualifiers (`ANB`, `VNB`, `NMSB`, `AMSB`) included.
    #[must_use]
    pub fn von_kuerzel(kuerzel: &str) -> Option<Self> {
        match kuerzel.to_uppercase().as_str() {
            "LF" => Some(Self::LfStrom),
            "LFG" => Some(Self::LfGas),
            "NB" | "ANB" | "VNB" => Some(Self::NbStrom),
            "GNB" => Some(Self::NbGas),
            "MSB" | "NMSB" | "AMSB" => Some(Self::MsbStrom),
            "GMSB" => Some(Self::MsbGas),
            "ESA" => Some(Self::EsaStrom),
            _ => None,
        }
    }

    /// The Rolle a `[deployment] rollen` token names.
    #[must_use]
    pub fn von_token(token: &str) -> Option<Self> {
        Self::ALLE.into_iter().find(|r| r.token() == token.trim())
    }

    /// Its Sparte.
    #[must_use]
    pub const fn sparte(self) -> Sparte {
        match self {
            Self::LfGas | Self::NbGas | Self::MsbGas => Sparte::Gas,
            _ => Sparte::Strom,
        }
    }

    /// A Lieferant, whose answers name a Bilanzkreis.
    #[must_use]
    pub const fn ist_lieferant(self) -> bool {
        matches!(self, Self::LfStrom | Self::LfGas)
    }
}

/// One Rolle the deployment plays and the MP-ID it plays it under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RolleAntwort {
    /// The Rolle.
    pub rolle: Rolle,
    /// Its MP-ID: a 13-digit BDEW- (`99…`, Strom) or DVGW-Codenummer (`98…`,
    /// Gas), a GS1 GLN, or a 16-character EIC. Unanswered, it is a gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mp_id: Option<String>,
}

/// The Bilanzkreise a Lieferant assigns to in one Sparte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BilanzkreisAntwort {
    /// The Sparte.
    pub sparte: Sparte,
    /// The ordinary Bilanzkreise, as the BKV's grant names them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub standard: Vec<String>,
}

/// One MP-ID's AS4 key material, by the path each file lives at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct As4Schluessel {
    /// The MP-ID these files belong to.
    pub mp_id: String,
    /// Path of the signing certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_cert_pem_file: Option<String>,
    /// Path of the signing key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_key_pem_file: Option<String>,
    /// Path of the encryption certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption_cert_pem_file: Option<String>,
    /// Path of the encryption key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption_key_pem_file: Option<String>,
    /// Path of the TLS certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_cert_pem_file: Option<String>,
    /// Path of the TLS key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_key_pem_file: Option<String>,
}

/// AS4: the deployment-wide settings and each MP-ID's key material.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct As4Antwort {
    /// The domain every `eb:MessageId` ends in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id_domain: Option<String>,
    /// Path of the SM-PKI root certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_anchor_pem_file: Option<String>,
    /// Path of the CRL signer's certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crl_signer_pem_file: Option<String>,
    /// Per MP-ID, the paths of its key pairs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schluessel: Vec<As4Schluessel>,
}

/// Another daemon of the deployment and where it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DienstAntwort {
    /// Its name (`processd`, `edmd`, …).
    pub name: String,
    /// Its base URL.
    pub url: String,
    /// Path of the file holding the key its `read-config` admits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_file: Option<String>,
}

/// The setup interview's answers. Every question is optional: an unanswered
/// one is a gap in the proposal, never a default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Antworten {
    /// The Rollen the deployment plays, each with its MP-ID.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rollen: Vec<RolleAntwort>,
    /// The MP-ID that is the storage key, where several are named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primaer: Option<String>,
    /// A Lieferant's Bilanzkreise per Sparte.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bilanzkreise: Vec<BilanzkreisAntwort>,
    /// The MP-IDs of the operator's affiliated Lieferanten; `[]` for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbundene_lieferanten: Option<Vec<String>>,
    /// AS4 key material; unanswered, each `[[party]]` carries a gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as4: Option<As4Antwort>,
    /// The deployment's other daemons, for the checklist.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dienste: Vec<DienstAntwort>,
}

// ── The proposal ─────────────────────────────────────────────────────────────

/// What a proposed line does to the loaded configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Art {
    /// A table header (`[deployment]`, `[[party]]`).
    Abschnitt,
    /// Loaded already, as proposed.
    Gleich,
    /// To add.
    Neu,
    /// Loaded, and to remove.
    Weg,
    /// A fact nobody gave: a `# FEHLT: …` comment.
    Fehlt,
}

/// One line of a proposed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Zeile {
    /// What it does.
    pub art: Art,
    /// The TOML line, or for a gap the comment.
    pub text: String,
    /// The rule or document the line satisfies; absent only on a header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quelle: Option<String>,
    /// The interview question the line answers (`rollen.lf-gas`,
    /// `bilanzkreise.gas`, `as4`, …), so a gap leads to its question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frage: Option<String>,
}

/// One daemon's proposed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Datei {
    /// The daemon.
    pub dienst: String,
    /// Its configuration file.
    pub datei: String,
    /// The daemon reports the facts the file sets, so [`Art::Gleich`] and
    /// [`Art::Weg`] are known; otherwise every line is [`Art::Neu`].
    pub abgeglichen: bool,
    /// Every line, in file order.
    pub zeilen: Vec<Zeile>,
    /// The file to paste: each line under its `# Quelle:`, a removed line as
    /// `# ENTFÄLLT:`, a gap as `# FEHLT:`.
    pub toml: String,
    /// The change as a diff: `+` new, `-` removed, ` ` unchanged.
    pub diff: String,
    /// Lines to add or remove.
    pub aenderungen: u32,
    /// Gaps.
    pub luecken: u32,
}

/// The proposal: a file per daemon and every answer refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Vorschlag {
    /// One file per daemon the answers configure.
    pub dateien: Vec<Datei>,
    /// Every answer refused, at its path in the answers (`rollen[1].mp_id`),
    /// with the rule it breaks.
    pub abgelehnt: Vec<Befund>,
    /// Gaps across all files.
    pub luecken: u32,
    /// No gap and no refused answer.
    pub vollstaendig: bool,
    /// The answers the loaded configuration already gives — where an
    /// interview starts.
    pub vorbelegt: Antworten,
}

// ── Building it ──────────────────────────────────────────────────────────────

/// What the loaded configuration says, from the reports.
#[derive(Default)]
struct Geladen {
    /// makod answered.
    makod: bool,
    /// MP-ID → the Rollen makod acts as under it.
    parteien: BTreeMap<String, BTreeSet<Rolle>>,
    /// `[deployment] rollen`, from makod's policy report.
    rollen: Option<BTreeSet<Rolle>>,
    /// `[[einrichtung.dienst]]`: (name, url).
    dienste: Vec<(String, String)>,
}

fn geladen_aus(berichte: &[DienstBericht]) -> Geladen {
    let mut g = Geladen::default();
    for b in berichte {
        if !b.url.is_empty() {
            g.dienste.push((b.dienst.clone(), b.url.clone()));
        }
        let Antwort::Erreicht { einrichtung } = &b.antwort else {
            continue;
        };
        if einrichtung.dienst != "makod" {
            continue;
        }
        g.makod = true;
        for p in &einrichtung.parteien {
            if let Some(r) = Rolle::von_kuerzel(&p.rolle) {
                g.parteien.entry(p.mp_id.clone()).or_default().insert(r);
            }
        }
        g.rollen = einrichtung
            .policy
            .get("deployment.rollen")
            .map(|e| e.value.split(',').filter_map(Rolle::von_token).collect());
    }
    g
}

/// The answers the loaded configuration gives.
fn vorbelegen(g: &Geladen) -> Antworten {
    let mut rollen: BTreeMap<Rolle, Option<String>> = BTreeMap::new();
    for r in g.rollen.iter().flatten() {
        rollen.insert(*r, None);
    }
    for (mp_id, rs) in &g.parteien {
        for r in rs {
            rollen
                .entry(*r)
                .or_default()
                .get_or_insert_with(|| mp_id.clone());
        }
    }
    Antworten {
        rollen: rollen
            .into_iter()
            .map(|(rolle, mp_id)| RolleAntwort { rolle, mp_id })
            .collect(),
        dienste: g
            .dienste
            .iter()
            .map(|(name, url)| DienstAntwort {
                name: name.clone(),
                url: url.clone(),
                api_key_file: None,
            })
            .collect(),
        ..Antworten::default()
    }
}

/// A value that is key material rather than the place it lives.
fn ist_schluesselmaterial(s: &str) -> bool {
    s.contains("-----BEGIN") || s.contains("PRIVATE KEY") || s.contains('\n')
}

/// A TOML basic string.
fn zitiert(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn liste<'a>(werte: impl IntoIterator<Item = &'a str>) -> String {
    format!(
        "[{}]",
        werte
            .into_iter()
            .map(zitiert)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The lines of one file, being built.
struct Entwurf {
    dienst: &'static str,
    datei: &'static str,
    abgeglichen: bool,
    zeilen: Vec<Zeile>,
}

impl Entwurf {
    const fn neu(dienst: &'static str, datei: &'static str, abgeglichen: bool) -> Self {
        Self {
            dienst,
            datei,
            abgeglichen,
            zeilen: Vec::new(),
        }
    }

    fn abschnitt(&mut self, art: Art, text: &str) {
        let art = if art == Art::Weg {
            Art::Weg
        } else {
            Art::Abschnitt
        };
        self.zeilen.push(Zeile {
            art,
            text: text.to_owned(),
            quelle: None,
            frage: None,
        });
    }

    fn zeile(&mut self, art: Art, text: String, quelle: &str, frage: &str) {
        self.zeilen.push(Zeile {
            art,
            text,
            quelle: Some(quelle.to_owned()),
            frage: Some(frage.to_owned()),
        });
    }

    fn fehlt(&mut self, was: String, quelle: &str, frage: &str) {
        self.zeile(Art::Fehlt, format!("# FEHLT: {was}"), quelle, frage);
    }

    /// `key = value` when answered, a gap otherwise.
    fn pfad(&mut self, art: Art, key: &str, wert: Option<&str>, quelle: &str, frage: &str) {
        match wert {
            Some(w) => self.zeile(art, format!("{key} = {}", zitiert(w)), quelle, frage),
            None => self.fehlt(format!("{key} — der Pfad der Datei"), quelle, frage),
        }
    }

    fn fertig(self) -> Datei {
        use std::fmt::Write as _;
        let mut toml = format!(
            "# {} — vorgeschlagen von der Einrichtung. Jede Zeile nennt ihre Quelle;\n\
             # eine Lücke steht als # FEHLT: und muss gefüllt werden.\n",
            self.datei
        );
        let mut diff = format!(
            "--- {} (geladen)\n+++ {} (Vorschlag)\n",
            self.datei, self.datei
        );
        let mut aenderungen = 0;
        let mut luecken = 0;
        for z in &self.zeilen {
            match z.art {
                Art::Abschnitt => {
                    let _ = write!(toml, "\n{}\n", z.text);
                    let _ = writeln!(diff, " {}", z.text);
                }
                Art::Gleich | Art::Neu => {
                    if let Some(q) = &z.quelle {
                        let _ = writeln!(toml, "# Quelle: {q}");
                    }
                    let _ = writeln!(toml, "{}", z.text);
                    let zeichen = if z.art == Art::Neu { '+' } else { ' ' };
                    let _ = writeln!(diff, "{zeichen}{}", z.text);
                    if z.art == Art::Neu {
                        aenderungen += 1;
                    }
                }
                Art::Weg => {
                    let _ = writeln!(toml, "# ENTFÄLLT: {}", z.text);
                    let _ = writeln!(diff, "-{}", z.text);
                    aenderungen += 1;
                }
                Art::Fehlt => {
                    let _ = writeln!(toml, "{}", z.text);
                    let _ = writeln!(diff, "?{}", z.text);
                    luecken += 1;
                }
            }
        }
        Datei {
            dienst: self.dienst.to_owned(),
            datei: self.datei.to_owned(),
            abgeglichen: self.abgeglichen,
            zeilen: self.zeilen,
            toml,
            diff,
            aenderungen,
            luecken,
        }
    }
}

/// An MP-ID as answered, checked against the rules its Rolle sets.
fn pruefe_mp_id(rolle: Rolle, mp_id: &str, pfad: &str) -> Result<(), Befund> {
    let ziffern = mp_id.len() == 13 && mp_id.bytes().all(|b| b.is_ascii_digit());
    let eic = mp_id.len() == 16
        && mp_id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-');
    if !ziffern && !eic {
        return Err(Befund::blockiert(
            pfad,
            format!("{mp_id:?} ist weder eine 13-stellige Codenummer noch ein 16-stelliger EIC"),
        )
        .bereich(Bereich::Identitaeten)
        .quelle(CODENUMMER));
    }
    let falsch = match (rolle.sparte(), &mp_id[..2]) {
        (Sparte::Gas, "99") if ziffern => {
            Some(("BDEW-Codenummer (Strom)", "DVGW-Codenummer (98…)"))
        }
        (Sparte::Strom, "98") if ziffern => {
            Some(("DVGW-Codenummer (Gas)", "BDEW-Codenummer (99…)"))
        }
        _ => None,
    };
    match falsch {
        Some((ist, braucht)) => Err(Befund::blockiert(
            pfad,
            format!(
                "{mp_id:?} ist eine {ist}, die Rolle {} braucht eine {braucht}",
                rolle.token()
            ),
        )
        .bereich(Bereich::Identitaeten)
        .quelle(CODENUMMER)),
        None => Ok(()),
    }
}

/// A path as answered: `None` when unanswered, refused when it is key
/// material.
fn pfad_antwort<'a>(
    wert: Option<&'a String>,
    pfad: &str,
    abgelehnt: &mut Vec<Befund>,
) -> Option<&'a str> {
    let w = wert.map(|s| s.trim()).filter(|s| !s.is_empty())?;
    if ist_schluesselmaterial(w) {
        abgelehnt.push(
            Befund::blockiert(
                pfad,
                "die Antwort ist Schlüsselmaterial, kein Pfad — der Vorschlag nennt nur, wo \
                 eine Datei liegt, und schreibt keinen Schlüssel",
            )
            .bereich(Bereich::Zertifikate),
        );
        return None;
    }
    Some(w)
}

/// The answered Rollen with their MP-IDs, every MP-ID checked against the
/// rules its Rolle sets; a refused one stays a Rolle without an MP-ID.
fn rollen_pruefen(
    antworten: &Antworten,
    abgelehnt: &mut Vec<Befund>,
) -> BTreeMap<Rolle, Option<String>> {
    let mut rollen: BTreeMap<Rolle, Option<String>> = BTreeMap::new();
    for (i, r) in antworten.rollen.iter().enumerate() {
        if rollen.contains_key(&r.rolle) {
            abgelehnt.push(
                Befund::blockiert(
                    format!("rollen[{i}]"),
                    format!(
                        "die Rolle {} ist zweimal beantwortet — je Rolle und Sparte genau eine \
                         MP-ID",
                        r.rolle.token()
                    ),
                )
                .bereich(Bereich::Identitaeten)
                .quelle(EINE_MP_ID),
            );
            continue;
        }
        let mp_id = r
            .mp_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(
                |m| match pruefe_mp_id(r.rolle, m, &format!("rollen[{i}].mp_id")) {
                    Ok(()) => Some(m.to_owned()),
                    Err(b) => {
                        abgelehnt.push(b);
                        None
                    }
                },
            );
        rollen.insert(r.rolle, mp_id);
    }
    // One Codenummer names one Sparte: a 13-digit GLN under both is refused.
    let mut sparte_von: BTreeMap<String, Sparte> = BTreeMap::new();
    for (rolle, mp_id) in &mut rollen {
        let Some(m) = mp_id.clone() else { continue };
        match sparte_von.get(&m) {
            Some(s) if *s != rolle.sparte() => {
                abgelehnt.push(
                    Befund::blockiert(
                        format!("rollen.{}", rolle.token()),
                        format!(
                            "{m:?} ist schon einer {}-Rolle zugeordnet — Strom und Gas brauchen \
                             je eine eigene MP-ID",
                            s.as_str()
                        ),
                    )
                    .bereich(Bereich::Identitaeten)
                    .quelle(SPARTEN_GETRENNT),
                );
                *mp_id = None;
            }
            _ => {
                sparte_von.insert(m, rolle.sparte());
            }
        }
    }
    rollen
}

/// Propose the configuration `antworten` describe, marked against `geladen` —
/// the reports of the deployment's daemons, as the checklist reads them.
#[must_use]
pub fn vorschlag(antworten: &Antworten, geladen: &[DienstBericht]) -> Vorschlag {
    let g = geladen_aus(geladen);
    let mut abgelehnt: Vec<Befund> = Vec::new();

    let rollen = rollen_pruefen(antworten, &mut abgelehnt);

    // [[party]]: one per MP-ID, the Rollen it plays in proposal order.
    let mut parteien: Vec<(String, Vec<Rolle>)> = Vec::new();
    for (rolle, mp_id) in &rollen {
        let Some(m) = mp_id else { continue };
        match parteien.iter_mut().find(|(id, _)| id == m) {
            Some((_, rs)) => rs.push(*rolle),
            None => parteien.push((m.clone(), vec![*rolle])),
        }
    }
    let primaer: Option<String> = match parteien.as_slice() {
        [(einzige, _)] => Some(einzige.clone()),
        _ => antworten
            .primaer
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .and_then(|p| {
                if parteien.iter().any(|(id, _)| id == p) {
                    Some(p.to_owned())
                } else {
                    abgelehnt.push(
                        Befund::blockiert(
                            "primaer",
                            format!("{p:?} ist keine der beantworteten MP-IDs"),
                        )
                        .bereich(Bereich::Identitaeten),
                    );
                    None
                }
            }),
    };

    let mut dateien = Vec::new();
    if !rollen.is_empty() || !antworten.dienste.is_empty() {
        dateien.push(makod_datei(
            antworten,
            &g,
            &rollen,
            &parteien,
            primaer.as_deref(),
            &mut abgelehnt,
        ));
    }
    if !rollen.is_empty() {
        dateien.push(processd_datei(
            antworten,
            &rollen,
            primaer.as_deref(),
            &mut abgelehnt,
        ));
    }
    let luecken = dateien.iter().map(|d| d.luecken).sum();
    Vorschlag {
        vollstaendig: luecken == 0 && abgelehnt.is_empty(),
        dateien,
        abgelehnt,
        luecken,
        vorbelegt: vorbelegen(&g),
    }
}

fn makod_datei(
    antworten: &Antworten,
    g: &Geladen,
    rollen: &BTreeMap<Rolle, Option<String>>,
    parteien: &[(String, Vec<Rolle>)],
    primaer: Option<&str>,
    abgelehnt: &mut Vec<Befund>,
) -> Datei {
    let mut d = Entwurf::neu("makod", "makod.toml", g.makod);

    if !rollen.is_empty() {
        d.abschnitt(Art::Abschnitt, "[deployment]");
        let gewollt: BTreeSet<Rolle> = rollen.keys().copied().collect();
        if let Some(alt) = g.rollen.as_ref().filter(|alt| **alt != gewollt) {
            d.zeile(
                Art::Weg,
                format!("rollen = {}", liste(alt.iter().map(|r| r.token()))),
                ROLLEN,
                "rollen",
            );
        }
        d.zeile(
            art_gegen(g, g.rollen.as_ref() == Some(&gewollt)),
            format!("rollen = {}", liste(gewollt.iter().map(|r| r.token()))),
            ROLLEN,
            "rollen",
        );
    }

    for (mp_id, rs) in parteien {
        let frage = format!("rollen.{}", rs[0].token());
        let gleich = g
            .parteien
            .get(mp_id)
            .is_some_and(|alt| *alt == rs.iter().copied().collect::<BTreeSet<_>>());
        let art = art_gegen(g, gleich);
        d.abschnitt(Art::Abschnitt, "[[party]]");
        let quelle = if rs.iter().any(|r| r.sparte() == Sparte::Gas) || mp_id.starts_with("99") {
            CODENUMMER
        } else {
            EINE_MP_ID
        };
        d.zeile(art, format!("mp_id = {}", zitiert(mp_id)), quelle, &frage);
        d.zeile(
            art,
            format!("roles = {}", liste(rs.iter().map(|r| r.kuerzel()))),
            EINE_MP_ID,
            &frage,
        );
        match primaer {
            Some(p) if p == mp_id => d.zeile(art, "primary = true".to_owned(), PRIMAER, "primaer"),
            Some(_) => {}
            None => d.fehlt(
                "primary = true bei genau einer [[party]] — welche MP-ID ist der Speicherschlüssel?"
                    .to_owned(),
                PRIMAER,
                "primaer",
            ),
        }
        party_as4(&mut d, antworten.as4.as_ref(), mp_id, abgelehnt);
    }
    for (rolle, mp_id) in rollen {
        if mp_id.is_none() {
            d.fehlt(
                format!(
                    "[[party]] für {} — mp_id, eine {} (roles = [{}])",
                    rolle.token(),
                    match rolle.sparte() {
                        Sparte::Strom => "BDEW-Codenummer (99…)",
                        Sparte::Gas => "DVGW-Codenummer (98…)",
                    },
                    zitiert(rolle.kuerzel())
                ),
                EINE_MP_ID,
                &format!("rollen.{}", rolle.token()),
            );
        }
    }
    // A loaded identity the answers no longer name, or name with other Rollen.
    if !rollen.is_empty() {
        for (mp_id, alt) in &g.parteien {
            let bleibt = parteien
                .iter()
                .any(|(id, rs)| id == mp_id && rs.iter().copied().collect::<BTreeSet<_>>() == *alt);
            if bleibt {
                continue;
            }
            d.abschnitt(Art::Weg, "[[party]]");
            d.zeile(
                Art::Weg,
                format!("mp_id = {}", zitiert(mp_id)),
                EINE_MP_ID,
                "rollen",
            );
            d.zeile(
                Art::Weg,
                format!("roles = {}", liste(alt.iter().map(|r| r.kuerzel()))),
                EINE_MP_ID,
                "rollen",
            );
        }
    }

    if let Some(as4) = &antworten.as4 {
        as4_abschnitt(&mut d, as4, abgelehnt);
    }
    dienste_abschnitt(&mut d, antworten, g, abgelehnt);
    d.fertig()
}

/// `[as4]`: the message-id domain and the trust paths, a gap for each one
/// unanswered.
fn as4_abschnitt(d: &mut Entwurf, as4: &As4Antwort, abgelehnt: &mut Vec<Befund>) {
    d.abschnitt(Art::Abschnitt, "[as4]");
    match as4
        .message_id_domain
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(dom) => d.zeile(
            Art::Neu,
            format!("message_id_domain = {}", zitiert(dom)),
            MESSAGE_ID,
            "as4",
        ),
        None => d.fehlt(
            "message_id_domain — eine Domain, die der Betreiber hält".to_owned(),
            MESSAGE_ID,
            "as4",
        ),
    }
    let anker = pfad_antwort(
        as4.trust_anchor_pem_file.as_ref(),
        "as4.trust_anchor_pem_file",
        abgelehnt,
    );
    d.pfad(
        Art::Neu,
        "trust_anchor_pem_file",
        anker,
        VERTRAUENSANKER,
        "as4",
    );
    let crl = pfad_antwort(
        as4.crl_signer_pem_file.as_ref(),
        "as4.crl_signer_pem_file",
        abgelehnt,
    );
    d.pfad(Art::Neu, "crl_signer_pem_file", crl, SPERRLISTE, "as4");
}

/// `[[einrichtung.dienst]]`: each daemon answered, and a loaded one the answers
/// no longer name as removed.
fn dienste_abschnitt(
    d: &mut Entwurf,
    antworten: &Antworten,
    g: &Geladen,
    abgelehnt: &mut Vec<Befund>,
) {
    let mut genannt = BTreeSet::new();
    for (i, s) in antworten.dienste.iter().enumerate() {
        let (name, url) = (s.name.trim(), s.url.trim());
        if name.is_empty() || url.is_empty() {
            continue;
        }
        genannt.insert((name.to_owned(), url.to_owned()));
        let art = art_gegen(g, g.dienste.iter().any(|(n, u)| n == name && u == url));
        d.abschnitt(Art::Abschnitt, "[[einrichtung.dienst]]");
        d.zeile(art, format!("name = {}", zitiert(name)), DIENST, "dienste");
        d.zeile(art, format!("url  = {}", zitiert(url)), DIENST, "dienste");
        if let Some(k) = pfad_antwort(
            s.api_key_file.as_ref(),
            &format!("dienste[{i}].api_key_file"),
            abgelehnt,
        ) {
            d.zeile(
                Art::Neu,
                format!("api_key_file = {}", zitiert(k)),
                SCHLUESSELDATEI,
                "dienste",
            );
        }
    }
    if !antworten.dienste.is_empty() {
        for (name, url) in &g.dienste {
            if genannt.contains(&(name.clone(), url.clone())) {
                continue;
            }
            d.abschnitt(Art::Weg, "[[einrichtung.dienst]]");
            d.zeile(
                Art::Weg,
                format!("name = {}", zitiert(name)),
                DIENST,
                "dienste",
            );
            d.zeile(
                Art::Weg,
                format!("url  = {}", zitiert(url)),
                DIENST,
                "dienste",
            );
        }
    }
}

/// Unchanged when the loaded configuration already holds the line, new otherwise.
const fn art_gegen(g: &Geladen, gleich: bool) -> Art {
    if gleich && g.makod {
        Art::Gleich
    } else {
        Art::Neu
    }
}

/// `[party.as4]` of one MP-ID: every path answered, a gap for the rest.
fn party_as4(d: &mut Entwurf, as4: Option<&As4Antwort>, mp_id: &str, abgelehnt: &mut Vec<Befund>) {
    let Some(as4) = as4 else {
        d.fehlt(
            format!(
                "[party.as4] für {mp_id} — die Pfade der Signatur-, Verschlüsselungs- und \
                 TLS-Schlüsselpaare dieser MP-ID"
            ),
            AS4_SCHLUESSEL,
            "as4",
        );
        return;
    };
    let leer = As4Schluessel::default();
    let (i, k) = as4
        .schluessel
        .iter()
        .enumerate()
        .find(|(_, k)| k.mp_id.trim() == mp_id)
        .map_or((None, &leer), |(i, k)| (Some(i), k));
    d.abschnitt(Art::Abschnitt, "[party.as4]");
    for (key, wert) in [
        ("signing_cert_pem_file", &k.signing_cert_pem_file),
        ("signing_key_pem_file", &k.signing_key_pem_file),
        ("encryption_cert_pem_file", &k.encryption_cert_pem_file),
        ("encryption_key_pem_file", &k.encryption_key_pem_file),
        ("tls_cert_pem_file", &k.tls_cert_pem_file),
        ("tls_key_pem_file", &k.tls_key_pem_file),
    ] {
        let pfad = i.map_or_else(String::new, |i| format!("as4.schluessel[{i}].{key}"));
        let w = pfad_antwort(wert.as_ref(), &pfad, abgelehnt);
        d.pfad(Art::Neu, key, w, AS4_SCHLUESSEL, "as4");
    }
}

fn processd_datei(
    antworten: &Antworten,
    rollen: &BTreeMap<Rolle, Option<String>>,
    primaer: Option<&str>,
    abgelehnt: &mut Vec<Befund>,
) -> Datei {
    let mut d = Entwurf::neu("processd", "processd.toml", false);
    d.abschnitt(Art::Abschnitt, "[identity]");
    match primaer {
        Some(p) => d.zeile(
            Art::Neu,
            format!("own_mp_id = {}", zitiert(p)),
            EIGENE_MP_ID,
            "primaer",
        ),
        None => d.fehlt(
            "own_mp_id — die MP-ID mit primary = true".to_owned(),
            EIGENE_MP_ID,
            "primaer",
        ),
    }
    match &antworten.verbundene_lieferanten {
        Some(v) => d.zeile(
            Art::Neu,
            format!(
                "verbundene_lieferanten = {}",
                liste(v.iter().map(|s| s.trim()).filter(|s| !s.is_empty()))
            ),
            VERBUNDENE,
            "verbundene_lieferanten",
        ),
        None => d.fehlt(
            "verbundene_lieferanten — die MP-IDs der verbundenen Lieferanten, [] für keine"
                .to_owned(),
            VERBUNDENE,
            "verbundene_lieferanten",
        ),
    }

    bilanzkreise_abschnitt(&mut d, antworten, rollen, abgelehnt);
    d.fertig()
}

/// `[[lf.bilanzkreise]]`: a row per Sparte the deployment supplies and the
/// answers name, a gap for each it supplies and they do not.
fn bilanzkreise_abschnitt(
    d: &mut Entwurf,
    antworten: &Antworten,
    rollen: &BTreeMap<Rolle, Option<String>>,
    abgelehnt: &mut Vec<Befund>,
) {
    let lieferant: BTreeSet<Sparte> = rollen
        .keys()
        .filter(|r| r.ist_lieferant())
        .map(|r| r.sparte())
        .collect();
    let mut beantwortet = BTreeSet::new();
    for (i, b) in antworten.bilanzkreise.iter().enumerate() {
        let standard: Vec<&str> = b
            .standard
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if standard.is_empty() {
            continue;
        }
        if !lieferant.contains(&b.sparte) {
            abgelehnt.push(
                Befund::blockiert(
                    format!("bilanzkreise[{i}]"),
                    format!(
                        "ein Bilanzkreis für {}, aber das Deployment spielt keinen Lieferanten \
                         {}",
                        b.sparte.as_str(),
                        b.sparte.as_str()
                    ),
                )
                .bereich(Bereich::Bilanzierung),
            );
            continue;
        }
        if !beantwortet.insert(b.sparte) {
            abgelehnt.push(
                Befund::blockiert(
                    format!("bilanzkreise[{i}]"),
                    format!("die Sparte {} ist zweimal beantwortet", b.sparte.as_str()),
                )
                .bereich(Bereich::Bilanzierung)
                .quelle(BILANZKREIS_GAS),
            );
            continue;
        }
        let quelle = match b.sparte {
            Sparte::Strom => BILANZKREIS_STROM,
            Sparte::Gas => BILANZKREIS_GAS,
        };
        let frage = format!("bilanzkreise.{}", b.sparte.as_str());
        d.abschnitt(Art::Abschnitt, "[[lf.bilanzkreise]]");
        d.zeile(
            Art::Neu,
            format!("sparte   = {}", zitiert(b.sparte.as_str())),
            quelle,
            &frage,
        );
        d.zeile(
            Art::Neu,
            format!("standard = {}", liste(standard)),
            quelle,
            &frage,
        );
    }
    for s in lieferant.difference(&beantwortet) {
        d.fehlt(
            format!(
                "[[lf.bilanzkreise]] sparte = {} — der Bilanzkreis, dem der Lieferant {} \
                 zuordnet (standard = [...])",
                zitiert(s.as_str()),
                s.as_str()
            ),
            match s {
                Sparte::Strom => BILANZKREIS_STROM,
                Sparte::Gas => BILANZKREIS_GAS,
            },
            &format!("bilanzkreise.{}", s.as_str()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::einrichtung::{Einrichtung, Partei};
    use crate::policy::{Origin, PolicyClass, PolicyEntry, PolicyReport};

    fn rolle(rolle: Rolle, mp_id: &str) -> RolleAntwort {
        RolleAntwort {
            rolle,
            mp_id: Some(mp_id.to_owned()),
        }
    }

    /// The fixture interview: a Lieferant Strom und Gas, its two MP-IDs, the
    /// Strom Bilanzkreis — and the Gas Bilanzkreis left open.
    fn lieferant_strom_und_gas() -> Antworten {
        Antworten {
            rollen: vec![
                rolle(Rolle::LfStrom, "9900357000004"),
                rolle(Rolle::LfGas, "9800357000005"),
            ],
            primaer: Some("9900357000004".into()),
            bilanzkreise: vec![BilanzkreisAntwort {
                sparte: Sparte::Strom,
                standard: vec!["11XBK-STD------Y".into()],
            }],
            verbundene_lieferanten: Some(vec![]),
            as4: None,
            dienste: vec![DienstAntwort {
                name: "processd".into(),
                url: "http://processd:8580".into(),
                api_key_file: Some("/run/secrets/processd-read-config".into()),
            }],
        }
    }

    fn datei<'a>(v: &'a Vorschlag, dienst: &str) -> &'a Datei {
        v.dateien.iter().find(|d| d.dienst == dienst).unwrap()
    }

    /// The Lieferant Strom und Gas: two `[[party]]`, Strom under `99…`, Gas
    /// under `98…`, both Rollen declared, the Strom Bilanzkreis proposed and
    /// the Gas one a named gap; every value line names its source, and each
    /// file parses as TOML.
    #[test]
    fn the_lieferant_strom_und_gas_interview_proposes_both_identities_and_names_the_gap() {
        let v = vorschlag(&lieferant_strom_und_gas(), &[]);
        assert!(v.abgelehnt.is_empty(), "{:?}", v.abgelehnt);
        let makod = datei(&v, "makod");
        assert!(
            makod.toml.contains("rollen = [\"lf-strom\", \"lf-gas\"]"),
            "{}",
            makod.toml
        );
        assert!(
            makod
                .diff
                .contains("+mp_id = \"9900357000004\"\n+roles = [\"LF\"]\n+primary = true"),
            "{}",
            makod.diff
        );
        assert!(
            makod.toml.contains("mp_id = \"9800357000005\"\n"),
            "{}",
            makod.toml
        );
        assert!(makod.toml.contains("roles = [\"LFG\"]"), "{}", makod.toml);
        assert!(
            makod
                .toml
                .contains("api_key_file = \"/run/secrets/processd-read-config\"")
        );

        let processd = datei(&v, "processd");
        assert!(processd.toml.contains("own_mp_id = \"9900357000004\""));
        assert!(processd.toml.contains("standard = [\"11XBK-STD------Y\"]"));
        let gas = processd
            .zeilen
            .iter()
            .find(|z| z.frage.as_deref() == Some("bilanzkreise.gas"))
            .expect("the Gas Bilanzkreis is named");
        assert_eq!(gas.art, Art::Fehlt);
        assert!(
            gas.text
                .starts_with("# FEHLT: [[lf.bilanzkreise]] sparte = \"gas\""),
            "{}",
            gas.text
        );

        for d in &v.dateien {
            for z in &d.zeilen {
                if z.art != Art::Abschnitt {
                    assert!(
                        z.quelle.is_some(),
                        "{}: {} names no source",
                        d.datei,
                        z.text
                    );
                }
            }
            let parsed: Result<toml::Value, _> = toml::from_str(&d.toml);
            assert!(
                parsed.is_ok(),
                "{} does not parse: {parsed:?}\n{}",
                d.datei,
                d.toml
            );
        }
        // AS4 was not answered: a gap per MP-ID, never a guessed path.
        assert_eq!(
            makod
                .zeilen
                .iter()
                .filter(|z| z.art == Art::Fehlt && z.frage.as_deref() == Some("as4"))
                .count(),
            2
        );
        assert!(!v.vollstaendig);
        assert_eq!(v.luecken, makod.luecken + processd.luecken);
        assert_eq!(
            v,
            vorschlag(&lieferant_strom_und_gas(), &[]),
            "deterministic"
        );
    }

    /// A Gas Rolle under a `99…` code is refused, naming the rule; the
    /// identity becomes a gap rather than being written.
    #[test]
    fn a_gas_rolle_under_a_bdew_codenummer_is_refused_with_its_rule() {
        let mut a = lieferant_strom_und_gas();
        a.rollen[1].mp_id = Some("9900357000004".into());
        let v = vorschlag(&a, &[]);
        assert_eq!(v.abgelehnt.len(), 1, "{:?}", v.abgelehnt);
        let b = &v.abgelehnt[0];
        assert_eq!(b.pfad, "rollen[1].mp_id");
        assert_eq!(b.bereich, Bereich::Identitaeten);
        assert!(b.text.contains("DVGW-Codenummer (98…)"), "{}", b.text);
        assert!(
            b.quelle
                .as_deref()
                .is_some_and(|q| q.contains("Identifikatoren V1.2, Kap. 2.2")),
            "{b:?}"
        );
        let makod = datei(&v, "makod");
        assert!(
            !makod.toml.contains("roles = [\"LF\", \"LFG\"]"),
            "{}",
            makod.toml
        );
        assert!(
            makod
                .zeilen
                .iter()
                .any(|z| z.art == Art::Fehlt && z.frage.as_deref() == Some("rollen.lf-gas")),
            "{}",
            makod.toml
        );
        assert!(!v.vollstaendig);
    }

    /// A certificate is a path in the proposal; an answer that is the
    /// certificate itself is refused and never written.
    #[test]
    fn a_certificate_is_written_as_a_path_and_key_material_is_refused() {
        let mut a = lieferant_strom_und_gas();
        a.rollen.truncate(1);
        a.bilanzkreise.clear();
        a.as4 = Some(As4Antwort {
            message_id_domain: Some("mako.example".into()),
            trust_anchor_pem_file: Some("/etc/makod/sm-pki-root.pem".into()),
            crl_signer_pem_file: Some("/etc/makod/sm-pki-ccrl-s-root.pem".into()),
            schluessel: vec![As4Schluessel {
                mp_id: "9900357000004".into(),
                signing_cert_pem_file: Some("/etc/makod/9900357000004/signing.cert.pem".into()),
                signing_key_pem_file: Some(
                    "-----BEGIN PRIVATE KEY-----\nMIIEvQ\n-----END PRIVATE KEY-----".into(),
                ),
                ..As4Schluessel::default()
            }],
        });
        let v = vorschlag(&a, &[]);
        let makod = datei(&v, "makod");
        assert!(
            makod
                .toml
                .contains("signing_cert_pem_file = \"/etc/makod/9900357000004/signing.cert.pem\""),
            "{}",
            makod.toml
        );
        for d in &v.dateien {
            assert!(!d.toml.contains("BEGIN"), "{}", d.toml);
            assert!(!d.toml.contains("MIIEvQ"), "{}", d.toml);
            assert!(!d.diff.contains("MIIEvQ"), "{}", d.diff);
        }
        assert!(
            v.abgelehnt
                .iter()
                .any(|b| b.pfad == "as4.schluessel[0].signing_key_pem_file"),
            "{:?}",
            v.abgelehnt
        );
        assert!(
            makod.toml.contains("# FEHLT: signing_key_pem_file"),
            "{}",
            makod.toml
        );
        assert!(
            makod.toml.contains("# FEHLT: tls_key_pem_file"),
            "{}",
            makod.toml
        );
    }

    /// Against what makod reports as loaded: an identity it already holds is
    /// unchanged, one it holds under another Rolle set is removed, and the
    /// interview starts from the loaded answers.
    #[test]
    fn the_proposal_is_marked_against_the_loaded_configuration() {
        let mut policy = PolicyReport::new();
        policy.entries.push(PolicyEntry {
            key: "deployment.rollen".into(),
            value: "lf-strom,nb-strom".into(),
            origin: Origin::Explicit,
            class: PolicyClass::Posture,
            quelle: None,
            doc: None,
            insecure: false,
            review_by: None,
            review_grund: None,
        });
        let makod = Einrichtung::neu(
            "makod",
            vec![],
            policy,
            vec![
                Partei {
                    rolle: "LF".into(),
                    sparte: Some("strom".into()),
                    mp_id: "9900357000004".into(),
                },
                Partei {
                    rolle: "VNB".into(),
                    sparte: Some("strom".into()),
                    mp_id: "9900001000001".into(),
                },
            ],
        );
        let geladen = vec![
            DienstBericht {
                dienst: "makod".into(),
                url: String::new(),
                antwort: Antwort::Erreicht { einrichtung: makod },
            },
            DienstBericht {
                dienst: "processd".into(),
                url: "http://processd:8580".into(),
                antwort: Antwort::Unerreichbar {
                    grund: "connection refused".into(),
                },
            },
        ];
        let v = vorschlag(&Antworten::default(), &geladen);
        assert!(v.dateien.is_empty());
        assert_eq!(
            v.vorbelegt.rollen,
            vec![
                rolle(Rolle::LfStrom, "9900357000004"),
                rolle(Rolle::NbStrom, "9900001000001")
            ]
        );
        assert_eq!(v.vorbelegt.dienste[0].url, "http://processd:8580");

        let v = vorschlag(&lieferant_strom_und_gas(), &geladen);
        let d = datei(&v, "makod");
        assert!(d.abgeglichen);
        assert!(
            d.diff.contains("-rollen = [\"lf-strom\", \"nb-strom\"]"),
            "{}",
            d.diff
        );
        assert!(
            d.diff.contains("+rollen = [\"lf-strom\", \"lf-gas\"]"),
            "{}",
            d.diff
        );
        assert!(d.diff.contains(" mp_id = \"9900357000004\""), "{}", d.diff);
        assert!(d.diff.contains("-mp_id = \"9900001000001\""), "{}", d.diff);
        assert!(d.diff.contains("+mp_id = \"9800357000005\""), "{}", d.diff);
        assert!(
            d.diff.contains(" url  = \"http://processd:8580\""),
            "{}",
            d.diff
        );
        assert!(!datei(&v, "processd").abgeglichen);
    }

    /// A GLN (neither `99…` nor `98…`) under both Sparten is one Codenummer
    /// naming two Energiearten, and is refused for the second.
    #[test]
    fn one_gln_for_both_sparten_is_refused() {
        let a = Antworten {
            rollen: vec![
                rolle(Rolle::LfStrom, "4012345000016"),
                rolle(Rolle::LfGas, "4012345000016"),
            ],
            ..Antworten::default()
        };
        let v = vorschlag(&a, &[]);
        assert_eq!(v.abgelehnt.len(), 1);
        assert_eq!(v.abgelehnt[0].quelle.as_deref(), Some(SPARTEN_GETRENNT));
    }
}
