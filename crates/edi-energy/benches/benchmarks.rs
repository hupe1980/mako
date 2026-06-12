//! Criterion benchmarks for the edi-energy crate (Epic 24.1).
//!
//! Covers the three performance-critical paths:
//!
//! 1. **parse** — `parse()` on pre-built byte fixtures of increasing segment
//!    count.
//! 2. **serialize** — `EdiEnergyMessage::serialize()` for each message type.
//! 3. **build** — full builder pipeline (construct → serialize → parse).
//!
//! Run with:
//! ```text
//! cargo bench --bench benchmarks
//! ```
//! Filter to a single group:
//! ```text
//! cargo bench --bench benchmarks -- parse
//! cargo bench --bench benchmarks -- build
//! ```
#![allow(clippy::result_large_err)]

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use edi_energy::{EdiEnergyMessage, Pruefidentifikator, Release, parse, parse_envelope_only};

// ── Fixtures ──────────────────────────────────────────────────────────────────

/// Minimal UTILMD interchange used as a baseline parse fixture.
const UTILMD_MINIMAL: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20230101:102'\
RFF+Z13:REF001'\
NAD+MS+4012345000023::293'\
IDE+Z19+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

/// Minimal MSCONS interchange.
#[cfg(feature = "mscons")]
const MSCONS_MINIMAL: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013002::+9'\
DTM+137:20230101:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNS+D'\
LOC+172+51238696781'\
LIN+1'\
QTY+220:100.500:KWH'\
UNT+10+1'\
UNZ+1+1'";

/// Minimal APERAK interchange.
#[cfg(feature = "aperak")]
const APERAK_MINIMAL: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+APERAK:D:07B:UN:2.0a'\
BGM+1000+29001+9'\
DTM+137:20230101:102'\
RFF+ACW:REF001'\
ERC+Z01'\
FTX+AAI+++Bitte pruefen Sie Ihre Meldung'\
UNT+7+1'\
UNZ+1+1'";

/// Minimal CONTRL interchange.
#[cfg(feature = "contrl")]
const CONTRL_MINIMAL: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+CONTRL:D:3:UN:1.0a'\
UCI+INTER001+4012345000023:14+9900357000004:14+4'\
UNT+3+1'\
UNZ+1+1'";

// ── Parse benchmarks ──────────────────────────────────────────────────────────

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    #[cfg(feature = "utilmd")]
    group.bench_function("utilmd_minimal", |b| {
        b.iter(|| parse(black_box(UTILMD_MINIMAL)).unwrap())
    });

    #[cfg(feature = "mscons")]
    group.bench_function("mscons_minimal", |b| {
        b.iter(|| parse(black_box(MSCONS_MINIMAL)).unwrap())
    });

    #[cfg(feature = "aperak")]
    group.bench_function("aperak_minimal", |b| {
        b.iter(|| parse(black_box(APERAK_MINIMAL)).unwrap())
    });

    #[cfg(feature = "contrl")]
    group.bench_function("contrl_minimal", |b| {
        b.iter(|| parse(black_box(CONTRL_MINIMAL)).unwrap())
    });

    group.finish();
}

// ── Serialize benchmarks ──────────────────────────────────────────────────────

fn bench_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialize");

    #[cfg(feature = "utilmd")]
    {
        let msg = parse(UTILMD_MINIMAL).unwrap();
        group.bench_function("utilmd", |b| b.iter(|| msg.serialize().unwrap()));
    }

    #[cfg(feature = "mscons")]
    {
        let msg = parse(MSCONS_MINIMAL).unwrap();
        group.bench_function("mscons", |b| b.iter(|| msg.serialize().unwrap()));
    }

    #[cfg(feature = "aperak")]
    {
        let msg = parse(APERAK_MINIMAL).unwrap();
        group.bench_function("aperak", |b| b.iter(|| msg.serialize().unwrap()));
    }

    #[cfg(feature = "contrl")]
    {
        let msg = parse(CONTRL_MINIMAL).unwrap();
        group.bench_function("contrl", |b| b.iter(|| msg.serialize().unwrap()));
    }

    group.finish();
}

// ── Build benchmarks ──────────────────────────────────────────────────────────

fn bench_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("build");

    #[cfg(feature = "utilmd")]
    group.bench_function("utilmd_builder", |b| {
        use edi_energy::builders::UtilmdBuilder;
        b.iter(|| {
            UtilmdBuilder::new(Release::new(black_box("S2.1")))
                .pruefidentifikator(Pruefidentifikator::new(black_box(55001)).unwrap())
                .sender(black_box("4012345000023"))
                .receiver(black_box("9900357000004"))
                .serialize()
                .unwrap()
        })
    });

    #[cfg(feature = "mscons")]
    group.bench_function("mscons_builder", |b| {
        use edi_energy::builders::MsconsBuilder;
        b.iter(|| {
            MsconsBuilder::new(Release::new(black_box("2.4c")))
                .pruefidentifikator(Pruefidentifikator::new(black_box(13002)).unwrap())
                .sender(black_box("4012345000023"))
                .receiver(black_box("9900357000004"))
                .metering_point(black_box("51238696781"))
                .quantity(black_box("220"), black_box("100.500"), black_box("KWH"))
                .done()
                .serialize()
                .unwrap()
        })
    });

    #[cfg(feature = "aperak")]
    group.bench_function("aperak_builder", |b| {
        use edi_energy::builders::AperakBuilder;
        b.iter(|| {
            AperakBuilder::new(Release::new(black_box("2.0a")))
                .pruefidentifikator(Pruefidentifikator::new(black_box(29001)).unwrap())
                .sender(black_box("4012345000023"))
                .receiver(black_box("9900357000004"))
                .serialize()
                .unwrap()
        })
    });

    #[cfg(feature = "contrl")]
    group.bench_function("contrl_builder", |b| {
        use edi_energy::builders::ContrlBuilder;
        b.iter(|| {
            ContrlBuilder::new(Release::new(black_box("1.0a")))
                .interchange_ref(black_box("INTER001"))
                .sender(black_box("4012345000023"))
                .receiver(black_box("9900357000004"))
                .accept()
                .serialize()
                .unwrap()
        })
    });

    group.finish();
}

// ── Round-trip throughput ─────────────────────────────────────────────────────

/// Measures the end-to-end cost: build → serialize → parse.
fn bench_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip");

    #[cfg(feature = "utilmd")]
    {
        let pids: &[u32] = &[55001, 55002, 55003, 55004, 55005];
        for &pid_u32 in pids {
            group.bench_with_input(
                BenchmarkId::new("utilmd", pid_u32),
                &pid_u32,
                |b, &pid_u32| {
                    use edi_energy::builders::UtilmdBuilder;
                    b.iter(|| {
                        let bytes = UtilmdBuilder::new(Release::new("S2.1"))
                            .pruefidentifikator(Pruefidentifikator::new(pid_u32).unwrap())
                            .sender("4012345000023")
                            .receiver("9900357000004")
                            .serialize()
                            .unwrap();
                        parse(black_box(&bytes)).unwrap()
                    })
                },
            );
        }
    }

    group.finish();
}

// ── Validate throughput ───────────────────────────────────────────────────────

/// Measures the cost of `validate()` on pre-parsed messages.
fn bench_validate(c: &mut Criterion) {
    let mut group = c.benchmark_group("validate");

    #[cfg(feature = "utilmd")]
    {
        let msg = parse(UTILMD_MINIMAL).unwrap();
        group.bench_function("utilmd", |b| b.iter(|| black_box(&msg).validate().unwrap()));
    }

    #[cfg(feature = "mscons")]
    {
        let msg = parse(MSCONS_MINIMAL).unwrap();
        group.bench_function("mscons", |b| b.iter(|| black_box(&msg).validate().unwrap()));
    }

    #[cfg(feature = "aperak")]
    {
        let msg = parse(APERAK_MINIMAL).unwrap();
        group.bench_function("aperak", |b| b.iter(|| black_box(&msg).validate().unwrap()));
    }

    group.finish();
}

// ── Registry lookup benchmarks ────────────────────────────────────────────────

/// Measures `ReleaseRegistry::profile()` lookup latency including the
/// `now_utc()` syscall.  This is on the hot path for every `validate()` call.
fn bench_registry(c: &mut Criterion) {
    use edi_energy::{MessageType, ReleaseRegistry};
    let mut group = c.benchmark_group("registry");

    #[cfg(feature = "utilmd")]
    {
        let release = Release::new("S2.1");
        let registry = ReleaseRegistry::global();
        group.bench_function("profile_lookup_utilmd", |b| {
            b.iter(|| {
                registry
                    .profile(black_box(MessageType::Utilmd), black_box(&release))
                    .is_ok()
            })
        });
    }

    #[cfg(feature = "mscons")]
    {
        use edi_energy::releases;
        let release = releases::mscons_fv20261001().clone();
        let registry = ReleaseRegistry::global();
        group.bench_function("profile_lookup_mscons", |b| {
            b.iter(|| {
                registry
                    .profile(black_box(MessageType::Mscons), black_box(&release))
                    .is_ok()
            })
        });
    }

    group.finish();
}

// ── Validate on specific date (avoids syscall) ────────────────────────────────

/// Measures `validate_on_date()` which avoids the `now_utc()` syscall.
/// Comparing this against `validate()` shows the cost of the date lookup.
fn bench_validate_on_date(c: &mut Criterion) {
    use time::macros::date;
    let date = date!(2026 - 01 - 15);
    let mut group = c.benchmark_group("validate_on_date");

    #[cfg(feature = "utilmd")]
    {
        let msg = parse(UTILMD_MINIMAL).unwrap();
        group.bench_function("utilmd", |b| {
            b.iter(|| black_box(&msg).validate_on_date(black_box(date)).unwrap())
        });
    }

    #[cfg(feature = "mscons")]
    {
        let msg = parse(MSCONS_MINIMAL).unwrap();
        group.bench_function("mscons", |b| {
            b.iter(|| black_box(&msg).validate_on_date(black_box(date)).unwrap())
        });
    }

    group.finish();
}

// ── Interchange throughput benchmarks ─────────────────────────────────────────

/// Measures parse+validate throughput for a synthetic multi-message interchange.
///
/// Each iteration builds an N-message MSCONS interchange from scratch, then
/// parses and validates every message.  This exercises the full hot path
/// for AS4 adapter workloads: tokenization + dispatch + rule-pack evaluation.
fn bench_interchange_throughput(c: &mut Criterion) {
    use edi_energy::parse_interchange;
    use std::io::Cursor;

    let mut group = c.benchmark_group("interchange_throughput");
    // Build a 10-message, 100-message, and 1000-message MSCONS interchange
    // fixture once, then bench parse-only and parse+validate separately.
    for count in [10_usize, 100, 1000] {
        // Build the interchange bytes.
        let mut interchange = Vec::new();
        // UNB
        interchange
            .extend_from_slice(b"UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'");
        // UNH...UNT per message
        for i in 1..=count {
            let unh = format!(
                "UNH+{i}+MSCONS:D:04B:UN:2.4c'\
                 BGM+7:::+00013002::+9'\
                 DTM+137:20230101:102'\
                 NAD+MS+4012345000023::293'\
                 NAD+MR+9900357000004::293'\
                 UNS+D'\
                 LOC+172+51238696781'\
                 LIN+1'\
                 QTY+220:{i}.500:KWH'\
                 UNT+9+{i}'"
            );
            interchange.extend_from_slice(unh.as_bytes());
        }
        // UNZ
        let unz = format!("UNZ+{count}+1'");
        interchange.extend_from_slice(unz.as_bytes());
        let interchange = interchange.into_boxed_slice();

        #[cfg(feature = "mscons")]
        {
            let label = format!("mscons_{count}_parse");
            group.throughput(criterion::Throughput::Elements(count as u64));
            group.bench_function(&label, |b| {
                b.iter(|| {
                    let _ = parse_interchange(Cursor::new(black_box(interchange.as_ref())))
                        .collect::<Vec<_>>();
                })
            });

            let label_v = format!("mscons_{count}_parse_validate");
            group.bench_function(&label_v, |b| {
                b.iter(|| {
                    parse_interchange(Cursor::new(black_box(interchange.as_ref())))
                        .map(|r| r.and_then(|m| m.validate()))
                        .collect::<Vec<_>>()
                })
            });
        }
    }
    group.finish();
}

// ── LightMessage vs full-parse routing benchmark ──────────────────────────────

/// Compares `parse_envelope_only()` (routing-only) parse path against a full
/// `parse()` to expose the cost delta of loading all segment fields into owned
/// heap storage.  `parse_envelope_only` is the fast path used in AS4 dispatch
/// to extract only the message type and PID without materialising the full AST.
fn bench_light_vs_full_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("routing_parse");

    #[cfg(feature = "mscons")]
    {
        group.bench_function("mscons_envelope_only", |b| {
            b.iter(|| parse_envelope_only(black_box(MSCONS_MINIMAL)).unwrap())
        });
        group.bench_function("mscons_full", |b| {
            b.iter(|| parse(black_box(MSCONS_MINIMAL)).unwrap())
        });
    }

    #[cfg(feature = "utilmd")]
    {
        group.bench_function("utilmd_envelope_only", |b| {
            b.iter(|| parse_envelope_only(black_box(UTILMD_MINIMAL)).unwrap())
        });
        group.bench_function("utilmd_full", |b| {
            b.iter(|| parse(black_box(UTILMD_MINIMAL)).unwrap())
        });
    }

    group.finish();
}

// ── Cold validation (registry cache miss) benchmark ──────────────────────────

/// Measures validate_on_date() across a variety of PID-specific rule packs
/// to capture variance in rule evaluation cost between PIDs with few vs.
/// many AHB rules.
fn bench_validate_multi_pid(c: &mut Criterion) {
    use time::macros::date;
    let date = date!(2026 - 01 - 15);
    let mut group = c.benchmark_group("validate_multi_pid");

    #[cfg(feature = "utilmd")]
    {
        let pids: &[u32] = &[55001, 55002, 55003, 55004];
        for &pid in pids {
            use edi_energy::builders::UtilmdBuilder;
            let bytes = UtilmdBuilder::new(Release::new("S2.1"))
                .pruefidentifikator(Pruefidentifikator::new(pid).unwrap())
                .sender("4012345000023")
                .receiver("9900357000004")
                .serialize()
                .unwrap();
            let msg = parse(&bytes).unwrap();
            group.bench_with_input(BenchmarkId::new("utilmd", pid), &msg, |b, msg| {
                b.iter(|| black_box(msg).validate_on_date(black_box(date)))
            });
        }
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_parse,
    bench_serialize,
    bench_build,
    bench_roundtrip,
    bench_validate,
    bench_validate_on_date,
    bench_registry,
    bench_interchange_throughput,
    bench_light_vs_full_parse,
    bench_validate_multi_pid,
);
criterion_main!(benches);
