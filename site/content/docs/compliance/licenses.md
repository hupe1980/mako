+++
title = "License Governance"
description = "License governance for the mako workspace. SPDX identifiers allowed in deny.toml and rationale for each decision."
weight = 50
+++
The rationale for every SPDX licence identifier allowed in `deny.toml`.

---

## Standard Allowed Licences

Approved for all direct and transitive dependencies without further review.

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

---

## Licences Requiring Governance Review

Each entry records the decision and the transitive path that introduced the licence.

### `0BSD` — Zero-Clause BSD

**Status:** Approved  
**Approval owner:** project maintainer (see deny.toml commit)

**Rationale:** public-domain-equivalent — unrestricted use, modification and
distribution, no attribution requirement.

**Transitive path:** `mailparse` → `quoted_printable` (via `asx-rs`).

**Risk assessment:** None. The licence imposes no obligations. It is on the
[SPDX approved list](https://spdx.org/licenses/0BSD.html) and is OSI-approved.

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
3. Commit both files together: `cargo xtask check-licenses` fails the build when
   this page and `deny.toml` disagree in either direction.

**A note on copyleft strength.** *File-level* weak copyleft (`MPL-2.0`)
constrains only modifications to the licensed files themselves, so it is
reviewable — it is not in the allow list, and a dependency introducing it fails
`cargo deny` until one is done. *Library/linking-level* and *network* copyleft
(LGPL, AGPL) impose obligations on the combined or served work and are never
acceptable for this dual-MIT/Apache workspace.

Licences that are **never acceptable** (regardless of governance review):
- GPL-2.0-only, GPL-3.0-only (strong copyleft, incompatible with MIT/Apache dual-licence)
- LGPL-2.0-only, LGPL-2.1-only (linking-level copyleft; obligations on the linked binary)
- AGPL-3.0-only (network-copyleft)
- SSPL-1.0, BUSL-1.1 (source-available, not OSI-approved)
- CC-BY-SA, CC-BY-NC (non-commercial or share-alike)
