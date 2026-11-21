//! The NB's own change of the Lokationsbündelstruktur — UTILMD 55173 to the
//! MSB, 55175 to the LF (GPKE Teil 4 Kap. 1.4.2, Prozessschritte 1 and 4).
//!
//! The bündel is the one marktd holds on the Gültigkeitstag, sent whole: every
//! object as an `SG5 LOC` of Verwendungszeitraum 1, the structure in `SG8
//! SEQ+Z78 RFF+Z31`, each object's `SEQ+Z58` with its `RFF+Z33` Objektcode
//! (UTILMD AHB Strom 2.2 Kap. 7). A fact marktd does not hold refuses the
//! command naming it; nothing is sent that its profile would refuse. The
//! Rückmeldung 55177/55180 is due by the 2. WT.

use mako_engine::payload::{Stammdaten, StammdatenBlock, StammdatenLokation, StammdatenSegment};
use mako_gpke::stammdatenaenderung::{
    GpkeStammdatenaenderungWorkflow, RUECKMELDUNG_WINDOW_LABEL, StammdatenCommand, WORKFLOW_NAME,
};
use mako_markt::domain::Lokationstyp;
use mako_markt::lokationsbuendel::MaloLokationsbuendel;
use rubo4e::lokationsbuendel::{Flexibilitaet, Lokationsbuendelstruktur};

use super::*;

/// The Verwendungszeitraum the change names — one, from the Gültigkeitstag.
const ZEITRAUM: u32 = 1;

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    /// The Marktlokation whose bündel changed.
    malo_id: String,
    /// 55173 (to the MSB) or 55175 (to the LF).
    pid: u32,
    /// The Berechtigter's MP-ID.
    empfaenger_mp_id: String,
    /// The day the new structure applies (`YYYY-MM-DD` or `YYYYMMDD`).
    gueltig_ab: String,
}

pub(super) fn cmd_gpke_lokationsbuendel_aendern<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch(s, p))
}

async fn dispatch(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let body: Body = serde_json::from_value(payload.clone())
        .map_err(|e| DispatchError::InvalidPayload(format!("payload: {e}")))?;
    if !matches!(body.pid, 55_173 | 55_175) {
        return Err(DispatchError::InvalidPayload(format!(
            "\"pid\" {} — a change of the Lokationsbündelstruktur is 55173 (to the MSB) or \
             55175 (to the LF)",
            body.pid
        )));
    }
    let malo_id = extract_malo_id(&serde_json::json!({ "malo_id": body.malo_id }))?;
    let gueltig_ab = super::eog_stammdaten::stichtag(&body.gueltig_ab)?;
    let lb = super::gda_stammdaten::lesen(state, malo_id.as_str(), gueltig_ab).await?;
    let stammdaten = stammdaten(&lb, gueltig_ab)?;

    let pid = Pruefidentifikator::new(body.pid).map_err(DispatchError::InvalidPayload)?;
    let domain_cmd = StammdatenCommand::SendAenderung {
        pid,
        sender: MarktpartnerCode::new(state.own_mp_id(&["NB"], "")?),
        receiver: MarktpartnerCode::new(body.empfaenger_mp_id.trim()),
        location_id: malo_id.clone(),
        aenderungsdatum: format!(
            "{:04}{:02}{:02}",
            gueltig_ab.year(),
            u8::from(gueltig_ab.month()),
            gueltig_ab.day()
        ),
        stammdaten,
    };
    {
        use mako_engine::workflow::Workflow as _;
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &mako_gpke::stammdatenaenderung::StammdatenState::New,
            domain_cmd.clone(),
        )
        .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
        super::eog_stammdaten::refuse_what_the_gate_refuses(
            state,
            &latest_format_version(),
            &out.outbox,
        )?;
    }

    // GPKE Teil 4 Kap. 1.4.2 Nr. 2 / Nr. 5: the Rückmeldung „spätester ÜT ist
    // der 2. WT nach dem ÜT"; silence is a Qualitätsrückmeldung without
    // diverging content.
    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        mako_fristen::antwort::STAMMDATEN_RUECKMELDUNG_WERKTAGE,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    let process_id = spawn_process::<GpkeStammdatenaenderungWorkflow>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(WORKFLOW_NAME, latest_format_version()),
        domain_cmd,
        &[(RUECKMELDUNG_WINDOW_LABEL, due_at)],
        &[malo_id.as_str()],
    )
    .await?;
    Ok(DispatchOutcome::Spawned { process_id })
}

/// The `SG5`, `SG6` and `SG8` of the change, from the bündel marktd holds.
///
/// # Errors
///
/// [`DispatchError::StammdatenUnvollstaendig`] naming every fact the change
/// needs and marktd does not hold.
fn stammdaten(
    lb: &MaloLokationsbuendel,
    gueltig_ab: time::Date,
) -> Result<Stammdaten, DispatchError> {
    let mut fehlend = Vec::new();
    let code = match lb.lokationsbuendelcodes.as_slice() {
        [code] => Some(code.as_str()),
        [] => {
            fehlend
                .push("the bündel's lokationsbuendelcode (SG8 SEQ+Z78 RFF+Z31, Muss)".to_owned());
            None
        }
        many => {
            fehlend.push(format!(
                "the bündel's Lokationszuordnungen name different Lokationsbündelstrukturen ({})",
                many.join(", ")
            ));
            None
        }
    };
    let struktur = code.and_then(|c| {
        let s = Lokationsbuendelstruktur::from_wire(c);
        if s.is_none() {
            fehlend.push(format!(
                "lokationsbuendelcode {c} is no structure of the Codeliste der \
                 Lokationsbündelstrukturen 1.0"
            ));
        }
        s
    });

    let mut lokationen = Vec::new();
    let mut objekte = Vec::new();
    for o in &lb.objekte {
        let (loc, rff) = match o.typ {
            Lokationstyp::Malo => ("Z16", Some("Z18")),
            Lokationstyp::Melo => ("Z17", Some("Z19")),
            Lokationstyp::Nelo => ("Z18", Some("Z32")),
            Lokationstyp::Tr => ("Z20", Some("Z37")),
            // `SEQ+Z58` names no Steuerbare Ressource (Nr 00075), and the
            // Codeliste gives it no Objektcode.
            Lokationstyp::Sr => ("Z19", None),
        };
        lokationen.push(StammdatenLokation {
            qualifier: loc.to_owned(),
            id: o.id.clone(),
            zeitraum_id: Some(ZEITRAUM),
        });
        let Some(rff) = rff else { continue };
        let Some(objektcode) = o.objektcode.as_deref() else {
            fehlend.push(format!(
                "{}: lokationsbuendelObjektcode (SG8 SEQ+Z58 RFF+Z33)",
                o.id
            ));
            continue;
        };
        if let Some(s) = struktur {
            match s.objekt(objektcode) {
                None => fehlend.push(format!(
                    "{}: Objektcode {objektcode} is not part of the Lokationsbündelstruktur {}",
                    o.id, s.code
                )),
                Some(so) if so.flexibilitaet == Flexibilitaet::Flexibel => fehlend.push(format!(
                    "{}: Objektcode {objektcode} is a flexible object, whose references (SG8 \
                     RFF+Z34/Z35) are not assembled",
                    o.id
                )),
                Some(_) => {}
            }
        }
        objekte.push(StammdatenBlock {
            zeitraum_id: Some(ZEITRAUM),
            seq: "Z58".to_owned(),
            segmente: vec![rff_segment(rff, &o.id), rff_segment("Z33", objektcode)],
        });
    }
    if !fehlend.is_empty() {
        return Err(DispatchError::StammdatenUnvollstaendig {
            malo_id: lb.malo_id.clone(),
            fehlend,
        });
    }
    let code = code.unwrap_or_default();

    // `SG6 RFF+Z49` „Gültige Daten" with its Zeitraum-ID and `DTM+Z25`, the
    // UTC instant the Gültigkeitstag begins in German time.
    let ab = mako_fristen::berlin_midnight(gueltig_ab);
    let instant = format!(
        "{:04}{:02}{:02}{:02}{:02}+00",
        ab.year(),
        u8::from(ab.month()),
        ab.day(),
        ab.hour(),
        ab.minute()
    );
    let sg6 = vec![
        StammdatenSegment {
            tag: "RFF".to_owned(),
            elemente: vec![vec!["Z49".to_owned(), String::new(), ZEITRAUM.to_string()]],
        },
        StammdatenSegment {
            tag: "DTM".to_owned(),
            elemente: vec![vec!["Z25".to_owned(), instant, "303".to_owned()]],
        },
    ];
    let mut bloecke = vec![StammdatenBlock {
        zeitraum_id: Some(ZEITRAUM),
        seq: "Z78".to_owned(),
        segmente: vec![rff_segment("Z31", code)],
    }];
    bloecke.extend(objekte);
    Ok(Stammdaten {
        lokationen,
        sg6,
        bloecke,
        ..Stammdaten::default()
    })
}

fn rff_segment(qualifier: &str, referenz: &str) -> StammdatenSegment {
    StammdatenSegment {
        tag: "RFF".to_owned(),
        elemente: vec![vec![qualifier.to_owned(), referenz.to_owned()]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::ids::TenantId;
    use mako_engine::workflow::Workflow as _;
    use mako_gpke::stammdatenaenderung::StammdatenState;
    use mako_markt::lokationsbuendel::BuendelObjekt;

    const MALO: &str = "51238696012";
    const MELO: &str = "DE0000011000000000000000012345678";

    fn objekt(typ: Lokationstyp, id: &str, code: Option<&str>) -> BuendelObjekt {
        BuendelObjekt {
            typ,
            id: id.to_owned(),
            objektcode: code.map(str::to_owned),
        }
    }

    fn buendel(objekte: Vec<BuendelObjekt>) -> MaloLokationsbuendel {
        MaloLokationsbuendel {
            malo_id: MALO.to_owned(),
            stichtag: time::macros::date!(2026 - 10 - 15),
            energierichtung: None,
            lokationsbuendelcodes: vec!["9992000000026".to_owned()],
            objekte,
            zuordnungen: Vec::new(),
            angaben: None,
            zaehler: Vec::new(),
        }
    }

    /// The bündel marktd holds becomes a 55173 the pre-send gate passes:
    /// `SEQ+Z78` with the structure and one `SEQ+Z58` per object.
    #[test]
    fn a_standard_buendel_becomes_a_55173_its_column_accepts() {
        let lb = buendel(vec![
            objekt(Lokationstyp::Malo, MALO, Some("9992000001016")),
            objekt(Lokationstyp::Melo, MELO, Some("9992000001032")),
        ]);
        let stammdaten = stammdaten(&lb, lb.stichtag).expect("complete");
        assert_eq!(
            stammdaten.bloecke.iter().filter(|b| b.seq == "Z58").count(),
            2
        );
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &StammdatenState::New,
            StammdatenCommand::SendAenderung {
                pid: Pruefidentifikator::new(55_173).expect("pid"),
                sender: MarktpartnerCode::new("9900259000001"),
                receiver: MarktpartnerCode::new("9900259000002"),
                location_id: extract_malo_id(&serde_json::json!({ "malo_id": MALO }))
                    .expect("malo"),
                aenderungsdatum: "20261015".to_owned(),
                stammdaten,
            },
        )
        .expect("sends");
        let findings = crate::orchestrator::edifact_renderer::utilmd_ahb_findings(
            &out.outbox[0],
            TenantId::from_tenant("9900259000001"),
            &latest_format_version(),
        )
        .expect("renders");
        assert!(findings.is_empty(), "{findings:#?}");
    }

    /// An object without its Objektcode is refused by name, not sent.
    #[test]
    fn an_object_without_its_objektcode_is_refused_by_name() {
        let lb = buendel(vec![
            objekt(Lokationstyp::Malo, MALO, Some("9992000001016")),
            objekt(Lokationstyp::Melo, MELO, None),
        ]);
        let Err(DispatchError::StammdatenUnvollstaendig { fehlend, .. }) =
            stammdaten(&lb, lb.stichtag)
        else {
            panic!("refused");
        };
        assert!(fehlend.iter().any(|f| f.contains(MELO)), "{fehlend:?}");
    }
}
