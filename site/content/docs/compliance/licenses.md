+++
title = "License Governance"
description = "License governance for the mako workspace. SPDX identifiers allowed in deny.toml and rationale for each decision."
weight = 50
+++
# License Governance

This document records the rationale for every SPDX license identifier allowed in
`deny.toml` and tracks decisions that required explicit governance review.

---

## Standard Allowed Licences

The following are approved for all direct and transitive dependencies without further
review. They are permissive OSI-approved licences commonly used in the Rust ecosystem.

| SPDX Identifier | Notes |
|---|---|
| `MIT` | Permissive; attribution only. |
| `Apache-2.0` | Permissive; includes patent grant. |
| `Apache-2.0 WITH LLVM-exception` | Apache-2.0 with an explicit LLVM linking exception. |
| `BSD-2-Clause` | Permissive; attribution only. |
| `BSD-3-Clause` | Permissive; attribution + non-endorsement clause. |
| `ISC` | Functionally equivalent to MIT/BSD-2-Clause. |
| `Unicode-3.0` | Unicode data files (ICU, Unicode tables). |
| `Zlib` | Permissive; commonly used in compression crates. |
| `CDLA-Permissive-2.0` | Community Data Licence Agreement (permissive variant). No copyleft conditions. |
| `MIT-0` | MIT without attribution requirement. More permissive than MIT. |
| `bzip2-1.0.6` | bzip2 compression library licence — BSD-like, no restrictions. |
| `CC0-1.0` | Creative Commons Zero — public-domain dedication, no conditions. |
| `BSL-1.0` | Boost Software License 1.0 — OSI-approved, permissive. |

---

## Licences Requiring Governance Review

The following licences required an explicit decision before being added to `deny.toml`.
Each entry documents the rationale and the transitive path that introduced the licence.

### `0BSD` — Zero-Clause BSD

**Status:** Approved  
**Added:** this session  
**Approval owner:** project maintainer (see deny.toml commit)

**Rationale:**  
`0BSD` (Zero-Clause BSD) is a public-domain-equivalent licence: it permits unrestricted
use, modification, and distribution without any attribution requirement. It is more
permissive than `MIT` and imposes no conditions whatsoever.

**Transitive path:** `mailparse` → `quoted_printable` (via `asx-rs`).

**Risk assessment:** None. The licence imposes no obligations. It is on the
[SPDX approved list](https://spdx.org/licenses/0BSD.html) and is OSI-approved.

---

### `CDDL-1.0` — Common Development and Distribution License 1.0

**Status:** Approved  
**Approval owner:** project maintainer (see deny.toml commit)

**Rationale:**  
`CDDL-1.0` is **file-level** (weak) copyleft: only files already under CDDL that we
*modify* must remain under CDDL. It imposes no obligation on the rest of the workspace
and no linking or network-use conditions. mako consumes the affected crate unmodified,
so no source-disclosure obligation is triggered.

**Transitive path:** `inferno` → `flamegraph` (lancedb profiling, `agentd`).

**Risk assessment:** Low. OSI-approved and FSF Free/Libre. File-level copyleft only
bites on modification of the CDDL-licensed files themselves, which we do not do.

---

### `MPL-2.0` — Mozilla Public License 2.0

**Status:** Approved  
**Approval owner:** project maintainer (see deny.toml commit)

**Rationale:**  
`MPL-2.0` is **file-level** (weak) copyleft, like CDDL: modifications to MPL-covered
files must be released under MPL, but the licence explicitly permits combining MPL code
with proprietary/permissively-licensed code in a larger work without relicensing that
work. mako consumes the affected crate unmodified.

**Transitive path:** `option-ext` (via `lance`/`lancedb`, `agentd`).

**Risk assessment:** Low. OSI-approved and FSF Free/Libre. No obligation on the
combined work; the file-level share-alike applies only to modified MPL files.

---

## Review Process

When a new non-standard licence needs to be added to `deny.toml`:

1. Add it to `deny.toml` with a `# reason:` comment identifying the transitive crate.
2. Add an entry to the **Licences Requiring Governance Review** table above with:
   - SPDX identifier
   - Approval status and date
   - Approval owner
   - Rationale (< 3 sentences)
   - Transitive dependency path
   - Risk assessment
3. Commit both files together so `deny.toml` and this document are always in sync.

**A note on copyleft strength.** mako distinguishes *file-level* weak copyleft
(`MPL-2.0`, `CDDL-1.0`) — which only constrains modifications to the licensed files
themselves and is **allowed** with governance review — from *library/linking-level* and
*network* copyleft (LGPL, AGPL), which impose obligations on the combined or served work
and are **never acceptable** for this dual-MIT/Apache workspace.

Licences that are **never acceptable** (regardless of governance review):
- GPL-2.0-only, GPL-3.0-only (strong copyleft, incompatible with MIT/Apache dual-licence)
- LGPL-2.0-only, LGPL-2.1-only (linking-level copyleft; obligations on the linked binary)
- AGPL-3.0-only (network-copyleft)
- SSPL-1.0, BUSL-1.1 (source-available, not OSI-approved)
- CC-BY-SA, CC-BY-NC (non-commercial or share-alike)
