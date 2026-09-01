# Native Preprint Sources Plan

## Goals

- Discover relevant papers directly from bioRxiv and arXiv before they appear in OpenAlex.
- Merge native records with OpenAlex and curated collections without duplication.
- Preserve every path through which a paper was discovered.
- Keep the default feed focused while making broader categories available but unselected.
- Retain predictable RSS behavior and avoid repeated historical downloads.

## Phase 1: Generalize discovery provenance

Replace the provider-versus-curated boolean with structured discovery-source records. Initial
source kinds cover OpenAlex, Google Scholar, bioRxiv categories, arXiv categories, and curated
collections. Merged works retain all contributing sources, and older persisted snapshots remain
readable. Reader source filters use the structured provenance while preserving the existing
"Exclude collection-only papers" behavior.

## Phase 2: Add reusable discovery snapshots

Create source-independent persistent snapshots for native repositories. Snapshots track the last
successful refresh, support overlapping incremental windows, deduplicate records, enforce
retention, replace files atomically, and allow callers to serve the last successful data when a
refresh fails. Source-specific code remains responsible for HTTP requests, cursors, and parsing.

## Phase 3: Pilot bioRxiv ingestion

Use the official paginated JSON API. Begin with `synthetic_biology` and `bioinformatics`. Capture
DOI, title, authors, abstract, date, version, category, license, publication linkage, PDF URL, and
JATS XML URL. Validate categories against an allowlist and merge records through the existing work
pipeline.

## Phase 4: Pilot arXiv ingestion

Use a combined RSS request for `q-bio.BM` and `q-bio.QM`, plus a rate-limited Atom API backfill.
Include new and cross-listed announcements, deduplicate by arXiv ID, and preserve categories,
license, abstract-page URL, PDF URL, and the `10.48550/arXiv.*` DOI.

## Phase 5: Add optional broader categories

Make broader categories available but unselected by default:

- bioRxiv: `bioengineering`, `systems_biology`, `genomics`, `biochemistry`, and
  `molecular_biology`
- arXiv: `q-bio.GN`, `q-bio.MN`, `q-bio.PE`, and `physics.bio-ph`

Configuration distinguishes `default_categories` from `optional_categories`. The browser can
select optional categories without changing the raw RSS defaults. Unfiltered `cs.LG` remains
deferred; a later keyword-filtered source is preferable because of its volume.

## Phase 6: Complete reader and RSS integration

Show exact repository/category provenance on article pages and RSS descriptions. Group source
filters by native repository and curated collection, support category multiselect with OR
semantics, and combine category, author, date, and provenance groups with AND semantics.

## Phase 7: Operational rollout

Run native sources first in a dedicated evaluation feed. Compare unique additions, OpenAlex and
curated overlap, metadata completeness, duplicates, and a manual relevance sample for at least one
normal publication week. Enable pilot categories on the main feed only after the evaluation, then
expand optional categories based on measured value.
