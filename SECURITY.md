# Security policy

mako is software for operators of critical infrastructure. A defect here reaches
market communication, metering and settlement, so this page says how to report
one and what happens after you do.

## Reporting a vulnerability

**Report privately. Do not open a public issue.**

Use GitHub's private vulnerability reporting on this repository —
**Security → Report a vulnerability** — which opens a channel visible only to
you and the maintainers.

If that button is not there, the repository has the feature switched off and
that is our bug, not yours: open a public issue saying only *"security report,
no details"* with no reproduction and no affected component, and a maintainer
will open the private channel and come to you. Never put the details in it.

Include whatever you have: the affected version or commit, the component
(`crates/<name>` or `services/<name>`), what an attacker gains, and the smallest
input that shows it. A report that is hard to reproduce is still worth sending;
say so and send it.

**Report a vulnerability in a dependency to that project**, not here — unless
mako's own use of it is what makes it exploitable, which is worth a report.

## What we do with it

| | |
|---|---|
| **Acknowledge** | within **3 working days** (Werktage, Europe/Berlin) |
| **First assessment** — affected versions, severity, whether it is being exploited | within **10 working days** |
| **Fix or written position** | tracked from the assessment, with the reporter kept informed |
| **Disclosure** | coordinated with the reporter, and by default after a fix is available |

We credit reporters by name unless you ask us not to. We will not take legal
action over a report made in good faith under this policy.

## Regulatory reporting

**mako is outside the scope of Regulation (EU) 2024/2847** (Cyber Resilience
Act). It is free and open source and is not supplied in the course of a
commercial activity — no price, no paid support, no hosting, no licence — so it
is not „made available on the market" (Art. 2(1), Art. 3(22), recitals 15 and
18). This is the owner's determination of **08.11.2026**; its basis, and what
would reopen it, are on the
[CRA and AI Act](https://hupe1980.github.io/mako/docs/compliance/eu-regulations/)
page.

**No Art. 14 early warning is owed for mako.** Art. 14 is headed „Reporting
obligations of manufacturers", and while mako is outside scope it has no
manufacturer. A report is handled under this page's timetable instead, and the
maintainer may still notify an actively exploited vulnerability to a CSIRT
designated as coordinator or to ENISA on a voluntary basis (Art. 15(1)).

The CRA's three outcomes, and what each would ask:

| Outcome | Source | Reporting duty |
|---|---|---|
| **Outside scope** — the current determination | Art. 2(1), Art. 3(22), recitals 15 and 18 | none |
| **Manufacturer** — paid support, hosting or licensing makes the supply commercial | Art. 3(13) | Art. 14 in full, applicable since 11 September 2026 (Art. 71(2)), also for a product placed on the market before 11 December 2027 (Art. 69(3)) |
| **Open-source software steward** — needs „a legal person, other than a manufacturer" to support mako | Art. 3(14) | Art. 14(1) „to the extent that they are involved in the development" (Art. 24(3)), from 11 December 2027 (Art. 71(2)) |

### If the determination changes

The owner becomes the manufacturer the moment mako is supplied commercially, and
the determination is reopened before that, not after. The manufacturer then
notifies through ENISA's **CRA Single Reporting Platform**
(<https://portal.cra-srp.enisa.europa.eu/>), using the electronic notification
end-point of the CSIRT designated as coordinator of the Member State of its main
establishment, simultaneously accessible to ENISA (Art. 14(7)). The
manufacturer's duty under Art. 14(2) and (4):

| | Actively exploited vulnerability — Art. 14(2) | Severe incident — Art. 14(4) |
|---|---|---|
| **Early warning** | within **24 hours** of becoming aware | within **24 hours** of becoming aware |
| **Notification** | within **72 hours** of becoming aware | within **72 hours** of becoming aware |
| **Final report** | no later than **14 days** after a corrective or mitigating measure is available | within **one month** after the incident notification |

"Becoming aware" starts the 24 hours, not "having confirmed" — so under that
outcome a credible report of exploitation is escalated before it is fully
triaged.

### Dependencies published by the same maintainer

Twelve of mako's 87 direct external dependencies are published from the
maintainer's own crates.io account and no other: `agentplane`, `asx-rs`,
`billing`, `doubleentry`, `edifact-rs`, `en16931`, `en16931-formats`, `krafka`,
`metering`, `meterstore`, `rubo4e` and `sepa`. Each is free and open source and
supplied on the same terms as mako, so the determination above covers them too.

They share mako's single point of failure: one person receives, assesses and
fixes a report for any of them. A vulnerability in one of the twelve may be
reported here or to that crate's own repository — both reach the same
maintainer and run on the timetable above. The fix is released in the crate
first, and mako takes it in its next release; the disclosure names both.

## Supported versions

mako is unreleased and pre-1.0. Fixes land on `main` and ship in the next release; no patch
releases are made for earlier tags.

## What a release carries

- `cargo-deny` runs over the full feature graph against the RustSec advisory
  database and the licence policy, and is part of the gate.
- The release pipeline publishes multi-platform images carrying **SLSA build
  provenance (`mode=max`) and an SBOM per platform**, and the release fails if
  either is missing after the manifest merge. Read them with
  `docker buildx imagetools inspect --format '{{ json .Provenance }}' <image>`
  and `--format '{{ json .SBOM }}'`.
- Every service binary embeds its **dependency manifest** (`cargo auditable`,
  `.dep-v0` section), and it survives `strip`: read it from a binary copied out
  of an image with `rust-audit-info <binary>` or `cargo audit bin <binary>`. The
  image build and the release both fail on a binary without one.
- Each release publishes a **CycloneDX SBOM per service binary** and a
  **VEX document** (`release/vex.json`, OpenVEX) with one statement for every
  advisory `deny.toml` accepts, as GitHub Release assets.
- Each image and each binary inside it carries **GitHub artifact attestations**
  — build provenance and its CycloneDX SBOM — as keyless Sigstore bundles signed
  with the release workflow's identity. Verify with
  `gh attestation verify oci://ghcr.io/hupe1980/mako-<service>:<version> --repo hupe1980/mako`.
- Crates reach crates.io by **Trusted Publishing** and the Python package PyPI
  the same way: the release workflow reads no stored registry token — each
  publish uses a short-lived token minted from the job's OIDC identity.
- Every workflow action is pinned to a commit SHA, every base image to a
  digest, and `cargo xtask check-action-pins` refuses a tag pin.
- Each release carries an **evidence pack** (`evidence-vX.Y.Z.json`): the
  extraction-fidelity budgets and measured values, the exit status of each
  `just ci` recipe the release runs, the SBOM digests and the attestations; a
  figure that was not measured reads `not measured`. Every guard verdict is
  taken before anything is published, and a failing guard stops the release.

The release process and how to check each artefact are on the
[Release Lifecycle](https://hupe1980.github.io/mako/docs/compliance/release-lifecycle/#what-a-release-carries)
page.
