//! An adapter reads the Prüfidentifikator where the message carries it.
//!
//! REMADV and INSRPT carry the PID in `SG1 RFF+Z13`; their `BGM` DE 1004 is the
//! Dokumentennummer, which a sender is free to make non-numeric. An adapter
//! that reads the PID out of `BGM` refuses every such conformant message, and
//! a fixture whose Dokumentennummer happens to be the PID hides it — so these
//! fixtures name a Dokumentennummer that is no PID.

use std::any::Any;

use mako_engine::types::Sparte;
use mako_engine::version::FormatVersion;
use mako_invoic::InvoicCommand;
use makod::adapters::{
    gpke_abrechnung_remadv_registry, wim_insrpt_registry, wim_invoic_remadv_registry,
};

const REMADV: &str = "UNB+UNOC:3+4012345000023:14+9900357000004:500+260401:0000+1'\
UNH+1+REMADV:D:05A:UN:2.9e'\
BGM+239+AV-2026-0815'\
DTM+137:202604010000?+00:303'\
RFF+Z13:33001'\
NAD+MS+4012345000023::293'\
CTA+IC+:Abrechnungsstelle'\
COM+030 12345678:TE'\
CUX+2:EUR:9'\
DOC+380+REF001'\
MOA+9:100.00:EUR'\
UNS+D'\
MOA+9:100.00:EUR'\
AJT+Z01'\
UNT+14+1'\
UNZ+1+1'";

const INSRPT: &str = "UNB+UNOC:3+4012345000023:14+9900357000004:500+260101:0000+1'\
UNH+1+INSRPT:D:10A:UN:1.1a'\
BGM+4+STOER-0815'\
DTM+137:202610010000?+00:303'\
NAD+MR+9900357000004::293'\
NAD+MS+4012345000023::9'\
DOC+23+DOK23012'\
RFF+Z13:23012'\
RFF+TN:REF23012'\
LIN+1'\
DTM+164:20261001:102'\
STS+Z06+Z09+Z75'\
NAD+DP'\
LOC+172+23012000008'\
UNT+14+1'\
UNZ+1+1'";

fn remadv_pid(cmd: &InvoicCommand) -> u32 {
    match cmd {
        InvoicCommand::ReceiveRemadv { pid, .. } => pid.as_u32(),
        _ => panic!("expected ReceiveRemadv"),
    }
}

#[test]
fn both_remadv_adapters_read_the_pid_from_rff_z13() {
    let msg = edi_energy::parse(REMADV.as_bytes()).expect("a conformant REMADV parses");
    let fv = FormatVersion::new("FV2026-04-01");
    let gpke = gpke_abrechnung_remadv_registry()
        .dispatch(&msg as &dyn Any, &fv)
        .unwrap_or_else(|e| panic!("the GPKE REMADV adapter reads RFF+Z13: {e}"));
    let wim = wim_invoic_remadv_registry()
        .dispatch(&msg as &dyn Any, &fv)
        .unwrap_or_else(|e| panic!("the WiM REMADV adapter reads RFF+Z13: {e}"));
    assert_eq!(remadv_pid(&gpke), 33001);
    assert_eq!(remadv_pid(&wim), 33001);
}

#[test]
fn the_insrpt_adapter_reads_the_pid_from_rff_z13() {
    let msg = edi_energy::parse(INSRPT.as_bytes()).expect("a conformant INSRPT parses");
    wim_insrpt_registry(
        time::OffsetDateTime::now_utc(),
        Sparte::Strom,
        mako_fristen::antwort::Messtechnik::KmeOhneRlm,
    )
    .dispatch(&msg as &dyn Any, &FormatVersion::new("FV2026-01-01"))
    .unwrap_or_else(|e| panic!("the INSRPT adapter reads RFF+Z13: {e}"));
}
