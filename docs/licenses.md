---
layout: default
title: License Governance
nav_order: 50
description: >
  License governance for the mako workspace. SPDX identifiers allowed in
  deny.toml and rationale for each decision.
---

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
| `Unicode-DFS-2016` | Unicode Data Files and Software licence (pre-3.0 form). |
| `Zlib` | Permissive; commonly used in compression crates. |
| `CDLA-Permissive-2.0` | Community Data Licence Agreement (permissive variant). No copyleft conditions. |
| `MIT-0` | MIT without attribution requirement. More permissive than MIT. |

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

Licences that are **never acceptable** (regardless of governance review):
- GPL-2.0-only, GPL-3.0-only (copyleft, incompatible with MIT/Apache dual-licence)
- LGPL-2.0-only, LGPL-2.1-only (weak copyleft; may require LGPL notices in binaries)
- AGPL-3.0-only (network-copyleft)
- SSPL-1.0, BUSL-1.1 (source-available, not OSI-approved)
- CC-BY-SA, CC-BY-NC (non-commercial or share-alike)
