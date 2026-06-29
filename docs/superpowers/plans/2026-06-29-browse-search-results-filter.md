# Browse Search Results Filter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Browse sources filter the current Radio Browser search result population when available, while preserving curated fallback behavior and Favorites semantics.

**Architecture:** Add a typed category-matching helper for `Station` values, then teach `App` to retain the last successful search result population separately from the visible list. UI remains render-only: it displays filter context and specific empty-state copy based on app display state.

**Tech Stack:** Rust 2021, Ratatui 0.29, existing `App` reducer tests, existing pure UI buffer tests, mikan for issue tracking.

## Global Constraints

- Follow `docs/superpowers/specs/2026-06-29-browse-search-results-filter-design.md`.
- Do not change Radio Browser API request shape, debounce, or cache behavior.
- Do not change Favorites persistence or identity rules.
- Do not change playback behavior.
- Do not change Browse rail labels or order.
- Do not change layout tier thresholds or key mappings.
- Use TDD: write a failing focused test before production changes in each task.
- Keep `.mikan/**` out of PR diffs.

---

## File Structure

- `src/catalog.rs`
  - Add `station_matches_category(station: &Station, category: Category) -> bool`.
  - Add `station_matches_section(station: &Station, section: Section) -> bool`.
  - Own conservative alias matching over station tags and station name.
- `src/app.rs`
  - Add `search_population: Option<Stations>` or equivalent.
  - Rebuild `visible` from either search population or curated catalog based on active source.
  - Preserve active Browse source when successful search results arrive and when search is cleared.
  - Keep Favorites isolated from search population.
  - Expose display helpers for UI filter context and search-filter empty state.
- `src/ui.rs`
  - Render filter context in the search/status strip.
  - Render `No <Category> results in current search` when search population exists, a genre/section filter is active, and the filtered visible list is empty.
- `docs/SPEC.md`
  - Update Browse/Search behavior after implementation.
- `docs/ui-design-decisions.md`
  - Record the finalized Browse-over-search-results behavior after implementation.

---

### Task 1: Category and Section Matching Helpers

**Files:**

- Modify: `src/catalog.rs`

**Interfaces:**

- Produces: `pub fn station_matches_category(station: &Station, category: Category) -> bool`
- Produces: `pub fn station_matches_section(station: &Station, section: Section) -> bool`
- Consumed by later tasks: `src/app.rs` source rebuilding.

- [ ] **Step 1: Write failing tests for tag and name alias matching**

Add tests in `src/catalog.rs` near existing catalog tests:

```rust
#[test]
fn station_matches_category_from_tags_and_name_aliases() {
    let mut tagged = station("tagged", CodecKind::Mp3, Some(128), Some(10));
    tagged.tags = vec!["smooth jazz".to_string(), "night".to_string()];
    tagged.name = StationName::new("Late Night Radio").unwrap();
    assert!(station_matches_category(&tagged, Category::Jazz));

    let mut named = station("named", CodecKind::Mp3, Some(128), Some(10));
    named.tags = vec!["music".to_string()];
    named.name = StationName::new("Deep House Session").unwrap();
    assert!(station_matches_category(&named, Category::Electronic));
}
```

- [ ] **Step 2: Write failing tests for conservative non-matches and sections**

Add:

```rust
#[test]
fn station_category_matching_is_conservative_and_sections_compose_categories() {
    let mut station = station("talk", CodecKind::Mp3, Some(128), Some(10));
    station.tags = vec!["community".to_string(), "spoken".to_string()];
    station.name = StationName::new("Neighborhood Voice").unwrap();

    assert!(station_matches_category(&station, Category::Talk));
    assert!(station_matches_section(&station, Section::SpokenNews));
    assert!(!station_matches_category(&station, Category::Jazz));
    assert!(!station_matches_section(&station, Section::Music));
}
```

- [ ] **Step 3: Run tests to verify RED**

Run:

```bash
cargo test catalog::tests::station_matches_category_from_tags_and_name_aliases
cargo test catalog::tests::station_category_matching_is_conservative_and_sections_compose_categories
```

Expected: fail because `station_matches_category` and `station_matches_section` do not exist.

- [ ] **Step 4: Implement matching helpers**

Add in `src/catalog.rs` near `station_score` or category helpers:

```rust
pub fn station_matches_section(station: &Station, section: Section) -> bool {
    section
        .categories()
        .iter()
        .any(|&category| station_matches_category(station, category))
}

pub fn station_matches_category(station: &Station, category: Category) -> bool {
    let aliases = category_aliases(category);
    station
        .tags
        .iter()
        .any(|tag| matches_alias(tag, aliases))
        || matches_alias(station.name.as_str(), aliases)
}

fn category_aliases(category: Category) -> &'static [&'static str] {
    match category {
        Category::Lofi => &["lofi", "chillhop", "beats"],
        Category::Ambient => &["ambient", "drone", "space", "atmospheric"],
        Category::Jazz => &["jazz", "smooth jazz"],
        Category::Classical => &["classical", "baroque", "orchestral"],
        Category::Electronic => &["electronic", "house", "techno", "downtempo", "deep-house"],
        Category::News => &["news", "public radio", "world news"],
        Category::Talk => &["talk", "spoken", "community"],
    }
}

fn matches_alias(value: &str, aliases: &[&str]) -> bool {
    let normalized = normalize_alias_text(value);
    aliases
        .iter()
        .map(|alias| normalize_alias_text(alias))
        .any(|alias| normalized.contains(&alias))
}

fn normalize_alias_text(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
```

- [ ] **Step 5: Run tests to verify GREEN**

Run:

```bash
cargo test catalog::tests::station_matches_category_from_tags_and_name_aliases
cargo test catalog::tests::station_category_matching_is_conservative_and_sections_compose_categories
cargo test catalog
```

Expected: all pass.

---

### Task 2: App Search Population and Browse Source Rebuild

**Files:**

- Modify: `src/app.rs`
- Uses: `src/catalog.rs::station_matches_category`, `src/catalog.rs::station_matches_section`

**Interfaces:**

- Consumes: `station_matches_category(&Station, Category) -> bool`
- Consumes: `station_matches_section(&Station, Section) -> bool`
- Produces: app state that keeps a full search result population separate from `visible`.
- Produces: display helpers for Task 3:
  - `pub fn has_search_population(&self) -> bool`
  - `pub fn active_filter_label(&self) -> Option<&'static str>`
  - `pub fn search_filter_empty_note(&self) -> Option<String>`

- [ ] **Step 1: Write failing test: All Stations uses search population**

Add in `src/app.rs` tests:

```rust
#[test]
fn browse_all_stations_uses_search_population_when_available() {
    let mut app = base_app();
    app.apply(Action::SearchResults(SearchResults::from_stations([
        station("search-a"),
        station("search-b"),
    ])));

    assert_eq!(app.active_source(), ListSource::AllStations);
    assert_eq!(visible_ids(&app), vec!["search-a", "search-b"]);

    app.apply(Action::ShowCatalog);
    assert_eq!(visible_ids(&app), vec!["search-a", "search-b"]);
}
```

- [ ] **Step 2: Write failing test: category filters from full search population**

Add:

```rust
#[test]
fn browse_category_filters_full_search_population_not_current_visible() {
    let mut jazz = station("jazz");
    jazz.tags = vec!["jazz".to_string()];
    let mut house = station("house");
    house.tags = vec!["house".to_string()];

    let mut app = base_app();
    app.apply(Action::SearchResults(SearchResults::from_stations([
        jazz.clone(),
        house.clone(),
    ])));

    app.apply(Action::ShowCategory(Category::Jazz));
    assert_eq!(visible_ids(&app), vec!["jazz"]);

    app.apply(Action::ShowCategory(Category::Electronic));
    assert_eq!(visible_ids(&app), vec!["house"]);
}
```

- [ ] **Step 3: Write failing test: new search results preserve active filter**

Add:

```rust
#[test]
fn new_search_results_preserve_active_browse_filter() {
    let mut first_jazz = station("first-jazz");
    first_jazz.tags = vec!["jazz".to_string()];
    let mut first_house = station("first-house");
    first_house.tags = vec!["house".to_string()];

    let mut second_jazz = station("second-jazz");
    second_jazz.tags = vec!["smooth jazz".to_string()];
    let mut second_house = station("second-house");
    second_house.tags = vec!["techno".to_string()];

    let mut app = base_app();
    app.apply(Action::SearchResults(SearchResults::from_stations([
        first_jazz,
        first_house,
    ])));
    app.apply(Action::ShowCategory(Category::Jazz));

    app.apply(Action::SearchResults(SearchResults::from_stations([
        second_jazz,
        second_house,
    ])));

    assert_eq!(app.active_source(), ListSource::Category(Category::Jazz));
    assert_eq!(visible_ids(&app), vec!["second-jazz"]);
}
```

- [ ] **Step 4: Write failing test: clear search preserves source and falls back to curated**

Add:

```rust
#[test]
fn clearing_search_preserves_browse_source_and_rebuilds_from_curated() {
    let mut search_jazz = station("search-jazz");
    search_jazz.tags = vec!["jazz".to_string()];

    let mut app = base_app();
    app.apply(Action::SearchResults(SearchResults::from_stations([search_jazz])));
    app.apply(Action::ShowCategory(Category::Jazz));
    assert_eq!(visible_ids(&app), vec!["search-jazz"]);

    app.apply(Action::ClearSearch);

    let curated_jazz = app
        .catalog
        .category_stations(Category::Jazz)
        .iter()
        .map(|station| station.id.as_str().to_string())
        .collect::<Vec<_>>();
    assert_eq!(app.active_source(), ListSource::Category(Category::Jazz));
    assert_eq!(visible_ids(&app), curated_jazz);
}
```

- [ ] **Step 5: Write failing test: Favorites ignores search population**

Add:

```rust
#[test]
fn favorites_source_ignores_search_population() {
    let favorite = fav_station("fav-only");
    let settings = Settings {
        favorites: Favorites::from_stations([favorite.clone()]),
        ..Settings::default()
    };
    let mut app = App::new(settings, Catalog::curated());

    let mut search_jazz = station("search-jazz");
    search_jazz.tags = vec!["jazz".to_string()];
    app.apply(Action::SearchResults(SearchResults::from_stations([search_jazz])));
    app.apply(Action::ShowFavorites);

    assert_eq!(app.active_source(), ListSource::Favorites);
    assert_eq!(visible_ids(&app), vec!["fav-only"]);
}
```

If no `Action::ShowFavorites` exists, use `app.apply(Action::ShowSource(ListSource::Favorites))` only if such an action exists; otherwise apply the Browse selection path already used in existing Favorites tests.

- [ ] **Step 6: Run tests to verify RED**

Run each new test by name. Expected: failures showing current implementation still sets `ListSource::Search` and/or uses curated-only category sources.

- [ ] **Step 7: Implement app search population**

Modify `src/app.rs`:

1. Import helpers:

```rust
use crate::catalog::{
    station_matches_category, station_matches_section, Catalog, Category, Section,
    SessionStationHealth, Stations,
};
```

1. Add field to `App`:

```rust
search_population: Option<Stations>,
```

1. Initialize it in `App::new`:

```rust
search_population: None,
```

1. Add population helpers:

```rust
fn source_stations(&self, source: ListSource) -> Stations {
    match source {
        ListSource::AllStations => self
            .search_population
            .clone()
            .unwrap_or_else(|| self.catalog.stations().ranked()),
        ListSource::Section(section) => self.section_source_stations(section),
        ListSource::Category(category) => self.category_source_stations(category),
        ListSource::Favorites => self.favorite_stations(),
        ListSource::Search => self
            .search_population
            .clone()
            .unwrap_or_else(|| self.catalog.stations().ranked()),
    }
}

fn section_source_stations(&self, section: Section) -> Stations {
    if let Some(population) = &self.search_population {
        population
            .iter()
            .filter(|station| station_matches_section(station, section))
            .cloned()
            .collect()
    } else {
        self.catalog.section_stations(section)
    }
}

fn category_source_stations(&self, category: Category) -> Stations {
    if let Some(population) = &self.search_population {
        population
            .iter()
            .filter(|station| station_matches_category(station, category))
            .cloned()
            .collect()
    } else {
        self.catalog.category_stations(category)
    }
}
```

1. Simplify `show_source` to use `source_stations`:

```rust
fn show_source(&mut self, source: ListSource) {
    self.source = source;
    let stations = self.source_stations(source);
    self.replace_visible(stations);
}
```

1. Update `apply_search_results`:

```rust
fn apply_search_results(&mut self, results: SearchResults) {
    self.offline = false;
    let stations: Stations = results.into_vec().into_iter().collect();
    self.search_population = Some(stations);
    let source = if self.source.is_search() {
        self.previous_source
    } else {
        self.source
    };
    self.show_source(source);
}
```

1. Update `clear_search`:

```rust
fn clear_search(&mut self) {
    self.search_population = None;
    let source = if self.source.is_search() {
        self.previous_source
    } else {
        self.source
    };
    self.show_source(source);
}
```

1. Keep offline/search failure actions from clearing `search_population`.

- [ ] **Step 8: Add UI display helpers**

Add methods on `App`:

```rust
pub fn has_search_population(&self) -> bool {
    self.search_population.is_some()
}

pub fn active_filter_label(&self) -> Option<&'static str> {
    if !self.has_search_population() {
        return None;
    }
    match self.source {
        ListSource::AllStations | ListSource::Section(_) | ListSource::Category(_) => {
            Some(self.source.title())
        }
        ListSource::Favorites | ListSource::Search => None,
    }
}

pub fn search_filter_empty_note(&self) -> Option<String> {
    if !self.has_search_population() || !self.visible.is_empty() {
        return None;
    }
    match self.source {
        ListSource::Section(section) => {
            Some(format!("No {} results in current search", section.title()))
        }
        ListSource::Category(category) => {
            Some(format!("No {} results in current search", category.title()))
        }
        _ => None,
    }
}
```

- [ ] **Step 9: Run tests to verify GREEN**

Run:

```bash
cargo test app
cargo test catalog
cargo check
```

Expected: all pass. Update existing tests that expected `ListSource::Search` after `Action::SearchResults` to the new agreed behavior: active Browse source is preserved, defaulting to `AllStations`.

---

### Task 3: UI Filter Context and Empty State

**Files:**

- Modify: `src/ui.rs`

**Interfaces:**

- Consumes: `App::active_filter_label() -> Option<&'static str>`
- Consumes: `App::search_filter_empty_note() -> Option<String>`

- [ ] **Step 1: Write failing test for search strip filter context**

Add in `src/ui.rs` tests:

```rust
#[test]
fn search_strip_shows_active_search_filter_context() {
    let mut jazz = fav_station("search-jazz");
    jazz.tags = vec!["jazz".to_string()];
    let mut app = base_app();
    app.apply(Action::SearchResults(SearchResults::from_stations([jazz])));
    app.apply(Action::ShowCategory(Category::Jazz));

    let text = buffer_text(&render_buffer(&app, 130, 32));

    assert!(text.contains("filter: Jazz"), "filter context missing: {text}");
}
```

- [ ] **Step 2: Write failing test for zero-match empty state**

Add:

```rust
#[test]
fn search_filter_zero_matches_shows_specific_empty_state() {
    let mut house = fav_station("search-house");
    house.tags = vec!["house".to_string()];
    let mut app = base_app();
    app.apply(Action::SearchResults(SearchResults::from_stations([house])));
    app.apply(Action::ShowCategory(Category::Jazz));

    let text = buffer_text(&render_buffer(&app, 130, 32));

    assert!(
        text.contains("No Jazz results in current search"),
        "specific empty state missing: {text}"
    );
}
```

- [ ] **Step 3: Run tests to verify RED**

Run:

```bash
cargo test ui::tests::search_strip_shows_active_search_filter_context
cargo test ui::tests::search_filter_zero_matches_shows_specific_empty_state
```

Expected: fail because UI does not render these strings yet.

- [ ] **Step 4: Render filter context in search strip**

Modify `render_search_strip` to append filter context when available. Keep existing query/loading/cache/offline text.

Example helper:

```rust
fn search_filter_status(app: &App) -> Option<String> {
    app.active_filter_label()
        .map(|label| format!("filter: {label}"))
}
```

Then include it in the status line spans near existing result count/search state, styled with `theme.muted` or `theme.accent`.

- [ ] **Step 5: Render specific empty state**

Modify `empty_list_note(app)` in `src/ui.rs`:

```rust
if let Some(note) = app.search_filter_empty_note() {
    return note;
}
```

Place this before the generic Favorites/No stations branch so search filter empty state wins when relevant.

- [ ] **Step 6: Run tests to verify GREEN**

Run:

```bash
cargo test ui::tests::search_strip_shows_active_search_filter_context
cargo test ui::tests::search_filter_zero_matches_shows_specific_empty_state
cargo test ui
```

Expected: all pass.

---

### Task 4: Documentation and Final Validation

**Files:**

- Modify: `docs/SPEC.md`
- Modify: `docs/ui-design-decisions.md`
- Optional: `README.md` only if user-facing controls/copy need a short mention.

**Interfaces:**

- Consumes completed Task 1-3 behavior.
- Produces docs matching implemented Browse-over-search behavior.

- [ ] **Step 1: Update docs**

In `docs/SPEC.md`, update Browse/Search behavior to state:

```markdown
When Radio Browser search results are available, Browse `All Stations`, sections,
and categories filter that current search result set. Clearing search preserves
Browse selection but rebuilds from the curated catalog. Favorites remains the
saved-favorites source.
```

In `docs/ui-design-decisions.md`, add a concise note under Browse/Favorites Polish:

```markdown
Browse sources act as filters over current search results when a successful
search result population exists; otherwise they fall back to curated catalog
sources. Browse labels remain stable, and the search strip carries the active
filter context.
```

- [ ] **Step 2: Run focused validation**

Run:

```bash
cargo fmt --check
cargo test app
cargo test catalog
cargo test ui
cargo check
cargo clippy --all-targets -- -D warnings
```

Expected: all pass.

- [ ] **Step 3: Run diagnostics**

Run:

```bash
lens_diagnostics mode=all severity=error
```

Expected: no errors.

- [ ] **Step 4: Prepare review summary**

Summarize:

- search population is stored separately from visible list;
- Browse filters search population when available;
- curated fallback remains;
- Favorites remains isolated;
- filter context and empty state render in UI;
- validation commands pass.

---

## Self-Review

### Spec Coverage

- Search population behavior: Task 2.
- Curated fallback: Task 2.
- Favorites isolation: Task 2.
- Category matching aliases: Task 1.
- Stable Browse labels and search strip context: Task 3.
- Empty search-filter state: Task 3.
- Docs updates: Task 4.

### Placeholder Scan

No `TBD`, `TODO`, or unspecified implementation placeholders are intentionally left. Task 2 includes one branch for the existing Favorites action shape because current reducer actions should be verified at implementation time; the expected fallback is to use the existing Browse selection path already present in tests.

### Type Consistency

The plan uses existing types: `App`, `Action`, `ListSource`, `SearchResults`, `Stations`, `Station`, `Category`, `Section`, `Settings`, and `Favorites`. New proposed methods are explicitly named and consumed by later tasks.
