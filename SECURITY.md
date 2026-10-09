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

mako is a product with digital elements under **Verordnung (EU) 2024/2847**
(Cyber Resilience Act). Its Article 14 reporting obligations have applied since
**11 September 2026**, and they run on their own clock, independently of this
page's timetable and of whether a fix exists yet.

For an **actively exploited vulnerability** in mako, or a **severe incident**
affecting its security, the manufacturer notifies through the **CRA Single
Reporting Platform**, addressed to the CSIRT of the Member State of its main
establishment and made available to ENISA at the same time:

| | Actively exploited vulnerability | Severe incident |
|---|---|---|
| **Early warning** | within **24 hours** of becoming aware | within **24 hours** |
| **Full notification** | within **72 hours** | within **72 hours** |
| **Final report** | no later than **14 days** after a corrective measure is available | within **one month** of the 72-hour notification |

"Becoming aware" starts the 24 hours, not "having confirmed" — so a credible
report of exploitation is escalated before it is fully triaged.

Whether mako ships as **manufacturer** or as **open-source software steward**
decides which obligations bind it. This page does not make that determination;
it changes who files the notification above, not whether one is owed.

## Supported versions

mako is unreleased and pre-1.0. Fixes land on `main` and ship in the next release; no patch
releases are made for earlier tags.

## What a release carries

Stated plainly, because a security page that overstates is worse than none:

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
  the same way: no registry token is stored in the repository.
- Every workflow action is pinned to a commit SHA, every base image to a
  digest, and `cargo xtask check-action-pins` refuses a tag pin.
- Each release carries an **evidence pack** (`evidence-vX.Y.Z.json`): the
  extraction-fidelity budgets and measured values, every `just ci` guard with
  its exit status, the SBOM digests and the attestations; a figure that was not
  measured reads `not measured`.

The release process and how to check each artefact are on the
[Release Lifecycle](https://hupe1980.github.io/mako/docs/compliance/release-lifecycle/#what-a-release-carries)
page.
