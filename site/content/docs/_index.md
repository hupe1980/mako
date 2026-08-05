+++
title = "Documentation"
description = "Complete documentation for mako — the open-source German energy-market platform in Rust. Guides, service operator manuals, EDIFACT reference, regulatory mapping and the annual BDEW release workflow."
sort_by = "weight"
template = "section.html"
[extra]
mermaid = true
+++

Everything needed to build, run and extend mako — grouped by what you are trying
to do.

## Start here

| If you want to… | Go to |
|---|---|
| See it work end to end in a few minutes | [Getting started](@/docs/guide/getting-started.md) |
| Understand how a message becomes a process | [Architecture](architecture/) |
| Run a service in production | [Services](services/) |
| Parse, validate or build EDIFACT yourself | [Reference](reference/) |
| Trace a rule back to its Festlegung or § | [Regulatory](regulatory/) |
| Take on a new annual BDEW release | [Release & compliance](compliance/) |

## How the layers fit together

```mermaid
graph TB
    subgraph transport ["Transport"]
        AS4["AS4 / ebMS3<br/>BDEW AS4-Profil"]
        API["API-Webdienste<br/>REST"]
    end
    subgraph format ["Format — generated from BDEW profiles"]
        EDI["edi-energy<br/>parse · validate · build"]
    end
    subgraph domain ["Domain — typed commands and events"]
        ENG["mako-engine<br/>workflows · Fristen · outbox"]
        MODS["mako-gpke · mako-wim<br/>mako-geli-gas · mako-mabis · …"]
    end
    subgraph services ["Services"]
        SVC["makod · marktd · billingd<br/>accountingd · edmd · …"]
    end

    AS4 --> EDI
    API --> ENG
    EDI -->|"typed message"| ENG
    ENG --- MODS
    ENG --> SVC
    SVC -->|"CloudEvents"| SVC
```

The split is deliberate. The domain layer never sees EDIFACT, and the format
layer never knows what a Lieferantenwechsel is — so an annual BDEW release is a
codegen run against new profiles rather than a change to process logic.

## Sections

- **[Guide](guide/)** — install, run the demo stack, submit your first message.
- **[Architecture](architecture/)** — the domain model, the workflow engine, ERP
  integration and the API-Webdienste transition.
- **[Services](services/)** — one operator manual per service: endpoints,
  configuration, emitted events, deployment.
- **[Reference](reference/)** — EDIFACT parsing, validation, builders, the AS4
  profile, process catalogue, and the `makotest` Python toolkit.
- **[Regulatory](regulatory/)** — BNetzA Festlegungen and the full
  Prüfidentifikator table with the crate and workflow that owns each one.
- **[Release & compliance](compliance/)** — the annual release workflow, schema
  versioning, and licence inventory.
