---
layout: default
title: PID Reference
nav_order: 41
parent: Regulatory
description: >
  Complete Prüfidentifikator (PID) reference for all German energy market
  processes. Covers BDEW PID 3.3 (FV2025-10-01) and PID 4.0 (FV2026-10-01).
---

# Prüfidentifikator (PID) Reference

**Source document:** BDEW EDI@Energy — *Anwendungsübersicht der Prüfidentifikatoren*  
**Versions:** PID 3.3 (FV2025-10-01, Fehlerkorrektur 27.03.2026) · PID 4.0 (FV2026-10-01, published 01.04.2026)

A Prüfidentifikator (PID) identifies a specific EDIFACT message use case within a
business process. Each PID is bound to one EDIFACT format (UTILMD, MSCONS, INVOIC, …)
and one business context (GPKE, WiM, GeLi Gas, …). The routing layer
(`mako_engine::pid_router::PidRouter`) dispatches inbound messages to the correct
workflow by PID.

> **Legend** — *PID 3.3 / PID 4.0 columns*  
> ✅ = present in the respective BDEW PID overview document  
> ⚠️ = **not** present in the respective PID overview document

---

## Table of contents

1. [UTILMD AHB Strom (189 PIDs)](#utilmd-ahb-strom)
2. [UTILMD AHB Gas (89 PIDs)](#utilmd-ahb-gas)
3. [ORDERS AHB (46 PIDs)](#orders-ahb)
4. [ORDRSP AHB (40 PIDs)](#ordrsp-ahb)
5. [ORDCHG AHB (3 PIDs)](#ordchg-ahb)
6. [IFTSTA AHB (35 PIDs)](#iftsta-ahb)
7. [MSCONS AHB (25 PIDs)](#mscons-ahb)
8. [INVOIC AHB (11 PIDs)](#invoic-ahb)
9. [REMADV AHB (4 PIDs)](#remadv-ahb)
10. [PARTIN AHB (14 PIDs)](#partin-ahb)
11. [REQOTE AHB (5 PIDs)](#reqote-ahb)
12. [QUOTES AHB (5 PIDs)](#quotes-ahb)
13. [PRICAT AHB (3 PIDs)](#pricat-ahb)
14. [INSRPT AHB (8 PIDs)](#insrpt-ahb)
15. [UTILTS AHB (8 PIDs)](#utilts-ahb)
16. [COMDIS AHB (2 PIDs)](#comdis-ahb)

---

## UTILMD AHB Strom

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 55001 | Anmeldung verb. MaLo                              | GPKE Teil 2                                  | ✅   | ✅   |
| 55002 | Bestätigung Anmeldung verb. MaLo                  | GPKE Teil 2                                  | ✅   | ✅   |
| 55003 | Ablehnung Anmeldung verb. MaLo                    | GPKE Teil 2                                  | ✅   | ✅   |
| 55004 | Abmeldung                                         | GPKE Teil 2                                  | ✅   | ✅   |
| 55005 | Bestätigung Abmeldung                             | GPKE Teil 2                                  | ✅   | ✅   |
| 55006 | Ablehnung Abmeldung                               | GPKE Teil 2                                  | ✅   | ✅   |
| 55007 | Abmeldung / Beendigung der Zuordnung              | GPKE Teil 2                                  | ✅   | ✅   |
| 55008 | Bestätigung Abmeldung                             | GPKE Teil 2                                  | ✅   | ✅   |
| 55009 | Ablehnung Abmeldung                               | GPKE Teil 2                                  | ✅   | ✅   |
| 55010 | Anfrage zur Beendigung der Zuordnung              | GPKE Teil 2                                  | ✅   | ✅   |
| 55011 | Bestätigung Beendigung der Zuordnung              | GPKE Teil 2                                  | ✅   | ✅   |
| 55012 | Ablehnung Beendigung der Zuordnung                | GPKE Teil 2                                  | ✅   | ✅   |
| 55013 | Anmeldung / Zuordnung EOG                         | GPKE Teil 2                                  | ✅   | ✅   |
| 55014 | Bestätigung EOG Anmeldung                         | GPKE Teil 2                                  | ✅   | ✅   |
| 55015 | Ablehung EOG Anmeldung                            | GPKE Teil 2                                  | ✅   | ✅   |
| 55016 | Kündigung                                         | GPKE Teil 2                                  | ✅   | ✅   |
| 55017 | Bestätigung Kündigung                             | GPKE Teil 2                                  | ✅   | ✅   |
| 55018 | Ablehnung Kündigung                               | GPKE Teil 2                                  | ✅   | ✅   |
| 55022 | Anfrage nach Stornierung                          | GPKE Teil 4                                  | ✅   | ✅   |
| 55023 | Bestätigung Anfrage Stornierung                   | GPKE Teil 4                                  | ✅   | ✅   |
| 55024 | Ablehnung Anfrage Stornierung                     | GPKE Teil 4                                  | ✅   | ✅   |
| 55035 | Antwort auf GDA verb. MaLo                        | GPKE Teil 4                                  | ✅   | ✅   |
| 55036 | Existierende Zuordnung                            | GPKE Teil 2                                  | ✅   | ✅   |
| 55037 | Beendigung der Zuordnung                          | GPKE Teil 2                                  | ✅   | ✅   |
| 55038 | Aufhebung einer zuk. Zuordnung                    | GPKE Teil 2                                  | ✅   | ✅   |
| 55039 | Kündigung MSB                                     | WiM Strom Teil 1                             | ✅   | ✅   |
| 55040 | Bestätigung Kündigung MSB                         | WiM Strom Teil 1                             | ✅   | ✅   |
| 55041 | Ablehnung Kündigung MSB                           | WiM Strom Teil 1                             | ✅   | ✅   |
| 55042 | Anmeldung MSB                                     | WiM Strom Teil 1                             | ✅   | ✅   |
| 55043 | Bestätigung Anmeldung MSB                         | WiM Strom Teil 1                             | ✅   | ✅   |
| 55044 | Ablehnung Anmeldung MSB                           | WiM Strom Teil 1                             | ✅   | ✅   |
| 55051 | Ende MSB                                          | WiM Strom Teil 1                             | ✅   | ✅   |
| 55052 | Bestätigung Ende MSB                              | WiM Strom Teil 1                             | ✅   | ✅   |
| 55053 | Ablehnung Ende MSB                                | WiM Strom Teil 1                             | ✅   | ✅   |
| 55060 | Antwort auf GDA                                   | GPKE Teil 4                                  | ✅   | ✅   |
| 55062 | Aktivierung von ZP                                | MaBiS                                        | ✅   | ✅   |
| 55063 | Deaktivierung von ZP                              | MaBiS                                        | ✅   | ✅   |
| 55064 | Antwort                                           | MaBiS                                        | ✅   | ✅   |
| 55065 | Lieferantenclearingliste                          | MaBiS                                        | ✅   | ✅   |
| 55066 | Korrekturliste zu Lieferantenclearingliste        | MaBiS                                        | ✅   | ✅   |
| 55067 | Bilanzkreiszuordnungsliste                        | MaBiS                                        | ✅   | ✅   |
| 55069 | Clearingliste DZR                                 | MaBiS                                        | ✅   | ✅   |
| 55070 | Clearingliste BAS                                 | MaBiS                                        | ✅   | ✅   |
| 55071 | Aktivierung der Zuordnungsermächtigung            | MaBiS                                        | ✅   | ✅   |
| 55072 | Deaktivierung der Zuordnungsermächtigung          | MaBiS                                        | ✅   | ✅   |
| 55073 | Übermittlung der Profildefinitionen               | MaBiS                                        | ✅   | ✅   |
| 55074 | Stammdaten auf eine ORDERS                        | Prozesse zum Informationsaustausch zwischen Netzbetreiber und Herkunftsnachweis-register (HKN-R) des Umweltbundesamts (UBA)| ✅   | ✅   |
| 55075 | Stammdaten aufgrund einer Änderung                | Prozesse zum Informationsaustausch zwischen Netzbetreiber und Herkunftsnachweis-register (HKN-R) des Umweltbundesamts (UBA)| ✅   | ✅   |
| 55076 | Antwort auf Stammdatenänderung                    | Prozesse zum Informationsaustausch zwischen Netzbetreiber und Herkunftsnachweis-register (HKN-R) des Umweltbundesamts (UBA)| ✅   | ✅   |
| 55077 | Anmeldung erz. MaLo                               | GPKE Teil 2                                  | ✅   | ✅   |
| 55078 | Bestätigung Anmeldung erz. MaLo                   | GPKE Teil 2                                  | ✅   | ✅   |
| 55080 | Ablehnung Anmeldung erz. MaLo                     | GPKE Teil 2                                  | ✅   | ✅   |
| 55095 | Antwort auf GDA erz. MaLo                         | GPKE Teil 4                                  | ✅   | ✅   |
| 55109 | Änderung Daten der MaLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55110 | Änderung Daten der MaLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55126 | Abr.-Daten BK-Abr. verb. MaLo                     | GPKE Teil 2                                  | ✅   | ✅   |
| 55136 | Rückmeldung/Anfrage Daten der MaLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55137 | Rückmeldung/Anfrage Daten der MaLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55156 | Rückmeldung/Anfrage Abr.-Daten BK-Abr. verb. MaLo | GPKE Teil 2                                  | ✅   | ✅   |
| 55168 | Verpflichtungsanfrage / Aufforderung              | WiM Strom Teil 1                             | ✅   | ✅   |
| 55169 | Bestätigung Verpflichtungsanfrage                 | WiM Strom Teil 1                             | ✅   | ✅   |
| 55170 | Ablehnung Verpflichtungsanfrage                   | WiM Strom Teil 1                             | ✅   | ✅   |
| 55173 | Änderung der Lokationsbündelstruktur              | GPKE Teil 4                                  | ✅   | ✅   |
| 55175 | Änderung der Lokationsbündelstruktur              | GPKE Teil 4                                  | ✅   | ✅   |
| 55177 | Rückmeldung/Anfrage Lokationsbündelstruktur       | GPKE Teil 4                                  | ✅   | ✅   |
| 55180 | Rückmeldung/Anfrage Lokationsbündelstruktur       | GPKE Teil 4                                  | ✅   | ✅   |
| 55194 | Antowrt auf GDA (Strom an Gas)                    | GPKE Teil 4                                  | ✅   | ✅   |
| 55195 | Bilanzierungsgebietsclearingliste                 | MaBiS                                        | ✅   | ✅   |
| 55196 | Antwort auf Bilanzierungsgebietsclearingliste     | MaBiS                                        | ✅   | ✅   |
| 55197 | Aktivierung ZP tägliche AAÜZ                      | MaBiS                                        | ✅   | ✅   |
| 55198 | Deaktivierung tägliche AAÜZ                       | MaBiS                                        | ✅   | ✅   |
| 55199 | Aktivierung ZP LF-AASZR                           | MaBiS                                        | ✅   | ✅   |
| 55200 | Deaktivierung ZP LF-AASZR                         | MaBiS                                        | ✅   | ✅   |
| 55201 | LF-AACL                                           | MaBiS                                        | ✅   | ✅   |
| 55202 | Korrekturliste LF-AACL                            | MaBiS                                        | ✅   | ✅   |
| 55203 | Aktivierung ZP monatliche AAÜZ                    | MaBiS                                        | ✅   | ✅   |
| 55204 | Antwort auf Aktivierung ZP                        | MaBiS                                        | ✅   | ✅   |
| 55205 | Weiterleitung Aktivierung ZP                      | MaBiS                                        | ✅   | ✅   |
| 55206 | Deaktivierung ZP monatliche AAÜZ                  | MaBiS                                        | ✅   | ✅   |
| 55207 | Antwort auf Deaktivierung ZP                      | MaBiS                                        | ✅   | ✅   |
| 55208 | Weiterleitung Deaktivierung ZP                    | MaBiS                                        | ✅   | ✅   |
| 55209 | Aktivierung ZP monatliche AAÜZ                    | MaBiS                                        | ✅   | ✅   |
| 55210 | Antwort auf Aktiveirung ZP                        | MaBiS                                        | ✅   | ✅   |
| 55211 | Weiterleitung Aktivierung ZP                      | MaBiS                                        | ✅   | ✅   |
| 55212 | Deaktivierung ZP monatliche AAÜZ                  | MaBiS                                        | ✅   | ✅   |
| 55213 | Antwort auf Deaktivierung ZP                      | MaBiS                                        | ✅   | ✅   |
| 55214 | Weiterleitung Deaktivierung ZP                    | MaBiS                                        | ✅   | ✅   |
| 55218 | Abr.-Daten NNA                                    | GPKE Teil 2                                  | ✅   | ✅   |
| 55220 | Rückmeldung/Anfrage Abr.-Daten NNA                | GPKE Teil 2                                  | ✅   | ✅   |
| 55223 | DZÜ-Liste                                         | MaBiS                                        | ✅   | ✅   |
| 55224 | Antwort auf DZÜ-Liste                             | MaBiS                                        | ✅   | ✅   |
| 55225 | Änderung Blindabr.-Daten der NeLo                 | GPKE Teil 4                                  | ✅   | ✅   |
| 55227 | Rückmeldung/Anfrage Blindabr.-Daten der NeLo      | GPKE Teil 4                                  | ✅   | ✅   |
| 55230 | Änderung Blindabr.-Daten der NeLo                 | GPKE Teil 4                                  | ✅   | ✅   |
| 55232 | Rückmeldung/Anfrage Blindabr.-Daten der NeLo      | GPKE Teil 4                                  | ✅   | ✅   |
| 55235 | Zuordnung ZP der NGZ zur NZR                      | AWH Erg. d. Marktregeln f. d. Durchfür. d. Bilanzkreisabr. Strom (MaBiS)| ✅   | ✅   |
| 55236 | Beendigung Zuordnung ZP der NGZ zur NZR           | AWH Erg. d. Marktregeln f. d. Durchfür. d. Bilanzkreisabr. Strom (MaBiS)| ✅   | ✅   |
| 55237 | Antwort                                           | AWH Erg. d. Marktregeln f. d. Durchfür. d. Bilanzkreisabr. Strom (MaBiS)| ✅   | ✅   |
| 55238 | Anmeldung in Modell 2                             | AWH Modell 2 ladev.scharf.
bila.
Energie.zuord.möglichkeit| ✅   | ✅   |
| 55239 | Antwort auf Anmeldung                             | AWH Modell 2 ladev.scharf.
bila.
Energie.zuord.möglichkeit| ✅   | ✅   |
| 55240 | Beendigung der Zuordnung zur Marktlokation        | AWH Modell 2 ladev.scharf.
bila.
Energie.zuord.möglichkeit| ✅   | ✅   |
| 55241 | Antwort auf Beendigung                            | AWH Modell 2 ladev.scharf.
bila.
Energie.zuord.möglichkeit| ✅   | ✅   |
| 55242 | Abmeldung aus dem Modell 2                        | AWH Modell 2 ladev.scharf.
bila.
Energie.zuord.möglichkeit| ✅   | ✅   |
| 55243 | Antwort auf Abmeldung                             | AWH Modell 2 ladev.scharf.
bila.
Energie.zuord.möglichkeit| ✅   | ✅   |
| 55553 | Daten auf individuelle Bestellung                 | GPKE Teil 4                                  | ✅   | ✅   |
| 55555 | Anfrage Daten der individuellen Bestellung        | GPKE Teil 4                                  | ✅   | ✅   |
| 55557 | Änderung MSB-Abr.-Daten der MaLo                  | GPKE Teil 4                                  | ✅   | ✅   |
| 55559 | Rückmeldung/Anfrage MSB-Abr.-Daten der MaLo       | GPKE Teil 4                                  | ✅   | ✅   |
| 55600 | Anmeldung neuer verb. MaLo                        | GPKE Teil 2                                  | ✅   | ✅   |
| 55601 | Anmeldung neuer erz. MaLo                         | GPKE Teil 2                                  | ✅   | ✅   |
| 55602 | Bestätigung Anmeldung neuer verb. MaLo            | GPKE Teil 2                                  | ✅   | ✅   |
| 55603 | Bestätigung Anmeldung neuer erz. MaLo             | GPKE Teil 2                                  | ✅   | ✅   |
| 55604 | Ablehnung Anmeldung neuer verb. MaLo              | GPKE Teil 2                                  | ✅   | ✅   |
| 55605 | Ablehnung Anmeldung neuer erz. MaLo               | GPKE Teil 2                                  | ✅   | ✅   |
| 55607 | Ankündigung Zuordnung / Zuordnung des LF zur MaLo/ Tranche| GPKE Teil 2                                  | ✅   | ✅   |
| 55608 | Bestätigung Zuordnung des LF zur MaLo/ Tranche    | GPKE Teil 2                                  | ✅   | ✅   |
| 55609 | Ablehnung Zuordnung des LF zur MaLo/ Tranche      | GPKE Teil 2                                  | ✅   | ✅   |
| 55611 | Beendigung der Zuordnung                          | GPKE Teil 2                                  | ✅   | ✅   |
| 55613 | Abr.-Daten BK-Abr. verb. MaLo                     | GPKE Teil 2                                  | ✅   | ✅   |
| 55614 | Rückmeldung/Anfrage Abr.-Daten BK-Abr. verb. MaLo | GPKE Teil 2                                  | ✅   | ✅   |
| 55615 | Änderung Daten der NeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55616 | Änderung Daten der MaLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55617 | Änderung Daten der TR                             | GPKE Teil 4                                  | ✅   | ✅   |
| 55618 | Änderung Daten der SR                             | GPKE Teil 4                                  | ✅   | ✅   |
| 55619 | Änderung Daten der Tranche                        | GPKE Teil 4                                  | ✅   | ✅   |
| 55620 | Änderung Daten der MeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55621 | Rückmeldung/Anfrage Daten zur NeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55622 | Rückmeldung/Anfrage Daten der MaLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55623 | Rückmeldung/Anfrage Daten der TR                  | GPKE Teil 4                                  | ✅   | ✅   |
| 55624 | Rückmeldung/Anfrage Daten der SR                  | GPKE Teil 4                                  | ✅   | ✅   |
| 55625 | Rückmeldung/Anfrage Daten der Tranche             | GPKE Teil 4                                  | ✅   | ✅   |
| 55626 | Rückmeldung/Anfrage Daten der MeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55627 | Änderung Daten der NeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55628 | Änderung Daten der MaLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55629 | Änderung Daten der TR                             | GPKE Teil 4                                  | ✅   | ✅   |
| 55630 | Änderung Daten der SR                             | GPKE Teil 4                                  | ✅   | ✅   |
| 55632 | Änderung Daten der MeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55633 | Rückmeldung/Anfrage Daten zur NeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55634 | Rückmeldung/Anfrage Daten der MaLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55635 | Rückmeldung/Anfrage Daten der TR                  | GPKE Teil 4                                  | ✅   | ✅   |
| 55636 | Rückmeldung/Anfrage Daten der SR                  | GPKE Teil 4                                  | ✅   | ✅   |
| 55638 | Rückmeldung/Anfrage Daten der MeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55639 | Änderung Daten der NeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55640 | Änderung Daten der MaLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55641 | Änderung Daten der SR                             | GPKE Teil 4                                  | ✅   | ✅   |
| 55642 | Änderung Daten der Tranche                        | GPKE Teil 4                                  | ✅   | ✅   |
| 55643 | Änderung Daten der MeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55644 | Rückmeldung/Anfrage Daten der NeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55645 | Rückmeldung/Anfrage Daten der MaLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55646 | Rückmeldung/Anfrage Daten der SR                  | GPKE Teil 4                                  | ✅   | ✅   |
| 55647 | Rückmeldung/Anfrage Daten der Tranche             | GPKE Teil 4                                  | ✅   | ✅   |
| 55648 | Rückmeldung/Anfrage Daten der MeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55649 | Änderung Daten der NeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55650 | Änderung Daten der MaLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55651 | Änderung Daten der SR                             | GPKE Teil 4                                  | ✅   | ✅   |
| 55652 | Änderung Daten der Tranche                        | GPKE Teil 4                                  | ✅   | ✅   |
| 55653 | Änderung Daten der MeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55654 | Rückmeldung/Anfrage Daten der NeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55655 | Rückmeldung/Anfrage Daten der MaLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55656 | Rückmeldung/Anfrage Daten der SR                  | GPKE Teil 4                                  | ✅   | ✅   |
| 55657 | Rückmeldung/Anfrage Daten der Tranche             | GPKE Teil 4                                  | ✅   | ✅   |
| 55658 | Rückmeldung/Anfrage Daten der MeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55659 | Änderung Daten der NeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55660 | Änderung Daten der MaLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55661 | Änderung Daten der SR                             | GPKE Teil 4                                  | ✅   | ✅   |
| 55662 | Änderung Daten der Tranche                        | GPKE Teil 4                                  | ✅   | ✅   |
| 55663 | Änderung Daten der MeLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55664 | Rückmeldung/Anfrage Daten der NeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55665 | Rückmeldung/Anfrage Daten der MaLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55666 | Rückmeldung/Anfrage Daten der SR                  | GPKE Teil 4                                  | ✅   | ✅   |
| 55667 | Rückmeldung/Anfrage Daten der Tranche             | GPKE Teil 4                                  | ✅   | ✅   |
| 55669 | Rückmeldung/Anfrage Daten der MeLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55670 | Stammdaten BK-Treue                               | GPKE Teil 4                                  | ✅   | ✅   |
| 55671 | Rückmeldung auf Stammdaten BK-Treue               | GPKE Teil 4                                  | ✅   | ✅   |
| 55672 | Abr.-Daten BK-Abr. erz. Malo                      | GPKE Teil 2                                  | ✅   | ✅   |
| 55673 | Rückmeldung/Anfrage Abr.-Daten BK-Abr. erz. Malo  | GPKE Teil 2                                  | ✅   | ✅   |
| 55674 | Abr.-Daten BK-Abr. erz. Malo                      | GPKE Teil 2                                  | ✅   | ✅   |
| 55675 | Rückmeldung/Anfrage Abr.-Daten BK-Abr. erz. Malo  | GPKE Teil 2                                  | ✅   | ✅   |
| 55684 | Änderung Daten der MaLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55685 | Rückmeldung/Anfrage Daten der MaLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55686 | Änderung Daten der Tranche                        | GPKE Teil 4                                  | ✅   | ✅   |
| 55687 | Rückmeldung/Anfrage Daten der Tranche             | GPKE Teil 4                                  | ✅   | ✅   |
| 55688 | Änderung Daten der MaLo                           | GPKE Teil 4                                  | ✅   | ✅   |
| 55689 | Rückmeldung/Anfrage Daten der MaLo                | GPKE Teil 4                                  | ✅   | ✅   |
| 55690 | Lokationsbündelstruktur und DB                    | AWH Netzbetreiberwechsel                     | ✅   | ✅   |
| 55691 | Änderung Paket-ID der MaLo                        | GPKE Teil 4                                  | ✅   | ✅   |
| 55692 | Rückmeldung/Anfrage Paket-ID der MaLo             | GPKE Teil 4                                  | ✅   | ✅   |
| 55693 | Änderung Daten der TR                             | GPKE Teil 4                                  | ⚠️  | ✅   |
| 55694 | Rückmeldung/ Anfrage Daten der TR                 | GPKE Teil 4                                  | ⚠️  | ✅   |

## UTILMD AHB Gas

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 44001 | Anmeldung NN                                      | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44002 | Bestätigung Anmeldung                             | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44003 | Ablehnung Anmeldung                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44004 | Abmeldung NN                                      | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44005 | Bestätigung Abmeldung                             | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44006 | Ablehnung Abmeldung                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44007 | Abmeldung NN vom NB                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44008 | Bestätigung Abmeldung vom NB                      | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44009 | Ablehnung Abmeldung vom NB                        | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44010 | Abmeldungsanfrage des NB                          | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44011 | Bestätigung Abmeldungsanfrage                     | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44012 | Ablehnung Abmeldungsanfrage                       | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44013 | Anmeldung EoG                                     | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44014 | Bestätigung EoG Anmeldung                         | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44015 | Ablehnung EoG Anmeldung                           | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44016 | Kündigung beim alten Lieferanten                  | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44017 | Bestätigung Kündigung                             | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44018 | Ablehnung Kündigung                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44019 | Bestandsliste zugeordnete Marktlokationen         | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44020 | Änderungsmeldung zur Bestandsliste                | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44021 | Antwort auf Änderungsmeldung zur Bestandsliste    | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44022 | Anfrage nach Stornierung                          | WiM Gas                                      | ✅   | ✅   |
| 44023 | Bestätigung Anfrage Stornierung                   | WiM Gas                                      | ✅   | ✅   |
| 44024 | Ablehnung Anfrage Stornierung                     | WiM Gas                                      | ✅   | ✅   |
| 44035 | Antwort auf die Geschäftsdatenanfrage             | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44036 | Informationsmeldung über existierende Zuordnung   | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44037 | Informationsmeldung zur Beendigung der Zuordnung  | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44038 | Informationsmeldung zur Aufhebung einer zuk. Zuordnung| GeLi Gas 2.0                                 | ✅   | ✅   |
| 44039 | Kündigung MSB                                     | WiM Gas                                      | ✅   | ✅   |
| 44040 | Bestätigung Kündigung MSB                         | WiM Gas                                      | ✅   | ✅   |
| 44041 | Ablehnung Kündigung MSB                           | WiM Gas                                      | ✅   | ✅   |
| 44042 | Anmeldung MSB                                     | WiM Gas                                      | ✅   | ✅   |
| 44043 | Bestätigung Anmeldung MSB                         | WiM Gas                                      | ✅   | ✅   |
| 44044 | Ablehnung Anmeldung MSB                           | WiM Gas                                      | ✅   | ✅   |
| 44051 | Ende MSB                                          | WiM Gas                                      | ✅   | ✅   |
| 44052 | Bestätigung Ende MSB                              | WiM Gas                                      | ✅   | ✅   |
| 44053 | Ablehnung Ende MSB                                | WiM Gas                                      | ✅   | ✅   |
| 44060 | Antwort auf die Geschäftsdatenanfrage             | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44101 | Stammdaten zur Messlokation                       | Leitfaden Marktprozesse Netzbetreiberwechsel | ✅   | ✅   |
| 44102 | Aktualisierte Stammdaten zur Messlokation         | Leitfaden Marktprozesse Netzbetreiberwechsel | ✅   | ✅   |
| 44103 | Stammdaten zur verbrauchenden Marktlokation       | Leitfaden Marktprozesse Netzbetreiberwechsel | ✅   | ✅   |
| 44104 | Aktualisierte Stammdaten zur verbrauchenden Marktlokation| Leitfaden Marktprozesse Netzbetreiberwechsel | ✅   | ✅   |
| 44105 | Ablehnung auf Stammdaten zur verbrauchenden Marktlokation| Leitfaden Marktprozesse Netzbetreiberwechsel | ✅   | ✅   |
| 44109 | Nicht bila.rel Änderung vom LF                    | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44111 | Antwort auf Änderung vom LF                       | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44112 | Nicht bila.rel. Änderung vom NB                   | Marktraumumstellung                          | ✅   | ✅   |
| 44113 | Nicht bila.rel. Änderung vom NB                   | Marktraumumstellung                          | ✅   | ✅   |
| 44115 | Antwort auf Änderung vom NB                       | Marktraumumstellung                          | ✅   | ✅   |
| 44116 | Änderung vom MSB mit Abhängigkeiten               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44117 | Änderung vom MSB mit Abhängigkeiten               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44119 | Antwort auf Änderung vom MSB                      | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44120 | Bila.rel. Änderung vom LF                         | Marktraumumstellung                          | ✅   | ✅   |
| 44121 | Antwort auf Änderung vom LF                       | Marktraumumstellung                          | ✅   | ✅   |
| 44123 | Bila.rel. Änderung vom NB mit Abhängigkeiten      | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44124 | Antwort auf Änderung vom NB                       | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44137 | Nicht bila. rel. Anfrage an LF                    | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44138 | Antwort auf Anfrage                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44139 | Nicht bila.rel. Anfrage an NB                     | Marktraumumstellung                          | ✅   | ✅   |
| 44140 | Nicht bila.rel. Anfrage an NB                     | Marktraumumstellung                          | ✅   | ✅   |
| 44142 | Antwort auf Anfrage                               | Marktraumumstellung                          | ✅   | ✅   |
| 44143 | Anfrage an MSB mit Abhängigkeiten                 | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44145 | Antwort auf Anfrage                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44146 | Ablehnung der Anfrage                             | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44147 | Anfrage an MSB mit Abhängigkeiten                 | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44148 | Anfrage an MSB mit Abhängigkeiten                 | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44149 | Antwort auf Anfrage                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44150 | Bila. rel. Anfrage an LF                          | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44151 | Antwort auf Anfrage                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44152 | Ablehnung der Anfrage                             | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44156 | Bila.rel. Anfrage an NB mit Abhängigkeiten        | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44157 | Antwort auf Anfrage                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44159 | Änderung vom MSB ohne Abhängigkeiten              | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44160 | Änderung vom MSB ohne Abhängigkeiten              | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44161 | Antwort auf Änderung                              | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44162 | Anfrage an MSB ohne Abhängigkeiten                | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44163 | Antwort auf Anfrage                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44164 | Ablehnung Anfrage                                 | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44165 | Nicht bila. rel Anfrage an MSB ohne Abhängigkeiten| GeLi Gas 2.0                                 | ✅   | ✅   |
| 44166 | Nicht bila. rel Anfrage an MSB ohne Abhängigkeiten| GeLi Gas 2.0                                 | ✅   | ✅   |
| 44167 | Antwort auf Anfrage                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44168 | Verpflichtungsanfrage / Aufforderung              | WiM Gas                                      | ✅   | ✅   |
| 44169 | Bestätigung Verpflichtungsanfrage                 | WiM Gas                                      | ✅   | ✅   |
| 44170 | Ablehnung Verpflichtungsanfrage                   | WiM Gas                                      | ✅   | ⚠️  |
| 44175 | Änderung der Marktlokationsstruktur               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44176 | Antwort auf Änderung der 
Marktlokationsstruktur  | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44180 | Anfrage der Marktlokationsstruktur                | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44181 | Antwort auf Anfrage der 
Marktlokationsstruktur   | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44182 | Ablehnung der Anfrage der 
Marktlokationsstruktur | GeLi Gas 2.0                                 | ✅   | ✅   |
| 44183 | Ende MSB von NB                                   | AWH WiM Gas 2.0                              | ⚠️  | ✅   |

## ORDERS AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 17001 | Bestellung Geräteübernahmeangebot                 | WiM Gas                                      | ✅   | ✅   |
| 17002 | Weiterverpflichtung                               | WiM Gas                                      | ✅   | ✅   |
| 17003 | Beauftragung Änderung Technik                     | WiM Gas                                      | ✅   | ⚠️  |
| 17004 | Anforderung von Werten                            | WiM Strom Teil 2                             | ✅   | ✅   |
| 17005 | Bestellung Rechnungsabwicklung MSB über LF        | WiM Strom Teil 1                             | ✅   | ✅   |
| 17006 | Beendigung Rechnungsabwicklung MSB über LF        | WiM Strom Teil 1                             | ✅   | ✅   |
| 17007 | Bestellung von Werten                             | WiM Strom Teil 2                             | ✅   | ✅   |
| 17008 | Abbestellung von Werten                           | WiM Strom Teil 2                             | ✅   | ✅   |
| 17009 | Anzeige Gerätewechselabsicht                      | WiM Gas                                      | ✅   | ✅   |
| 17011 | Bestellung Angebot Änderung Technik               | WiM Strom Teil 1                             | ✅   | ✅   |
| 17101 | Anfrage Stammdaten Marktlokation (Gas)            | GeLi Gas 2.0                                 | ✅   | ✅   |
| 17102 | Anfrage von Werten                                | GPKE Teil 4                                  | ✅   | ✅   |
| 17103 | Anfrage Brennwert / Zustandszahl                  | GeLi Gas 2.0                                 | ✅   | ✅   |
| 17104 | Anfrage vom MSB Gas                               | GPKE Teil 4                                  | ✅   | ✅   |
| 17110 | Anforderung der Allokationsliste                  | Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ✅   |
| 17113 | Reklamation von Werten                            | WiM Gas                                      | ✅   | ✅   |
| 17114 | Anforderung bilanzierte Menge                     | Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ⚠️  |
| 17115 | Sperrauftrag                                      | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 17116 | Anfrage Sperrung                                  | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 17117 | Entsperrauftrag                                   | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 17118 | Bestellung einer Konfigurationsänderung           | GPKE Teil 3                                  | ✅   | ✅   |
| 17120 | Bestellung Änderung Prognosegrundlage             | GPKE Teil 3                                  | ✅   | ✅   |
| 17121 | Bestellung Änderung                               | GPKE Teil 3                                  | ✅   | ✅   |
| 17122 | Reklamation einer Definition                      | GPKE Teil 3                                  | ✅   | ✅   |
| 17123 | Bestellung Änderung Zählzeitdefinition            | GPKE Teil 3                                  | ✅   | ✅   |
| 17126 | Anfrage Stammdaten Messlokation (Gas)             | GeLi Gas 2.0                                 | ✅   | ✅   |
| 17128 | Reklamation einer Konfiguration                   | GPKE Teil 3                                  | ✅   | ✅   |
| 17129 | Bestellung Beendigung einer Konfiguration         | GPKE Teil 3                                  | ✅   | ✅   |
| 17130 | Bestellung einer Konfiguration                    | GPKE Teil 3                                  | ✅   | ✅   |
| 17131 | Bestellung Angebot einer Konfiguration            | GPKE Teil 3                                  | ✅   | ✅   |
| 17132 | Anfrage Stammdaten (Strom)                        | GPKE Teil 4                                  | ✅   | ✅   |
| 17133 | Bestellung Änderung Abrechnungsdaten              | GPKE Teil 2                                  | ✅   | ✅   |
| 17134 | Einrichtung Konfiguration Zuordnung LF von NB     | GPKE Teil 3                                  | ✅   | ✅   |
| 17135 | Einrichtung Konfiguration Zuordnung LF von MSB    | GPKE Teil 3                                  | ✅   | ✅   |
| 17201 | Anforder. normierter Profile und Profilscharen    | MaBiS                                        | ✅   | ✅   |
| 17202 | Anforder. Lieferantenclearingliste                | MaBiS                                        | ✅   | ✅   |
| 17203 | Anforder. Bilanzkreiszuordnungsliste              | MaBiS                                        | ✅   | ✅   |
| 17204 | Anforder. Clearingliste BAS                       | MaBiS                                        | ✅   | ✅   |
| 17205 | Anforder. Clearingliste DZR                       | MaBiS                                        | ✅   | ✅   |
| 17206 | Anforderung Bilanzierungsgebietsclearingliste     | MaBiS                                        | ✅   | ✅   |
| 17207 | Ab-/Bestellung BK-SZR auf Aggregationsebene RZ    | MaBiS                                        | ✅   | ✅   |
| 17208 | Anforderung Clearingliste ÜNB-DZR                 | MaBiS                                        | ✅   | ✅   |
| 17209 | Anforderung Ausfallarbeit                         | Kommunikationsprozesse Redispatch            | ✅   | ✅   |
| 17210 | Anforderung Lieferantenausfallarbeitsclearingliste| MaBiS                                        | ✅   | ✅   |
| 17211 | Reklamation Profile bzw. Profilscharen            | MABIS                                        | ✅   | ✅   |
| 17301 | Anforderung von Stammdaten bzw. Messwerten        | Prozesse zum Informationsaustausch zwischen Netzbetreiber und Herkunftsnachweis-register (HKN-R) des Umweltbundesamts (UBA)| ✅   | ✅   |

## ORDRSP AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 19001 | Bestätigung Bestellung                            | WiM Gas                                      | ✅   | ✅   |
| 19002 | Ablehnung Bestellung                              | WiM Gas                                      | ✅   | ✅   |
| 19003 | Bestätigung Weiterverpflichtung                   | WiM Gas                                      | ✅   | ✅   |
| 19004 | Ablehnung Weiterverpflichtung                     | WiM Gas                                      | ✅   | ✅   |
| 19005 | Bestätigung Auftrag Änderung Technik              | WiM Gas                                      | ✅   | ✅   |
| 19006 | Ablehnung Auftrag Änderung Technik                | WiM Gas                                      | ✅   | ✅   |
| 19007 | Ablehnung Anforderung Werte                       | WiM Strom Teil 2                             | ✅   | ✅   |
| 19009 | Bestätigung Beendigung Rechnungsabwicklung MSB    | WiM Strom Teil 1                             | ✅   | ✅   |
| 19010 | Ablehnung Beendigung Rechnungsabwicklung MSB      | WiM Strom Teil 1                             | ✅   | ✅   |
| 19011 | Bestätigung der Ab-/Bestellung von Werten         | WiM Strom Teil 2                             | ✅   | ✅   |
| 19012 | Ablehnung der Ab-/Bestellung von Werten           | WiM Strom Teil 2                             | ✅   | ✅   |
| 19013 | Bestätigung der Stornierung einer Bestellung      | WiM Strom Teil 2                             | ✅   | ✅   |
| 19014 | Ablehnung der Stornierung einer Bestellung        | WiM Strom Teil 2                             | ✅   | ✅   |
| 19015 | Bestätigung Gerätewechselabsicht                  | WiM Gas                                      | ✅   | ✅   |
| 19016 | Ablehnung Gerätewechselabsicht                    | WiM Gas                                      | ✅   | ✅   |
| 19101 | Ablehnung der Anfrage  Stammdaten                 | GPKE Teil 4                                  | ✅   | ✅   |
| 19102 | Ablehnung der Anfrage Werte                       | GPKE Teil 4                                  | ✅   | ✅   |
| 19103 | Ablehnung der Anfrage Brennwert / Zustandszahl    | GeLi Gas 2.0                                 | ✅   | ✅   |
| 19104 | Ablehnung der Anfrage vom MSB Gas                 | GPKE Teil 4                                  | ✅   | ✅   |
| 19110 | Ablehnung der Anforderung Allokationsliste        | Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ✅   |
| 19114 | Ablehnung Reklamation                             | WiM Gas                                      | ✅   | ✅   |
| 19115 | Ablehnung der Anforderung bilanzierte Menge       | Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ⚠️  |
| 19116 | Bestätigung Sperr-/Entsperrauftrag                | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 19117 | Ablehnung Sperr-/Entsperrauftrag                  | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 19118 | Bestätigung Anfrage Sperrung                      | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 19119 | Ablehnung Anfrage Sperrung                        | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 19120 | Mitteilung zur Änderung                           | GPKE Teil 3                                  | ✅   | ✅   |
| 19121 | Mitteilung zur Änderung Prognosegrundlage         | GPKE Teil 3                                  | ✅   | ✅   |
| 19123 | Ablehnung Reklamation einer Definition            | GPKE Teil 3                                  | ✅   | ✅   |
| 19124 | Mitteilung zur Änderung Zählzeitdefinition        | GPKE Teil 3                                  | ✅   | ✅   |
| 19127 | Mitteilung zur Konfigurationsänderung             | GPKE Teil 3                                  | ✅   | ✅   |
| 19128 | Bestätigung Stornierung Sperr-/Entsperrauftrag    | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 19129 | Ablehnung Stornierung Sperr-/Entsperrauftrag      | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 19130 | Bearbeitungsstand Reklamation Konfiguration       | GPKE Teil 3                                  | ✅   | ✅   |
| 19131 | Mitteilung zur Beendigung Konfiguration           | GPKE Teil 3                                  | ✅   | ✅   |
| 19132 | Mitteilung zur Bestellung Konfiguration           | GPKE Teil 3                                  | ✅   | ✅   |
| 19133 | Bearbeitungsstand Bestellung Änderung Abrechnungsdaten| GPKE Teil 2                                  | ✅   | ✅   |
| 19204 | Ablehnung Ab-/Bestellung der Aggregationsebene    | MaBiS                                        | ✅   | ✅   |
| 19301 | Abl. der Anforderung                              | Prozesse zum Informationsaustausch zwischen Netzbetreiber und Herkunftsnachweis-register (HKN-R) des Umweltbundesamts (UBA)| ✅   | ✅   |
| 19302 | Best. der Anforderung zum Beenden des Abos zur Stammdaten bzw. Messwertübermittlung| Prozesse zum Informationsaustausch zwischen Netzbetreiber und Herkunftsnachweis-register (HKN-R) des Umweltbundesamts (UBA)| ✅   | ✅   |

## ORDCHG AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 39000 | Stornierung Sperr-/Entsperrauftrag                | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 39001 | Weiterleitung der Stornierung                     | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 39002 | Stornierung der Bestellung                        | WiM Strom Teil 2                             | ✅   | ✅   |

## IFTSTA AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 21000 | Statusmeldung                                     | MaBiS                                        | ✅   | ✅   |
| 21001 | Statusmeldung                                     | MaBiS                                        | ✅   | ✅   |
| 21002 | Abweisung                                         | MaBiS                                        | ✅   | ✅   |
| 21003 | Statusmeldung                                     | MaBiS                                        | ✅   | ✅   |
| 21004 | Statusmeldung                                     | MaBiS                                        | ✅   | ✅   |
| 21005 | Statusmeldung                                     | MaBiS                                        | ✅   | ✅   |
| 21007 | Statusmeldung                                     | WiM Gas                                      | ✅   | ✅   |
| 21009 | Statusmeldung                                     | WiM Gas                                      | ✅   | ✅   |
| 21010 | Statusmeldung                                     | WiM Gas                                      | ✅   | ✅   |
| 21011 | Statusmeldung                                     | WiM Gas                                      | ✅   | ✅   |
| 21012 | Statusmeldung                                     | WiM Gas                                      | ✅   | ✅   |
| 21013 | Statusmeldung                                     | WiM Gas                                      | ✅   | ✅   |
| 21015 | Informationsmeldung                               | WiM Gas                                      | ✅   | ⚠️  |
| 21018 | Statusmeldung                                     | WiM Gas                                      | ✅   | ✅   |
| 21024 | Statusmeldung                                     | WiM Gas                                      | ✅   | ⚠️  |
| 21025 | Statusmeldung                                     | WiM Gas                                      | ✅   | ✅   |
| 21026 | Statusmeldung                                     | WiM Gas                                      | ✅   | ⚠️  |
| 21027 | Statusmeldung                                     | WiM Gas                                      | ✅   | ✅   |
| 21028 | Informationsmeldung                               | GeLi Gas 2.0                                 | ✅   | ✅   |
| 21029 | Vorabinformation                                  | WiM Strom Teil 1                             | ✅   | ✅   |
| 21030 | iMS-Ersteinbauzust.                               | WiM Strom Teil 1                             | ✅   | ✅   |
| 21031 | Bestandss. / Eigenausbau iMS                      | WiM Strom Teil 1                             | ✅   | ✅   |
| 21032 | Antwort auf das Angebot                           | WiM Strom Teil 1                             | ✅   | ✅   |
| 21033 | Ablehnung der Anfrage                             | GPKE Teil 3                                  | ✅   | ✅   |
| 21035 | Rückmeld. a. Liefers.                             | GPKE Teil 2                                  | ✅   | ✅   |
| 21036 | Statusmeldung                                     | WiM Strom Teil 1                             | ✅   | ✅   |
| 21037 | Ansicht NB                                        | Kommunikationsprozesse Redispatch            | ✅   | ✅   |
| 21038 | Ansicht BTR                                       | Kommunikationsprozesse Redispatch            | ✅   | ✅   |
| 21039 | Auftragsstatus (Sperren)                          | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 21040 | Info Entsperrauftrag                              | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 21042 | Bestellung (WiM)                                  | WiM Strom Teil 2                             | ✅   | ✅   |
| 21043 | Bestellungsantwort / -mitteilung                  | GPKE Teil 3                                  | ✅   | ✅   |
| 21044 | Bestellungsbeendigung                             | GPKE Teil 3                                  | ✅   | ✅   |
| 21045 | EnFG Informationen                                | GPKE Teil 4                                  | ✅   | ✅   |
| 21047 | Bearbeitungsstandsmeldung                         | GPKE Teil 2                                  | ✅   | ✅   |

## MSCONS AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 13002 | Zählerstand (Gas)                                 | WiM Gas                                      | ✅   | ✅   |
| 13003 | Summenzeitreihe                                   | MaBiS                                        | ✅   | ✅   |
| 13005 | EEG-Überf.-ZR                                     | Geschäftsprozesse für EEG-Überführungszeitreihen V1.0| ✅   | ✅   |
| 13006 | Messwert Storno                                   | WiM Gas                                      | ✅   | ✅   |
| 13007 | Gasbeschaffenheit                                 | KoV Leitfaden Marktprozesse Bilanzkreismanagement Gas| ✅   | ✅   |
| 13008 | Lastgang (Gas)                                    | KoV Leitfaden Marktprozesse Bilanzkreismanagement Gas| ✅   | ✅   |
| 13009 | Energiemenge (Gas)                                | WiM Gas                                      | ✅   | ✅   |
| 13010 | normiertes Profil                                 | MaBiS                                        | ✅   | ✅   |
| 13011 | Profilschar                                       | MaBiS                                        | ✅   | ✅   |
| 13012 | TEP vergh. Werte Referenzmessung                  | MaBiS                                        | ✅   | ✅   |
| 13013 | Marktlokationsscharfe Allokationsliste Gas (MMMA) | Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ✅   |
| 13014 | Marktlokationsscharfe bilanzierte Menge Strom/Gas (MMMA)| Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ✅   |
| 13015 | Arbeit Leistungsmax. Kalenderjahr vor
Lieferbeginn| GPKE Teil 2                                  | ✅   | ✅   |
| 13016 | Energiemenge u. Leistungsmax. (Strom)             | GPKE Teil 2                                  | ✅   | ✅   |
| 13017 | Zählerstand (Strom)                               | Prozesse zum Informationsaustausch zwischen Netzbetreiber und Herkunftsnachweis-register (HKN-R) des Umweltbundesamts (UBA)| ✅   | ✅   |
| 13018 | Lastgang Messlokation, Netzkoppelpunkt, Netzlokation| MaBiS BK6-19-218 Bilanzkreistreue            | ✅   | ✅   |
| 13019 | Energiemenge (Strom)                              | Prozesse zum Informationsaustausch zwischen Netzbetreiber und Herkunftsnachweis-register (HKN-R) des Umweltbundesamts (UBA)| ✅   | ✅   |
| 13020 | Ausfallarbeitsüberführungszeitreihe               | MaBiS                                        | ✅   | ✅   |
| 13021 | Übermittlung von meteorologischen Daten           | Kommunikationsprozesse Redispatch            | ✅   | ✅   |
| 13022 | Redispatch 2.0 Einzelzeitreihe Ausfallarbeit      | Kommunikationsprozesse Redispatch            | ✅   | ✅   |
| 13023 | Redispatch 2.0 Ausfallarbeitssummenzeitreihe      | MaBiS                                        | ✅   | ✅   |
| 13025 | Lastgang Marktlokation, Tranche                   | Prozesse zum Informationsaustausch zwischen Netzbetreiber und Herkunftsnachweis-register (HKN-R) des Umweltbundesamts (UBA)| ✅   | ✅   |
| 13026 | EEG-Überf.-ZR Aufgrund Ausfallarbeit              | Geschäftsprozesse für EEG-Überführungszeitreihen V1.0| ✅   | ✅   |
| 13027 | Werte nach Typ 2                                  | WiM Strom Teil 2                             | ✅   | ✅   |
| 13028 | Grundlage POG-Ermittlung                          | GPKE Teil 4                                  | ✅   | ✅   |

## INVOIC AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 31001 | Abschlagsrechnung                                 | GPKE Teil 2                                  | ✅   | ✅   |
| 31002 | NN-Rechnung                                       | GPKE Teil 2                                  | ✅   | ✅   |
| 31003 | WiM-Rechnung                                      | WiM Gas                                      | ✅   | ✅   |
| 31004 | Stornorechnung                                    | WiM Gas                                      | ✅   | ✅   |
| 31005 | MMM-Rechnung                                      | Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ✅   |
| 31006 | MMM-selbst ausgest. Rechnung                      | Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ✅   |
| 31007 | Aggreg. MMM-Rechnung                              | Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ✅   |
| 31008 | Aggreg. MMM-selbst ausgest. Rechnung              | Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas| ✅   | ✅   |
| 31009 | MSB-Rechnung                                      | GPKE Teil 3                                  | ✅   | ✅   |
| 31010 | Kapazitätsrechnung                                | Prozessbeschreibung zur Kapazitätsabrechnung an Ausspeisepunkten zu Letztverbrauchern| ✅   | ✅   |
| 31011 | Rechnung sonstige Leistung                        | AWH Sperrprozesse Gas                        | ✅   | ✅   |

## REMADV AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 33001 | Bestätigung                                       | WiM Gas                                      | ✅   | ✅   |
| 33002 | Abweisung                                         | WiM Gas                                      | ✅   | ✅   |
| 33003 | Strom Abweisung Kopf und Summe                    | GPKE Teil 2                                  | ✅   | ✅   |
| 33004 | Strom Abweisung Position                          | GPKE Teil 2                                  | ✅   | ✅   |

## PARTIN AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 37000 | Kommunikationsdaten des LF Strom                  | GPKE Teil 4                                  | ✅   | ✅   |
| 37001 | Kommunikationsdaten des NB Strom                  | GPKE Teil 4                                  | ✅   | ✅   |
| 37002 | Kommunikationsdaten des MSB Strom                 | GPKE Teil 4                                  | ✅   | ✅   |
| 37003 | Kommunikationsdaten des BKV Strom                 | GPKE Teil 4                                  | ✅   | ✅   |
| 37004 | Kommunikationsdaten des BIKO Strom                | GPKE Teil 4                                  | ✅   | ✅   |
| 37005 | Kommunikationsdaten des ÜNB Strom                 | GPKE Teil 4                                  | ✅   | ✅   |
| 37006 | Kommunikationsdaten des ESA Strom                 | GPKE Teil 4                                  | ✅   | ✅   |
| 37008 | Kommunikationsdaten des LF Gas                    | GeLi Gas 2.0                                 | ✅   | ✅   |
| 37009 | Kommunikationsdaten des NB Gas                    | GeLi Gas 2.0                                 | ✅   | ✅   |
| 37010 | Kommunikationsdaten des MSB Gas                   | GeLi Gas 2.0                                 | ✅   | ✅   |
| 37011 | Kommunikationsdaten des MGV Gas                   | GeLi Gas 2.0                                 | ✅   | ✅   |
| 37012 | Spartenübergreifende Kommunikationsdaten des NB Gas| GeLi Gas 2.0                                 | ✅   | ✅   |
| 37013 | Spartenübergreifende Kommunikationsdaten des MSB Gas| GeLi Gas 2.0                                 | ✅   | ✅   |
| 37014 | Spartenübergreifende Kommunikationsdaten des MSB Strom| GeLi Gas 2.0                                 | ✅   | ✅   |

## REQOTE AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 35001 | Anfrage Geräteübernahmeangebot                    | WiM Gas                                      | ✅   | ✅   |
| 35002 | Anfrage Rechnungsabwicklung MSB über LF           | WiM Strom Teil 1                             | ✅   | ✅   |
| 35003 | Anfrage von Werten                                | WiM Strom Teil 2                             | ✅   | ✅   |
| 35004 | Anfrage einer Konfiguration                       | GPKE Teil 3                                  | ✅   | ✅   |
| 35005 | Anfrage Angebot Änderung Technik                  | AWH Prozesse zur Änderung der Technik an Lokationen| ✅   | ✅   |

## QUOTES AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 15001 | Angebot Geräteübernahme                           | WiM Gas                                      | ✅   | ✅   |
| 15002 | Angebot Abrechnung Messstellenbetrieb MSB         | WiM Strom Teil 1                             | ✅   | ✅   |
| 15003 | Angebot zur Anfrage von Werten                    | WiM Strom Teil 2                             | ✅   | ✅   |
| 15004 | Angebot  einer Konfiguration                      | GPKE Teil 3                                  | ✅   | ✅   |
| 15005 | Angebot Änderung Technik                          | AWH Prozesse zur Änderung der Technik an Lokationen| ✅   | ✅   |

## PRICAT AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 27001 | Übermittlung Ausgleichsenergiepreis               | MaBiS                                        | ✅   | ✅   |
| 27002 | Preisblätter MSB-Leistungen                       | GPKE Teil 3                                  | ✅   | ✅   |
| 27003 | Preisblätter NB-Leistungen                        | AWH Sperrprozesse Gas                        | ✅   | ✅   |

## INSRPT AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 23001 | Störungsmeldung                                   | WiM Gas                                      | ✅   | ✅   |
| 23003 | Ablehnung                                         | WiM Gas                                      | ✅   | ✅   |
| 23004 | Bestätigung                                       | WiM Gas                                      | ✅   | ✅   |
| 23005 | Informationsmeldung                               | WiM Gas                                      | ✅   | ✅   |
| 23008 | Ergebnisbericht                                   | WiM Gas                                      | ✅   | ✅   |
| 23009 | Informationsmeldung                               | WiM Gas                                      | ✅   | ✅   |
| 23011 | Informationsmeldung                               | WiM Strom Teil 2                             | ✅   | ✅   |
| 23012 | Informationsmeldung                               | WiM Strom Teil 2                             | ✅   | ✅   |

## UTILTS AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 25001 | Berechnungsformel                                 | WiM Strom Teil 2                             | ✅   | ✅   |
| 25004 | Übermittlung Übersicht Zählzeitdefinitionen       | GPKE Teil 3                                  | ✅   | ✅   |
| 25005 | Übermittlung einer ausgerollten Zählzeitdefinition| GPKE Teil 3                                  | ✅   | ✅   |
| 25006 | Übermittlung Übersicht Schaltzeitdefinitionen     | GPKE Teil 3                                  | ✅   | ✅   |
| 25007 | Übermittlung Übersicht Leistungskurvendefinitionen| GPKE Teil 3                                  | ✅   | ✅   |
| 25008 | Übermittlung einer ausgerollten Schaltzeitdefinition| GPKE Teil 3                                  | ✅   | ✅   |
| 25009 | Übermittlung einer ausgerollten Leistungskurvendefinition| GPKE Teil 3                                  | ✅   | ✅   |
| 25010 | Antwort auf Berechnungsformel                     | WiM Strom Teil 2                             | ✅   | ✅   |

## COMDIS AHB

| PID    | Description                                       | Process                                       | 3.3  | 4.0  |
|--------|---------------------------------------------------|-----------------------------------------------|------|------|
| 29001 | Ablehnung REMADV                                  | AWH Sperrprozesse Gas                        | ✅   | ✅   |
| 29002 | Ablehnung IFTSTA                                  | GPKE Teil 2                                  | ✅   | ✅   |

---

*Source: BDEW PID 3.3 (FV2025-10-01, Fehlerkorrektur 27.03.2026) and PID 4.0 (FV2026-10-01).*

---

## Redispatch 2.0 — XML document types (not EDIFACT PIDs)

Redispatch 2.0 uses CIM/IEC 62325-based **XML** documents, not EDIFACT. These
document types have no Prüfidentifikator and are therefore not listed in the
BDEW PID overview. They are handled by the `redispatch-xml` crate.

| Document type | XSD version | Crate |
|---|---|---|
| `ActivationDocument` | 1.1f | `redispatch-xml` |
| `PlannedResourceScheduleDocument` | 1.0f | `redispatch-xml` |
| `AcknowledgementDocument` | 1.0f | `redispatch-xml` |
| `Stammdaten` | 1.4b | `redispatch-xml` |
| `StatusRequest_MarketDocument` | 1.1 | `redispatch-xml` |
| `Unavailability_MarketDocument` | 1.1b | `redispatch-xml` |
| `Kaskade` | 1.0 | `redispatch-xml` |
| `NetworkConstraintDocument` | 1.1b | `redispatch-xml` |
| `Kostenblatt` | 1.0d | `redispatch-xml` |

IFTSTA status messages that accompany Redispatch 2.0 workflows are EDIFACT and
use PIDs 21035–21047 (see the IFTSTA AHB section above).

See [`crates/redispatch-xml`](../crates/redispatch-xml/README.md) for schema
documentation and the parse/serialize/validate API.

