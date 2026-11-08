+++
title = "CRA and AI Act"
description = "mako under the Cyber Resilience Act (EU) 2024/2847 and the AI Act (EU) 2024/1689: outside CRA scope by the owner's determination, with an advisory agent plane."
weight = 40
+++
Two European regulations reach software like mako on fixed dates. This page
records where mako stands under each, the articles that position rests on, and
what would change it. The articles are quoted from the Official Journal text on
EUR-Lex, read on 08.11.2026.

---

## Cyber Resilience Act

Regulation (EU) 2024/2847, OJ L, 2024/2847, 20.11.2024
(<https://eur-lex.europa.eu/eli/reg/2024/2847/oj/eng>).

### The determination

**mako is outside the scope of Regulation (EU) 2024/2847.** It is free and open
source software, and it is not supplied in the course of a commercial activity:
no price is charged for it, and no paid support, hosting or licence is offered
for it. This is the owner's determination of **08.11.2026**.

Consequently mako has no manufacturer under the Regulation, and no Art. 14
notification is owed for it. How a vulnerability is still reported and handled
is the [security policy](https://github.com/hupe1980/mako/blob/main/SECURITY.md).

### Its basis

- **Art. 2(1)** — the Regulation „applies to products with digital elements
  made available on the market".
- **Art. 3(22)** — „‘making available on the market’ means the supply of a
  product with digital elements for distribution or use on the Union market in
  the course of a commercial activity, whether in return for payment or free of
  charge". Free of charge is not enough to leave scope; the absence of a
  commercial activity is.
- **Recital 18** — „the provision of products with digital elements qualifying
  as free and open-source software that are not monetised by their
  manufacturers should not be considered to be a commercial activity", and „the
  mere presence of regular releases should not in itself lead to the conclusion
  that a product with digital elements is supplied in the course of a
  commercial activity". mako's semi-annual release rhythm therefore does not
  decide the question.
- **Recital 15** — names what does characterise a commercial supply: „not only
  … charging a price for a product with digital elements, but also … charging a
  price for technical support services where this does not serve only the
  recuperation of actual costs, by an intention to monetise, for instance by
  providing a software platform through which the manufacturer monetises other
  services, by requiring as a condition for use the processing of personal data
  for reasons other than exclusively for improving the security, compatibility
  or interoperability of the software, or by accepting donations exceeding the
  costs associated with the design, development and provision of a product with
  digital elements". None of these is done for mako.
- **Recital 20** — „The sole act of hosting products with digital elements on
  open repositories, including through package managers or on collaboration
  platforms, does not in itself constitute the making available on the market
  of a product with digital elements." Publishing the crates on crates.io, the
  images on GHCR and the source on GitHub is that act.

### What reopens it

Any of the recital 15 characteristics makes the supply commercial, and the
owner then a **manufacturer** — Art. 3(13): „a natural or legal person who
develops or manufactures products with digital elements … and markets them
under its name or trademark, whether for payment, monetisation or free of
charge". In practice that is:

- paid support for mako beyond the recovery of actual costs;
- hosting mako, or a platform built on it, for payment;
- selling a licence for it, or an edition of it;
- accepting donations beyond its costs.

The determination is then reopened before the first such offer, not after it,
because a manufacturer's Art. 14 duties take effect at once: under Art. 71(2)
„Article 14 shall apply from 11 September 2026", and under Art. 69(3) „the
obligations laid down in Article 14 shall apply to all products with digital
elements that fall within the scope of this Regulation that have been placed on
the market before 11 December 2027".

### The open-source steward route

Art. 3(14) defines an open-source software steward as „a legal person, other
than a manufacturer, that has the purpose or objective of systematically
providing support on a sustained basis for the development of specific
products with digital elements, qualifying as free and open-source software and
intended for commercial activities". The owner is a natural person, so the
route needs a legal person — a foundation or a company — to take mako on.

A steward's reporting duty is narrower than a manufacturer's — Art. 24(3): „The
obligations laid down in Article 14(1) shall apply to open-source software
stewards to the extent that they are involved in the development of the
products with digital elements." Art. 24 is not among the provisions Art. 71(2) brings
forward, so it applies from the general date, 11 December 2027.

### What it does not decide

The determination is about mako as the owner supplies it. A party that builds
mako into a product of its own and markets that product under its name or
trademark is that product's manufacturer under Art. 3(13), and the Regulation's
obligations for that product are its own. The release evidence it may need for
that — SBOMs, provenance, the embedded dependency manifest, VEX — is described
on [Release Lifecycle](@/docs/compliance/release-lifecycle.md#what-a-release-carries).

---

## AI Act

Regulation (EU) 2024/1689, OJ L, 2024/1689, 12.7.2024
(<https://eur-lex.europa.eu/eli/reg/2024/1689/oj/eng>), as amended by
Regulation (EU) 2026/1744 („Digital Omnibus on AI"), OJ L, 2026/1744,
24.7.2026 (<https://eur-lex.europa.eu/eli/reg/2026/1744/oj/eng>).

### Transparency

Every worklist row a model writes opens with `AI-generated. ` (`agentd`'s
`plane::MACHINE_GENERATED`), and `agentd`'s own suite holds the set of
model-written fields complete. Art. 50 applies from the Regulation's general
date, 2 August 2026 (Art. 113); Art. 111(4), added by Art. 1 point (39) of
Regulation (EU) 2026/1744, gives providers of systems generating synthetic content
„that have been placed on the market before 2 August 2026" until
„2 December 2026" to „comply with Article 50(2)".

### The agent plane is advisory

No specialist manifest holds a grant to a mutating tool — a tool whose MCP
server does not declare `annotations(read_only_hint = true)` — so the agent
plane reads, drafts and proposes, and a human acts. `cargo xtask
check-tool-grants` refuses any such grant while this posture holds, whatever
the manifest itself declares about the grant, and names this section.

The posture is what the Annex III classification rests on. Annex III point 2
lists „AI systems intended to be used as safety components in the management
and operation of critical digital infrastructure, road traffic, or in the
supply of water, gas, heating or electricity", and Art. 6(1a) and (1b), inserted
by Art. 1 point (8) of Regulation (EU) 2026/1744, read:

> „1a. For the purposes of this Regulation, including paragraph 1 of this
> Article, AI systems that are solely used for non-safety related aspects of
> user assistance, performance optimisation, service efficiency, automation or
> convenience or quality control shall not qualify as safety components.
> 1b. Notwithstanding paragraph 1a, AI systems the failure or malfunctioning of
> which would endanger health and safety shall qualify as safety components."

mako records **no project-side Annex III classification**. Chapter III
Sections 1 to 3 apply to Annex III systems from „2 December 2027" (Art. 113
third paragraph point (c)(i) as replaced by Art. 1 point (40) of Regulation
(EU) 2026/1744). Without a classification the posture stays advisory:
`check-tool-grants` admits a mutating grant only when the recorded posture
changes together with the classification.
