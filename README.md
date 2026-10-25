# MELCloud til HomewrightHQ

En ekstern integration til [HomewrightHQ](https://github.com/magnusdunker/homewrighthq) for Mitsubishi Electric-aircondition (luft-til-luft) og Ecodan-varmepumper (luft-til-vand) gennem MELCloud.

## Installation

I HomewrightHQ: **Indstillinger → Integrationer → Fra GitHub**. Indsæt

```
https://github.com/magnusdunker/homewrighthq-melcloud
```

og tryk **Installér**. Tilføj derefter **MELCloud** under **Tilføj integration**, og log ind med den e-mail og adgangskode, du bruger i MELCloud-appen.

## Hvad du får

Et luft-til-luft-anlæg bliver en enhed med:

- et klimaanlæg med tilstande (varme, køl, affugt, blæser, auto), temperatur og blæserhastighed (auto og 1 til det antal trin, anlægget har)
- lodret lamel (auto, 1-5, swing) og vandret lamel (auto, 1-5, split, swing), hvor anlægget har den
- rumtemperatur, og udetemperatur hvor MELCloud oplyser den
- samlet energiforbrug i kWh på anlæg med energimåler

Et luft-til-vand-anlæg (Ecodan) bliver en enhed med:

- en varmtvandsbeholder med måltemperatur og tilstandene `auto` og `force_hot_water`
- et klimaanlæg pr. zone (zone 2, hvor anlægget har den) med varme, køl hvor anlægget kan køle, og forindstillingerne `room` (rumtemperatur), `flow` (fremløbstemperatur) og `curve` (varmekurve)
- en indstilling for fremløbstemperaturen ved varme pr. zone
- rumtemperatur pr. zone, udetemperatur, fremløbs-, retur- og varmtvandstemperatur
- status: `idle`, `hot_water`, `heating`, `cooling`, `defrost`, `standby` eller `legionella`

Sluk på et luft-til-vand-anlæg slukker hele anlægget, også varmt vand, fordi MELCloud kun har én tænd/sluk for anlægget.

## Opdatering og MELClouds grænser

MELCloud spærrer konti, der spørger for tit. Integrationen læser hvert anlæg hvert femte minut og listen over anlæg (energi, fremløb og retur) en gang i timen. Under **Indstillinger** på integrationen kan tiden mellem opdateringer sættes fra 3 til 60 minutter. Svarer MELCloud 429 (for mange forespørgsler), holder integrationen pause i mindst 30 minutter, eller så længe MELCloud beder om.

Efter en kommando venter integrationen 90 sekunder, før den læser anlægget igen, fordi MELCloud et stykke tid kan svare med de gamle værdier. Anlægget selv henter kommandoen ved sin næste forbindelse til MELCloud, typisk inden for et minut.

## Ikke afprøvet mod den rigtige MELCloud

Integrationen er skrevet ud fra, hvordan MELClouds web-API er beskrevet i pymelcloud og Home Assistants MELCloud-integration, og afprøvet mod en falsk MELCloud med svar formet som de rigtige. Disse ting er ikke kontrolleret mod rigtige anlæg:

- `EffectiveFlags`-værdierne for luft-til-vand (tank, zonetemperatur, zonetilstand, fremløb). Værdierne for luft-til-luft er velkendte.
- Koderne for status (`OperationMode` 0-6) og zonetilstande (`OperationModeZone1/2` 0-4) på luft-til-vand.
- Om fremløbs- og returtemperaturen findes i `Device/Get`; ellers bruges værdierne fra listen over anlæg, som kun hentes en gang i timen.
- Hvilke felter der siger, at et luft-til-luft-anlæg kan affugte eller køre auto (`ModelSupportsDryMode`, `ModelSupportsAuto`). Mangler de, vises tilstanden.
- Hvor ofte MELCloud tillader forespørgsler. Fem minutter er et forsigtigt gæt.
- Den `AppVersion`, der sendes ved login.

Ventilationsanlæg (Lossnay, `DeviceType` 3) og MELCloud Home (den nye app) er ikke med.

## Udvikling

Bygget i Rust på SDK'en `thehome-plugin` fra [homewright-example-integration-rust](https://github.com/magnusdunker/homewrighthq-example-integration-rust).

- `src/api.rs`: login og MELClouds web-API
- `src/model.rs`: MELClouds data og hvordan de bliver til tilstande og kommandoer
- `src/lib.rs`: forbindelsen til HomewrightHQ
- `tests/melcloud.rs`: tests mod en falsk MELCloud med svarene i `tests/fixtures`, og en falsk HomewrightHQ i `tests/host`

```
cargo test
cargo build --release
```

En ny version udgives med et tag, f.eks. `git tag v0.1.0 && git push origin v0.1.0`. Workflowet bygger til Windows, Linux (x86_64 og arm64, statisk musl) og macOS og lægger binærerne og `homewright.json` på releasen.

## Licens

MIT
