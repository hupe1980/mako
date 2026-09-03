//! `cargo xtask sync-regulatories` — mirror and audit the BDEW document set.
//!
//! `regulatories/bdew-mako/` is the source of truth behind every profile in
//! `crates/edi-energy/profiles/`: the MIG segment layouts, the AHB
//! Prüfidentifikator tables, the code lists and the Entscheidungsbäume are all
//! read out of those PDFs by hand, so a profile is only as correct as the
//! document it was read from.
//!
//! BDEW publishes the catalogue as JSON:
//!
//! - `GET https://www.bdew-mako.de/api/documents` — the whole index
//! - `GET https://www.bdew-mako.de/api/downloadFile/{fileId}` — one file
//!
//! ## What it does
//!
//! | Mode | Effect |
//! |---|---|
//! | (default) | Fetch the index, reconcile against the mirror, print a report |
//! | `--download` | Additionally fetch every document the mirror is missing |
//! | `--offline` | Check the manifest against what is on disk; no network |
//! | `--json` | Emit the reconciliation as JSON |
//!
//! ## Why a manifest
//!
//! BDEW reissues a corrected document under an unchanged version number, so
//! only a content hash can say whether the local copy is the one a profile was
//! read from. `manifest.json` records, per mirrored file, the upstream
//! `fileId`, the validity window and the SHA-256 of its bytes.
//!
//! The manifest is tracked while the PDFs are not, so `--offline` — what CI
//! runs — needs neither the network nor the 400 MB. There, a recorded document
//! that is simply absent is reported rather than failed; only one that is
//! present and no longer matches its hash is an error.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The catalogue endpoint. Public and unauthenticated.
const DOCUMENTS_API: &str = "https://www.bdew-mako.de/api/documents";
/// One file, by the `fileId` the catalogue gives it.
const DOWNLOAD_API: &str = "https://www.bdew-mako.de/api/downloadFile";
/// Where the mirror lives, relative to the workspace root.
const MIRROR_DIR: &str = "regulatories/bdew-mako";
/// The mirror's provenance record, inside the mirror.
const MANIFEST: &str = "manifest.json";

// ── The BDEW catalogue ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ResponseModel {
    data: Vec<RemoteDocument>,
}

/// One entry of the BDEW document catalogue.
///
/// Only the fields the reconciliation needs are modelled; the catalogue carries
/// a dozen more (topic grouping, sort order, whether questions are allowed)
/// that say nothing about identity or validity.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteDocument {
    /// Catalogue entry id — stable across reissues of the same document.
    id: u64,
    /// The downloadable file. Absent for link-only entries.
    file_id: Option<u64>,
    title: String,
    /// Free documents need no BDEW membership; the rest cannot be mirrored.
    #[serde(default)]
    is_free: bool,
    /// **Anwendungszeitpunkt** — the day the document goes on the wire, not the
    /// day it was published. See `Publikationsdatum ≠ Anwendungszeitpunkt`.
    valid_from: Option<String>,
    /// Last day it applies; `None` for the open-ended current one.
    valid_to: Option<String>,
    #[serde(default)]
    is_consolidated_reading_version: bool,
    #[serde(default)]
    is_extraordinary_publication: bool,
    #[serde(default)]
    is_error_correction: bool,
    #[serde(default)]
    is_informational_reading_version: bool,
    correction_date: Option<String>,
    file_type: Option<String>,
}

impl RemoteDocument {
    /// Whether this entry can be mirrored at all.
    fn is_mirrorable(&self) -> bool {
        self.is_free && self.file_id.is_some()
    }

    /// Whether the document applies on `today`, or is scheduled to.
    ///
    /// A superseded document is deliberately *not* dropped from the mirror:
    /// mako runs several format versions at once, and a process started under
    /// an older one continues under its rules. It is only excluded from the
    /// "missing" list, because nothing is gained by fetching a version no
    /// profile cites.
    fn is_current(&self, today: &str) -> bool {
        self.valid_to.as_deref().is_none_or(|v| v >= today)
    }

    /// The `informatorische Lesefassung` is the same content typeset for
    /// reading. Mirroring both doubles the directory for no added fact.
    fn is_informational(&self) -> bool {
        self.is_informational_reading_version || self.title.contains("informatorische Lesefassung")
    }

    /// The file name this document takes in the mirror.
    ///
    /// Modelled on the naming already committed by hand — `UTILMD_MIG_Strom_S2.1.pdf`,
    /// `APERAK_AHB_1.0_-_konsolidierte_Lesefassung_Stand_30.09.2025.pdf` — so a
    /// freshly downloaded file lands beside its predecessors rather than under a
    /// second convention.
    fn mirror_file_name(&self) -> String {
        let mut stem = self
            .title
            .trim()
            .replace(['/', '\\'], "-")
            .replace(':', "")
            .replace("  ", " ");
        stem = stem.replace(' ', "_");
        let ext = match self.file_type.as_deref() {
            Some("application/pdf") => "pdf",
            Some(t) if t.contains("wordprocessingml") => "docx",
            Some(t) if t.contains("xml") => "xsd",
            _ => "pdf",
        };
        format!("{stem}.{ext}")
    }

    /// The tokens that identify this document, for matching a mirror file whose
    /// name was written by hand.
    ///
    /// Hand-written names transliterate umlauts (`Anwendungsuebersicht`), drop
    /// the boilerplate `mit Fehlerkorrekturen`, and vary in separators, so a
    /// generated name never matches. The tokens that survive normalisation are
    /// the identifying ones — message type, kind, version, Sparte, `Stand`
    /// date, variant.
    fn identity_tokens(&self) -> BTreeSet<String> {
        let mut t = normalise_tokens(&self.title);
        // The flags are unreliable (see `variant`), so the title decides; these
        // only add a token the title already implies.
        if self.is_consolidated_reading_version {
            t.insert("konsolidierte".to_owned());
        }
        if self.is_extraordinary_publication {
            t.insert("ausserordentliche".to_owned());
        }
        t
    }

    /// A short label for the report.
    ///
    /// Read off the title, not the booleans: the catalogue leaves
    /// `isConsolidatedReadingVersion` false on entries whose own title says
    /// „konsolidierte Lesefassung", so the flags cannot be trusted to
    /// distinguish a variant.
    fn variant(&self) -> &'static str {
        let t = &self.title;
        if self.is_consolidated_reading_version || t.contains("konsolidierte Lesefassung") {
            "konsolidiert"
        } else if self.is_extraordinary_publication || t.contains("außerordentliche") {
            "außerordentlich"
        } else if self.is_error_correction || t.contains("Fehlerkorrektur") {
            "Fehlerkorrektur"
        } else {
            "Fassung"
        }
    }
}

/// Words that carry no identity — every document's title is full of them, and
/// the hand-written mirror names drop them.
const NOISE: &[&str] = &[
    "der",
    "die",
    "das",
    "den",
    "des",
    "und",
    "mit",
    "zum",
    "zur",
    "zu",
    "fuer",
    "von",
    "im",
    "in",
    "lesefassung",
    "fehlerkorrekturen",
    "fehlerkorrektur",
    "stand",
    "veroeffentlichung",
    "version",
    "edi",
    "energy",
    "bdew",
];

/// Lower-case, transliterate, and split a title into identity tokens.
///
/// German transliteration is the point: the mirror writes `Anwendungsuebersicht`
/// where the catalogue writes `Anwendungsübersicht`, and `Uebertragungsweg` for
/// `Übertragungsweg`.
fn normalise_tokens(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut word = String::new();
    let push = |w: &mut String, out: &mut BTreeSet<String>| {
        if !w.is_empty() {
            let t = std::mem::take(w);
            if !NOISE.contains(&t.as_str()) {
                out.insert(t);
            }
        }
    };
    for c in s.chars() {
        match c {
            'ä' | 'Ä' => word.push_str("ae"),
            'ö' | 'Ö' => word.push_str("oe"),
            'ü' | 'Ü' => word.push_str("ue"),
            'ß' => word.push_str("ss"),
            c if c.is_ascii_alphanumeric() => word.extend(c.to_lowercase()),
            '.' if !word.is_empty() => word.push('.'),
            _ => push(&mut word, &mut out),
        }
    }
    push(&mut word, &mut out);
    out
}

// ── The local mirror ─────────────────────────────────────────────────────────

/// Provenance for one mirrored file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestEntry {
    /// Upstream catalogue entry id.
    id: u64,
    /// Upstream file id — what `downloadFile` takes.
    file_id: u64,
    /// Catalogue title, verbatim. The version lives inside it.
    title: String,
    valid_from: Option<String>,
    valid_to: Option<String>,
    /// SHA-256 of the mirrored bytes.
    ///
    /// The reason the manifest exists: BDEW reissues corrected documents under
    /// the same version, so only the hash distinguishes the copy a profile was
    /// read from.
    sha256: String,
    /// Set when the upstream entry is flagged as a correction.
    #[serde(skip_serializing_if = "Option::is_none")]
    correction_date: Option<String>,
}

/// The mirror's provenance record, keyed by file name.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    /// Human note — this file is generated.
    #[serde(rename = "_note")]
    note: String,
    /// Day the manifest was last reconciled against the catalogue.
    synced_at: String,
    files: BTreeMap<String, ManifestEntry>,
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}

// ── Reconciliation ───────────────────────────────────────────────────────────

/// What the mirror and the catalogue disagree about.
#[derive(Debug, Default, Serialize)]
struct Reconciliation {
    /// Current upstream documents with no local file.
    missing: Vec<String>,
    /// Local files whose bytes no longer match the manifest.
    drifted: Vec<String>,
    /// Manifest entries with no file on disk.
    ///
    /// Not a defect: `regulatories/` is gitignored, so a fresh clone and every
    /// CI runner have the manifest and none of the PDFs. Only a file that is
    /// *present and changed* means the bytes a profile was read from moved.
    absent: Vec<String>,
    /// Local files the manifest does not describe.
    ///
    /// Not an error: the mirror holds hand-added sources (BNetzA Festlegungen,
    /// DVGW documents) that the BDEW catalogue never listed.
    unmanifested: Vec<String>,
    /// Upstream documents that superseded one the mirror holds.
    superseded: Vec<String>,
}

/// Whether two identity tokens name the same thing.
///
/// Equal, or one a prefix of the other with at least four characters in common.
/// German declension is the reason: the catalogue writes "Codeliste der
/// **europäischen** Ländercodes" while the mirror was named
/// `Codeliste_europaeische_Laendercodes`, and an exact match reports a document
/// as missing that is sitting in the directory. The four-character floor keeps
/// short version tokens (`1.0` vs `1.0a`) from collapsing into each other.
fn token_matches(want: &str, have: &str) -> bool {
    if want == have {
        return true;
    }
    let (short, long) = if want.len() < have.len() {
        (want, have)
    } else {
        (have, want)
    };
    // A version is an identity, not a word: `1.0` must not match `1.0a`.
    if short.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    short.len() >= 4 && long.starts_with(short)
}

/// The mirrored file that carries this document's identity, if any.
///
/// A local file matches when its own tokens contain every identity token of the
/// remote document. Containment rather than equality, because a hand-written
/// name may carry extra words the catalogue title does not — a Sparte spelled
/// out, or a date the title puts in prose.
fn find_local(doc: &RemoteDocument, local: &BTreeMap<String, BTreeSet<String>>) -> Option<String> {
    let want = doc.identity_tokens();
    if want.is_empty() {
        return None;
    }
    let mut best: Option<(usize, String)> = None;
    for (name, have) in local {
        if !want
            .iter()
            .all(|t| have.iter().any(|h| token_matches(t, h)))
        {
            continue;
        }
        // A base version's tokens are a subset of its konsolidierte sibling's,
        // so prefer the *closest* match — the file with fewest extra tokens.
        let extra = have.len().saturating_sub(want.len());
        if best.as_ref().is_none_or(|(e, _)| extra < *e) {
            best = Some((extra, name.clone()));
        }
    }
    best.map(|(_, n)| n)
}

/// Collapse catalogue entries that describe the same document.
///
/// BDEW lists some documents twice under an identical title, once with
/// `isFree: false` and once with `isFree: true`, pointing at different
/// `fileId`s. Downloading the wrong one answers `403`, and since the two are
/// indistinguishable by title the choice has to be made here: prefer the free
/// entry, and among equals the newest catalogue id.
fn dedupe(mut remote: Vec<RemoteDocument>) -> Vec<RemoteDocument> {
    remote.sort_by(|a, b| {
        a.title
            .trim()
            .cmp(b.title.trim())
            // free first, then the newest entry
            .then(b.is_free.cmp(&a.is_free))
            .then(b.id.cmp(&a.id))
    });
    remote.dedup_by(|a, b| a.title.trim() == b.title.trim());
    remote
}

/// Run the sync. Returns `true` when the mirror is consistent.
pub fn run(workspace_root: &Path, args: &[String]) -> bool {
    let download = args.iter().any(|a| a == "--download");
    let offline = args.iter().any(|a| a == "--offline");
    let json_out = args.iter().any(|a| a == "--json");

    let mirror = workspace_root.join(MIRROR_DIR);
    if !mirror.is_dir() {
        eprintln!("sync-regulatories: {} does not exist", mirror.display());
        return false;
    }
    let manifest_path = mirror.join(MANIFEST);
    let mut manifest: Manifest = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let local: BTreeSet<String> = std::fs::read_dir(&mirror)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != MANIFEST)
        .collect();

    // Every local file, reduced to the tokens that identify it.
    // The *stem*, not the file name: `IFTSTA_MIG_2.0g.pdf` tokenised whole
    // yields `2.0g.pdf`, and the version no longer matches the catalogue's
    // `2.0g`.
    let local_tokens: BTreeMap<String, BTreeSet<String>> = local
        .iter()
        .map(|n| {
            let stem = n.rsplit_once('.').map_or(n.as_str(), |(s, _)| s);
            (n.clone(), normalise_tokens(stem))
        })
        .collect();

    let mut rec = Reconciliation::default();

    // ── Offline half: the manifest against the files on disk ─────────────────
    for (name, entry) in &manifest.files {
        let path = mirror.join(name);
        if !path.is_file() {
            rec.absent.push(name.clone());
            continue;
        }
        match sha256_of(&path) {
            Ok(h) if h == entry.sha256 => {}
            Ok(_) => rec.drifted.push(name.clone()),
            Err(e) => {
                eprintln!("sync-regulatories: cannot hash {name}: {e}");
                return false;
            }
        }
    }
    if offline {
        for name in &local {
            if !manifest.files.contains_key(name) {
                rec.unmanifested.push(name.clone());
            }
        }
        return report(&rec, &manifest, json_out, true);
    }

    // ── Online half: the catalogue ───────────────────────────────────────────
    let remote = match fetch_catalogue() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sync-regulatories: cannot reach the BDEW catalogue: {e}");
            eprintln!(
                "  Re-run with --offline to check the mirror against the committed manifest."
            );
            return false;
        }
    };
    let remote = dedupe(remote);
    let today = today_iso();
    println!(
        "sync-regulatories: {} catalogue entries, {} mirrorable, {} in force on {today}",
        remote.len(),
        remote.iter().filter(|d| d.is_mirrorable()).count(),
        remote
            .iter()
            .filter(|d| d.is_mirrorable() && d.is_current(&today))
            .count()
    );

    // Index the mirror by the upstream id it came from, so a renamed local file
    // is still recognised as the same document.
    let by_id: BTreeMap<u64, &String> = manifest
        .files
        .iter()
        .map(|(name, e)| (e.id, name))
        .collect();

    let mut to_download: Vec<&RemoteDocument> = Vec::new();
    for doc in &remote {
        if !doc.is_mirrorable() || doc.is_informational() {
            continue;
        }
        if by_id.contains_key(&doc.id) {
            continue;
        }
        // Not in the manifest — but the mirror predates the manifest, so fall
        // back to matching the local file whose name carries the same identity.
        if find_local(doc, &local_tokens).is_some() {
            continue;
        }
        if doc.is_current(&today) {
            rec.missing.push(format!(
                "{} [{}] valid {} → {}",
                doc.title.trim(),
                doc.variant(),
                doc.valid_from.as_deref().unwrap_or("?"),
                doc.valid_to.as_deref().unwrap_or("open"),
            ));
            to_download.push(doc);
        } else {
            rec.superseded.push(doc.title.trim().to_owned());
        }
    }

    if download && !to_download.is_empty() {
        println!("\nDownloading {} document(s):", to_download.len());
        let mut failed: Vec<String> = Vec::new();
        for doc in &to_download {
            let Some(file_id) = doc.file_id else { continue };
            let name = doc.mirror_file_name();
            match download_file(file_id) {
                Ok(bytes) => {
                    let path = mirror.join(&name);
                    if let Err(e) = std::fs::write(&path, &bytes) {
                        eprintln!("  FAILED {name}: {e}");
                        failed.push(format!("{} — {e}", doc.title.trim()));
                        continue;
                    }
                    let mut h = Sha256::new();
                    h.update(&bytes);
                    manifest.files.insert(
                        name.clone(),
                        ManifestEntry {
                            id: doc.id,
                            file_id,
                            title: doc.title.trim().to_owned(),
                            valid_from: doc.valid_from.clone(),
                            valid_to: doc.valid_to.clone(),
                            sha256: format!("{:x}", h.finalize()),
                            correction_date: doc.correction_date.clone(),
                        },
                    );
                    println!("  {} ({} bytes)", name, bytes.len());
                }
                Err(e) => {
                    // One document that will not come down must not abandon the
                    // other twenty-four; the run reports what it could not get.
                    eprintln!("  FAILED {name}: {e}");
                    failed.push(format!("{} — {e}", doc.title.trim()));
                }
            }
        }
        rec.missing = failed;
    }

    // Adopt every already-mirrored document into the manifest, so the next run
    // can reconcile by id and `--offline` has something to check.
    let mut adopted = 0usize;
    for doc in &remote {
        let Some(file_id) = doc.file_id else { continue };
        if !doc.is_mirrorable() || doc.is_informational() {
            continue;
        }
        let Some(name) = find_local(doc, &local_tokens) else {
            continue;
        };
        if manifest.files.contains_key(&name) {
            continue;
        }
        let Ok(sha) = sha256_of(&mirror.join(&name)) else {
            continue;
        };
        manifest.files.insert(
            name,
            ManifestEntry {
                id: doc.id,
                file_id,
                title: doc.title.trim().to_owned(),
                valid_from: doc.valid_from.clone(),
                valid_to: doc.valid_to.clone(),
                sha256: sha,
                correction_date: doc.correction_date.clone(),
            },
        );
        adopted += 1;
    }
    if adopted > 0 {
        println!("adopted {adopted} already-mirrored document(s) into the manifest");
    }

    // After adoption, so a file the catalogue *does* describe is not reported
    // as unknown to it.
    for name in &local {
        if !manifest.files.contains_key(name) {
            rec.unmanifested.push(name.clone());
        }
    }

    manifest.note = "Generated by `cargo xtask sync-regulatories`. \
         Records which BDEW document each mirrored file came from, and the hash \
         of the bytes the profiles were read from — BDEW reissues corrections \
         under an unchanged version number, so the version alone cannot say \
         whether a local copy is current."
        .to_owned();
    manifest.synced_at = today.clone();
    let rendered = match serde_json::to_string_pretty(&manifest) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sync-regulatories: cannot render the manifest: {e}");
            return false;
        }
    };
    if let Err(e) = std::fs::write(&manifest_path, rendered + "\n") {
        eprintln!("sync-regulatories: cannot write the manifest: {e}");
        return false;
    }

    report(&rec, &manifest, json_out, false)
}

/// Print the reconciliation. Returns `true` when nothing needs attention.
fn report(rec: &Reconciliation, manifest: &Manifest, json_out: bool, offline: bool) -> bool {
    if json_out {
        match serde_json::to_string_pretty(rec) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("cannot render JSON: {e}"),
        }
    }
    println!(
        "\nmirror: {} file(s) with recorded provenance",
        manifest.files.len()
    );

    let mut ok = true;
    // Drift is the error: the bytes a profile was read from are not the bytes
    // on disk any more. Absence is not — see `Reconciliation::absent`.
    if !rec.drifted.is_empty() {
        ok = false;
        eprintln!(
            "\n{} file(s) changed since they were mirrored:",
            rec.drifted.len()
        );
        for f in &rec.drifted {
            eprintln!("  {f}");
        }
    }
    if !rec.absent.is_empty() {
        println!(
            "\n{} of {} recorded document(s) are not on disk — \
             `cargo xtask sync-regulatories --download` fetches them",
            rec.absent.len(),
            manifest.files.len()
        );
    }
    if !rec.missing.is_empty() {
        ok = false;
        eprintln!(
            "\n{} document(s) in force upstream but not mirrored:",
            rec.missing.len()
        );
        for f in &rec.missing {
            eprintln!("  {f}");
        }
        eprintln!("\nRe-run with --download to fetch them.");
    }
    if !rec.superseded.is_empty() {
        println!(
            "\n{} superseded document(s) upstream are not mirrored (expected — \
             nothing cites them)",
            rec.superseded.len()
        );
    }
    if !rec.unmanifested.is_empty() {
        println!(
            "\n{} local file(s) are not in the BDEW catalogue (BNetzA Festlegungen, \
             DVGW, hand-added sources)",
            rec.unmanifested.len()
        );
    }
    if ok {
        println!(
            "\nsync-regulatories: mirror consistent{}",
            if offline { " (offline check)" } else { "" }
        );
    }
    ok
}

// ── HTTP ─────────────────────────────────────────────────────────────────────

fn client() -> Result<reqwest::blocking::Client, reqwest::Error> {
    reqwest::blocking::Client::builder()
        // The MIG PDFs run to tens of megabytes and the catalogue serves them
        // slowly; a two-minute ceiling truncated the largest ones mid-body,
        // which reqwest reports only as "error decoding response body".
        .timeout(std::time::Duration::from_secs(600))
        .connect_timeout(std::time::Duration::from_secs(30))
        .user_agent("mako-xtask/sync-regulatories (+https://github.com/hupe1980/mako)")
        .build()
}

fn fetch_catalogue() -> Result<Vec<RemoteDocument>, Box<dyn std::error::Error>> {
    let resp = client()?.get(DOCUMENTS_API).send()?.error_for_status()?;
    Ok(resp.json::<ResponseModel>()?.data)
}

fn download_file(file_id: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let url = format!("{DOWNLOAD_API}/{file_id}");
    let resp = client()?.get(&url).send()?.error_for_status()?;
    Ok(resp.bytes()?.to_vec())
}

/// Today, as the catalogue writes its dates.
fn today_iso() -> String {
    let now = mako_fristen::heute();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(title: &str, file_type: &str) -> RemoteDocument {
        RemoteDocument {
            id: 1,
            file_id: Some(2),
            title: title.to_owned(),
            is_free: true,
            valid_from: None,
            valid_to: None,
            is_consolidated_reading_version: false,
            is_extraordinary_publication: false,
            is_error_correction: false,
            is_informational_reading_version: false,
            correction_date: None,
            file_type: Some(file_type.to_owned()),
        }
    }

    /// The generated name matches the convention already committed by hand, so
    /// a downloaded file lands beside its predecessors.
    #[test]
    fn the_mirror_name_follows_the_committed_convention() {
        assert_eq!(
            doc("UTILMD MIG Strom S2.1", "application/pdf").mirror_file_name(),
            "UTILMD_MIG_Strom_S2.1.pdf"
        );
        assert_eq!(
            doc("APERAK AHB 1.1 ", "application/pdf").mirror_file_name(),
            "APERAK_AHB_1.1.pdf"
        );
    }

    /// A `.docx` info sheet keeps its extension; the catalogue's MIME type is
    /// the only place that says so.
    #[test]
    fn the_extension_comes_from_the_mime_type() {
        assert_eq!(
            doc(
                "UTILMD MIG Strom S2 2 info Fehlerkorrektur",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            )
            .mirror_file_name(),
            "UTILMD_MIG_Strom_S2_2_info_Fehlerkorrektur.docx"
        );
    }

    /// A document with no `validTo` is open-ended and always current; one that
    /// expired yesterday is not.
    #[test]
    fn currency_is_decided_by_valid_to() {
        let mut d = doc("X", "application/pdf");
        assert!(d.is_current("2026-08-30"));
        d.valid_to = Some("2026-09-30T00:00:00".to_owned());
        assert!(d.is_current("2026-08-30"));
        d.valid_to = Some("2026-08-29T00:00:00".to_owned());
        assert!(!d.is_current("2026-08-30"));
    }

    /// A paid document has no `fileId` we may fetch, and a link-only entry has
    /// none at all.
    #[test]
    fn only_free_downloadable_entries_are_mirrorable() {
        let mut d = doc("X", "application/pdf");
        assert!(d.is_mirrorable());
        d.is_free = false;
        assert!(!d.is_mirrorable());
        d.is_free = true;
        d.file_id = None;
        assert!(!d.is_mirrorable());
    }

    /// The informational reading version is the same content, typeset for
    /// reading; mirroring it doubles the directory for no added fact.
    #[test]
    fn informational_reading_versions_are_skipped() {
        assert!(
            doc(
                "APERAK AHB 1.0 - informatorische Lesefassung",
                "application/pdf"
            )
            .is_informational()
        );
        assert!(!doc("APERAK AHB 1.0", "application/pdf").is_informational());
    }
}
