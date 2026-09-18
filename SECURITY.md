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
decides which obligations bind it; that determination is not yet written down,
and it changes who files the notification above rather than whether one is owed.

## Supported versions

mako is **unreleased** and pre-1.0. Only `main` is supported: fixes land there,
and no patch releases are made for earlier tags.

## What the artefact carries today

Stated plainly, because a security page that overstates is worse than none:

- `cargo-deny` runs over the full feature graph against the RustSec advisory
  database and the licence policy, and is part of the gate.
- The release pipeline publishes multi-platform images carrying **SLSA build
  provenance (`mode=max`) and an SBOM per platform**, and the release fails if
  either is missing after the manifest merge. Read them with
  `docker buildx imagetools inspect --format '{{ json .Provenance }}' <image>`
  and `--format '{{ json .SBOM }}'`.
- The images are **not signed**. Provenance says how they were built, not who
  vouches for them; a signature (cosign/Sigstore) is a separate thing mako does
  not do yet.
- There is **no in-binary dependency manifest** yet (`cargo auditable`), so an
  operator reads the SBOM from the image rather than from the binary.
