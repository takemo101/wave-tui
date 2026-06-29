# Browse Search Results Filter Design

## Summary

Make Browse genre selections filter the current Radio Browser search result set when search results are available. Today, Browse genre sources only filter the small curated catalog, which makes genre selection feel like it shows recommendations rather than all currently discovered stations. The new behavior should treat Browse as a filter over the active search result population while preserving the existing curated fallback when no search results exist.

## Product Fit

`wave-tui` is a terminal-first work-session radio player. Search is the discovery path; Browse should help narrow that discovery set without turning the app into a full music platform. This design keeps the existing calm Browse rail and avoids adding new playlist/library concepts.

## Current Behavior

- `ListSource::Category(category)` calls `Catalog::category_stations(category)` in `src/app.rs`.
- `Catalog` intentionally owns a small curated/recommended station set.
- Radio Browser results arrive via `Action::SearchResults` and replace `visible`, but the reducer does not keep the full search result set as a reusable Browse population.
- Selecting a Browse genre after searching therefore jumps back to curated category stations instead of filtering the full search result set.

## User Experience

### Search Results as Browse Population

When at least one successful Radio Browser search result set exists, Browse sources should use that search result set as their population:

- `All Stations` shows all current search results.
- Genre sources such as `Lofi`, `Jazz`, `Electronic`, `News`, and `Talk` filter the current search results.
- Switching between genres always filters from the full search result set, not from the already-filtered visible rows.

When no successful search result set exists, Browse keeps the current curated behavior:

- `All Stations` shows the full curated catalog.
- Genre sources show curated category stations.

### Search Clearing

Clearing search removes the search-result population but preserves the active Browse source:

- If `Jazz` is active over search results, clearing search should show curated `Jazz`.
- If `All Stations` is active over search results, clearing search should show curated `All Stations`.

This keeps Browse selection stable while changing only the population from search results back to curated catalog.

### New Search Results

If a new successful search response arrives while a Browse genre is active, keep the genre active and apply it to the new search result set. For example, if `Jazz` is active and the user changes the search query, the next result set should be filtered to Jazz matches immediately.

If a later search request fails or the app is offline, keep the last successful search result population available. The existing offline/error state should still be shown, but prior results should remain filterable.

### Favorites

`Favorites` is not a genre filter. It remains the saved-favorites source and always shows persisted favorite stations in saved order. Do not reinterpret it as "favorites within the current search results".

### Browse Labels and Status Context

Keep Browse rail labels stable. In particular, do not dynamically rename `All Stations` to `Search Results` or `All Results`.

Instead, show search/filter context in the search/status strip. Examples:

```text
filter: Jazz · 12 results
filter: All Stations · 48 results
```

Exact copy can be adjusted during implementation, but the UI should make it clear when Browse is filtering current search results.

### Empty Filter Results

If search results exist but a genre filter matches zero stations, do not silently fall back to curated stations. Show a short, specific empty state in the Results pane, for example:

```text
No Jazz results in current search
```

## Category Matching

Radio Browser stations do not carry the app's `Category` enum, so category membership must be inferred from station metadata.

Use a safe, small alias dictionary:

- Match primarily against station `tags`.
- Use station `name` as a secondary fallback.
- Do not use `country` or `language` as primary genre signals.

Initial aliases:

| Category | Aliases |
| --- | --- |
| `Lofi` | `lofi`, `chillhop`, `beats` |
| `Ambient` | `ambient`, `drone`, `space`, `atmospheric` |
| `Jazz` | `jazz`, `smooth jazz` |
| `Classical` | `classical`, `baroque`, `orchestral` |
| `Electronic` | `electronic`, `house`, `techno`, `downtempo`, `deep-house` |
| `News` | `news`, `public radio`, `world news` |
| `Talk` | `talk`, `spoken`, `community` |

Implementation may normalize case, whitespace, punctuation, and separators so common Radio Browser tag variations still match. Keep the alias set conservative; expand later based on observed misses.

## Architecture Direction

App state should distinguish between:

- the current visible station list;
- the last successful full search result population;
- the active Browse source.

The reducer should rebuild `visible` from the appropriate population whenever a Browse source or search result changes.

A small category matching helper can live near catalog/search boundary code. It should operate on typed `Station` values and avoid leaking raw Radio Browser DTOs into `app`.

Candidate responsibilities:

- `catalog` or a focused helper owns `station_matches_category(station, category)` and alias tests.
- `app` owns population selection and source application.
- `ui` only renders the resulting visible list, search/filter context, and empty-state text.

## Behavior Boundaries

Do not change:

- Radio Browser API request shape;
- search debounce/caching behavior;
- Favorites persistence or identity rules;
- playback behavior;
- Browse rail labels or order;
- layout tier thresholds;
- key mappings.

## Acceptance Criteria

- With search results available, selecting `All Stations` shows all search results.
- With search results available, selecting a Browse genre filters from the full search result set.
- Switching from one genre to another always filters from the full search result set, not from the previously filtered visible list.
- New successful search results preserve and reapply the active Browse genre filter.
- Search failures keep the last successful search result population available for filtering while surfacing the existing offline/error state.
- Clearing search preserves the active Browse source but rebuilds it from curated catalog.
- With no search results available, Browse genre behavior matches the current curated fallback behavior.
- `Favorites` remains the saved-favorites source and is not filtered by current search results.
- The search/status strip shows active filter context when filtering search results.
- Zero search-result matches for a genre render a specific empty state such as `No Jazz results in current search`.

## Testing

Prefer pure reducer and rendering tests:

- category alias matching from tags and station name;
- no match for unrelated tags/names;
- search results are retained as a full population separate from `visible`;
- Browse genre filters from the full search population;
- switching genres does not filter from the already-filtered visible list;
- `All Stations` restores all search results when search population exists;
- new search results preserve active Browse filter;
- search failure/offline state does not clear prior successful search population;
- clearing search preserves active source and falls back to curated population;
- `Favorites` ignores current search population;
- search/status strip shows filter context;
- zero filtered matches show the specific empty-state copy.

## Validation

Run at least:

```bash
cargo fmt --check
cargo test app
cargo test ui
cargo test search
cargo check
cargo clippy --all-targets -- -D warnings
```
