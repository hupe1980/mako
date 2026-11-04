+++
title = "License Governance"
description = "License governance for the mako workspace: where the allow list lives, how a licence is added, and which licences are never acceptable."
weight = 50
+++
The licences this workspace may depend on are the `[licenses] allow` list in
[`deny.toml`](https://github.com/hupe1980/mako/blob/main/deny.toml), and
`cargo deny` refuses any dependency outside it — in `just ci` and in the release.
The Desk's npm tree is held to the same list: `cargo xtask check-desk` reads
every package's licence from `desk/package-lock.json` and refuses one the list
does not allow. Every identifier beyond the plain permissive ones carries its
reason and the transitive package that introduced it as a comment beside it.

## Review Process

When a new licence needs to be allowed:

1. Add it to `deny.toml`'s `allow` list with a comment naming the transitive
   crate that introduced it and why the licence is acceptable.
2. Have the project maintainer approve it in the pull request that adds it.

**A note on copyleft strength.** *File-level* weak copyleft (`MPL-2.0`)
constrains only modifications to the licensed files themselves, so it is
reviewable. It is allowed for one reviewed case — lightningcss, the CSS
transformer Vite runs when building the Desk, which ships no MPL file in the
bundle. *Library/linking-level* and *network* copyleft
(LGPL, AGPL) impose obligations on the combined or served work and are never
acceptable for this dual-MIT/Apache workspace.

Licences that are **never acceptable** (regardless of governance review):
- GPL-2.0-only, GPL-3.0-only (strong copyleft, incompatible with MIT/Apache dual-licence)
- LGPL-2.0-only, LGPL-2.1-only (linking-level copyleft; obligations on the linked binary)
- AGPL-3.0-only (network-copyleft)
- SSPL-1.0, BUSL-1.1 (source-available, not OSI-approved)
- CC-BY-SA, CC-BY-NC (non-commercial or share-alike)
