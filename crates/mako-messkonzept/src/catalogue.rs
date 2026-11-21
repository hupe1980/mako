//! Which compositions hold a bundle: the BDEW Codeliste's structures, and the
//! Netzbetreiber's own Messkonzepte. Both match by the same rule — the count
//! of each compared [`LokationKind`] within the composition's bounds.

use std::collections::{BTreeMap, BTreeSet};

use rubo4e::lokationsbuendel::{Flussrichtung, Objekttyp, STRUKTUREN};
use serde::{Deserialize, Serialize};

use crate::netzanschluss::{LokationKind, Lokationsbuendel};

/// One kind of Lokation in a composition, with its bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    /// The kind.
    pub kind: LokationKind,
    /// At least this many.
    pub min: u32,
    /// At most this many; `None` for no bound.
    #[serde(default)]
    pub max: Option<u32>,
}

/// Bounds per kind; a `None` max is unbounded.
type Bounds = BTreeMap<LokationKind, (u32, Option<u32>)>;

fn bounds(each: impl IntoIterator<Item = Bound>) -> Bounds {
    let mut b = Bounds::new();
    for o in each {
        let e = b.entry(o.kind).or_insert((0, Some(0)));
        e.0 += o.min;
        e.1 = e.1.zip(o.max).map(|(x, y)| x + y);
    }
    b
}

fn holds(b: &Bounds, buendel: &Lokationsbuendel) -> bool {
    let n = buendel.counts();
    let kinds: BTreeSet<_> = b.keys().chain(n.keys()).filter(|k| k.compared()).collect();
    kinds.into_iter().all(|k| {
        let c = n.get(k).copied().unwrap_or(0);
        let (min, max) = b.get(k).copied().unwrap_or((0, Some(0)));
        c >= min && max.is_none_or(|m| c <= m)
    })
}

fn kind(typ: Objekttyp, richtung: Option<Flussrichtung>) -> Option<LokationKind> {
    use Flussrichtung as F;
    use LokationKind as K;
    Some(match (typ, richtung) {
        (Objekttyp::Marktlokation, Some(F::Verbrauch)) => K::MaloConsumption,
        (Objekttyp::Marktlokation, Some(F::Erzeugung)) => K::MaloGeneration,
        (Objekttyp::Messlokation, _) => K::Melo,
        (Objekttyp::TechnischeRessource, Some(F::Verbrauch)) => K::TrConsumption,
        (Objekttyp::TechnischeRessource, Some(F::Erzeugung)) => K::TrGeneration,
        (Objekttyp::TechnischeRessource, Some(F::VerbrauchUndErzeugung)) => K::TrStorage,
        _ => return None,
    })
}

/// A structure of the BDEW Codeliste der Lokationsbündelstrukturen 1.0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lokationsbuendelstruktur {
    /// Its 13-digit code.
    pub code: &'static str,
    /// Its Bezeichnung in the Codeliste.
    pub bezeichnung: &'static str,
    /// Its objects as compared kinds, each with its Ebene.
    pub objects: Vec<(Bound, u8)>,
}

/// Every Codeliste structure that holds `buendel`. A structure carrying a
/// mandatory object no [`LokationKind`] names, other than its Netzlokation,
/// cannot hold one.
#[must_use]
pub fn structures(buendel: &Lokationsbuendel) -> Vec<Lokationsbuendelstruktur> {
    STRUKTUREN
        .iter()
        .filter_map(|s| {
            let mut objects = Vec::new();
            for o in s.objekte {
                let rolle = o.rolle();
                match kind(rolle.objekttyp, rolle.richtung) {
                    Some(kind) => objects.push((
                        Bound {
                            kind,
                            min: o.min,
                            max: o.max,
                        },
                        rolle.ebene,
                    )),
                    None if o.is_mandatory() && rolle.objekttyp != Objekttyp::Netzlokation => {
                        return None;
                    }
                    None => {}
                }
            }
            holds(&bounds(objects.iter().map(|(b, _)| *b)), buendel).then_some(
                Lokationsbuendelstruktur {
                    code: s.code,
                    bezeichnung: s.bezeichnung,
                    objects,
                },
            )
        })
        .collect()
}

/// One Messkonzept of a Netzbetreiber's catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Messkonzept {
    /// The operator's number, as its catalogue prints it („MK 15b“).
    pub number: String,
    /// Its title.
    pub title: String,
    /// The Lokationen it holds.
    pub objects: Vec<Bound>,
    /// The Codeliste structure it maps to, where the operator states one.
    #[serde(default)]
    pub lokationsbuendelcode: Option<String>,
    /// The publication it comes from — catalogue, edition, page.
    pub source: String,
}

/// A Netzbetreiber's Messkonzept catalogue — the only source that names a
/// Messkonzept.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Catalogue {
    /// Its Messkonzepte.
    pub messkonzepte: Vec<Messkonzept>,
}

/// The Messkonzepte that hold one bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Match<'a> {
    /// The bundle, by index into [`crate::Netzanschluss::lokationsbuendel`].
    pub buendel: usize,
    /// The Messkonzepte that hold it; empty when none does or the catalogue
    /// is empty.
    pub messkonzepte: Vec<&'a Messkonzept>,
}

impl Catalogue {
    /// For each bundle, the Messkonzepte that hold it.
    #[must_use]
    pub fn matching(&self, buendel: &[Lokationsbuendel]) -> Vec<Match<'_>> {
        buendel
            .iter()
            .enumerate()
            .map(|(i, b)| Match {
                buendel: i,
                messkonzepte: self
                    .messkonzepte
                    .iter()
                    .filter(|m| holds(&bounds(m.objects.iter().copied()), b))
                    .collect(),
            })
            .collect()
    }
}
