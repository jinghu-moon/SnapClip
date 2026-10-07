//! History state: the rows the view shows and the selection it reports.
//!
//! The model owns the query and the selection; the view owns only presentation. Selection
//! is a **clip id**, not a row index, so filtering and reordering keep pointing at the same
//! entry (the Design Guides require selection by domain identity).

use std::path::Path;

use snapclip_history::store::Store;
use snapclip_history::StoreError;
use snapclip_model::{ClipSummary, PayloadKind, PayloadRef};

/// Rows fetched per page. The list is virtualized, so this is a paging decision only.
/// `None` would let the store pick its own default; the shell asks for an explicit page.
const PAGE_SIZE: Option<u32> = Some(50);

pub struct HistoryState {
    store: Store,
    items: Vec<ClipSummary>,
    next_cursor: Option<String>,
    selected: Option<String>,
    query: String,
    /// The type filter, mirroring the shape the old front end had (`all` = `None`).
    kind: Option<PayloadKind>,
}

impl HistoryState {
    /// Open the same database the Tauri host writes (`<app local data>`) and read the first
    /// page.
    ///
    /// The first page is loaded here rather than by whoever renders the screen: a history
    /// screen that opens onto an empty list and only fills in once the user types is broken,
    /// and "open" failing to read is an error the caller has to see anyway.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::with_store(Store::open(root)?)
    }

    /// The same, over a store the caller already opened.
    ///
    /// `Store` is cheap to clone, and the composition root opens exactly one so the clipboard
    /// pipeline and the screen read and write through the same writer thread rather than
    /// through two competing ones.
    pub fn with_store(store: Store) -> Result<Self, StoreError> {
        let mut state = Self {
            store,
            items: Vec::new(),
            next_cursor: None,
            selected: None,
            query: String::new(),
            kind: None,
        };
        state.reload()?;
        Ok(state)
    }

    pub fn items(&self) -> &[ClipSummary] {
        &self.items
    }

    /// Whether the store has more rows behind the current window.
    pub fn has_more(&self) -> bool {
        self.next_cursor.is_some()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn kind(&self) -> Option<&PayloadKind> {
        self.kind.as_ref()
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Set the type filter and reload the first page. Returns whether anything changed.
    pub fn set_kind(&mut self, kind: Option<PayloadKind>) -> Result<bool, StoreError> {
        if self.kind == kind {
            return Ok(false);
        }
        self.kind = kind;
        self.reload()?;
        Ok(true)
    }

    /// Set the search text and reload the first page. Returns whether anything changed.
    pub fn set_query(&mut self, query: &str) -> Result<bool, StoreError> {
        if self.query == query {
            return Ok(false);
        }
        self.query = query.to_string();
        self.reload()?;
        Ok(true)
    }

    /// Load the first page for the current query.
    pub fn reload(&mut self) -> Result<(), StoreError> {
        let page = self.fetch(None)?;
        self.items = page.items;
        self.next_cursor = page.next_cursor;
        self.keep_selection_valid();
        Ok(())
    }

    /// Append the next page. Returns how many rows were actually added.
    ///
    /// A row can be added to the store between two pages, which shifts the cursor window and
    /// makes the boundary row appear twice; the id check is what keeps the list a set of
    /// distinct entries rather than a set of distinct fetches.
    pub fn load_more(&mut self) -> Result<usize, StoreError> {
        let Some(cursor) = self.next_cursor.clone() else {
            return Ok(0);
        };
        let page = self.fetch(Some(cursor))?;
        let mut added = 0;
        for item in page.items {
            if self.items.iter().any(|existing| existing.id == item.id) {
                continue;
            }
            self.items.push(item);
            added += 1;
        }
        self.next_cursor = page.next_cursor;
        Ok(added)
    }

    /// Re-read the current query without losing how deep the user has scrolled.
    ///
    /// This is the "a clip arrived while the window was open" path: a plain reload would
    /// collapse a list the user had loaded five pages of back to one page.
    pub fn refresh(&mut self) -> Result<(), StoreError> {
        let wanted = self.items.len();
        self.reload()?;
        while self.items.len() < wanted && self.next_cursor.is_some() {
            if self.load_more()? == 0 {
                break;
            }
        }
        Ok(())
    }

    fn fetch(&self, cursor: Option<String>) -> Result<snapclip_model::HistoryPage, StoreError> {
        // One call covers all four combinations: the store treats an empty query as "no text
        // filter" and an absent kind as "every kind". Splitting them here (as the first slice
        // did) is how `kind` would silently stop applying to an empty search box.
        self.store.search_history_page(
            self.query.clone(),
            self.kind.clone(),
            cursor,
            PAGE_SIZE,
        )
    }

    /// Delete the selected entry. Returns the id that was removed.
    ///
    /// The model owns this because the *policy* is the model's: what "selected" means, what
    /// to do afterwards (refresh without collapsing the user's loaded pages), and what the
    /// user is told. The store only knows rows.
    pub fn delete_selected(&mut self) -> Result<Option<String>, StoreError> {
        let Some(id) = self.selected.clone() else {
            return Ok(None);
        };
        if !self.store.delete_publication(id.clone())? {
            // Another shell already deleted it; the refresh below still fixes the list.
            self.refresh()?;
            return Ok(None);
        }
        self.refresh()?;
        Ok(Some(id))
    }

    /// The bytes of the first image payload of `item`, for a row preview.
    ///
    /// Mirrors what the old front end's `image_payload_data_url` did (the first image payload
    /// of the row), minus the base64 step: a native shell can hand the bytes straight to the
    /// renderer.
    pub fn image_bytes(&self, item: &ClipSummary) -> Option<Result<Vec<u8>, StoreError>> {
        let image = item
            .payloads
            .iter()
            .find(|payload| payload.kind == PayloadKind::Image)?;
        Some(self.store.read_payload_bytes(
            image.content_hash.clone(),
            PayloadKind::Image,
        ))
    }

    /// Select the first row when nothing is selected, or when the selection vanished
    /// (the entry was deleted, or the filter no longer matches it).
    fn keep_selection_valid(&mut self) {
        let still_present = self
            .selected
            .as_deref()
            .is_some_and(|id| self.items.iter().any(|item| item.id == id));
        if !still_present {
            self.selected = self.items.first().map(|item| item.id.clone());
        }
    }

    pub fn select(&mut self, id: &str) {
        if self.items.iter().any(|item| item.id == id) {
            self.selected = Some(id.to_string());
        }
    }

    /// The payload `Enter` would copy: the primary payload of the selected row.
    ///
    /// Returns the reference *and* its bytes, because the caller (the shell) is what turns
    /// them into a clipboard write; the model never touches the clipboard.
    pub fn selected_payload_bytes(
        &self,
    ) -> Option<Result<(PayloadRef, PayloadKind, Vec<u8>), StoreError>> {
        let id = self.selected.as_deref()?;
        let item = self.items.iter().find(|item| item.id == id)?;
        let payload = item
            .payloads
            .iter()
            .find(|payload| payload.kind == item.primary_kind)
            .or_else(|| item.payloads.first())?
            .clone();
        // `PayloadKind` is a plain enum without `Copy`, so take the value once and reuse it.
        let kind = payload.kind.clone();
        let bytes = match self
            .store
            .read_payload_bytes(payload.content_hash.clone(), kind.clone())
        {
            Ok(bytes) => bytes,
            Err(error) => return Some(Err(error)),
        };
        Some(Ok((payload, kind, bytes)))
    }

    /// Move the selection by `delta` rows, clamped to the loaded window.
    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.selected = None;
            return;
        }
        let current = self
            .selected
            .as_deref()
            .and_then(|id| self.items.iter().position(|item| item.id == id))
            .unwrap_or(0);
        let next = (current as isize + delta).clamp(0, self.items.len() as isize - 1) as usize;
        self.selected = Some(self.items[next].id.clone());
    }
}

#[cfg(test)]
mod tests {
    //! Pure model tests: they need neither a window nor the component runtime.

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use snapclip_history::store::Store;
    use snapclip_model::{
        PayloadData, PayloadKind, PayloadRef, Publication, PublicationOrigin,
    };

    use super::HistoryState;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "snapclip-app-history-{name}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn save(store: &Store, id: &str, text: &str) {
        let bytes = text.as_bytes().to_vec();
        let payload = PayloadRef {
            payload_id: format!("{id}-0"),
            content_hash: blake3::hash(&bytes).to_hex().to_string(),
            kind: PayloadKind::Text,
            size_bytes: bytes.len() as u64,
            mime_type: Some(PayloadKind::Text.default_mime_type().to_string()),
            image_dimensions: None,
        };
        store
            .save_publication(
                Publication {
                    publication_id: id.to_string(),
                    origin: PublicationOrigin::Clipboard,
                    captured_at_unix_ms: 1_700_000_000_000,
                    source_app: Some("Test App".to_string()),
                    source_exe_path: Some(r"C:\test\app.exe".to_string()),
                    payloads: vec![payload.clone()],
                },
                vec![PayloadData::new(payload, bytes)],
            )
            .expect("save publication");
    }

    fn save_image(store: &Store, id: &str, bytes: &[u8]) {
        let bytes = bytes.to_vec();
        let payload = PayloadRef {
            payload_id: format!("{id}-0"),
            content_hash: blake3::hash(&bytes).to_hex().to_string(),
            kind: PayloadKind::Image,
            size_bytes: bytes.len() as u64,
            mime_type: Some(PayloadKind::Image.default_mime_type().to_string()),
            image_dimensions: None,
        };
        store
            .save_publication(
                Publication {
                    publication_id: id.to_string(),
                    origin: PublicationOrigin::Clipboard,
                    captured_at_unix_ms: 1_700_000_000_000,
                    source_app: Some("Test App".to_string()),
                    source_exe_path: None,
                    payloads: vec![payload.clone()],
                },
                vec![PayloadData::new(payload, bytes)],
            )
            .expect("save image publication");
    }

    #[test]
    fn the_first_row_is_selected_and_arrows_move_by_domain_id() {
        let dir = root("select");
        {
            let store = Store::open(&dir).unwrap();
            save(&store, "clip-1", "first entry");
            save(&store, "clip-2", "second entry");
        }
        let mut state = HistoryState::open(&dir).unwrap();
        state.reload().unwrap();
        assert_eq!(state.items().len(), 2);
        // Newest first: the second save is the first row.
        assert_eq!(state.selected_id(), Some("clip-2"));
        state.move_selection(1);
        assert_eq!(state.selected_id(), Some("clip-1"));
        // Clamped at the ends rather than wrapping.
        state.move_selection(1);
        assert_eq!(state.selected_id(), Some("clip-1"));
        state.move_selection(-1);
        assert_eq!(state.selected_id(), Some("clip-2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_search_that_matches_nothing_leaves_no_selection() {
        let dir = root("empty");
        {
            let store = Store::open(&dir).unwrap();
            save(&store, "clip-1", "alpha");
        }
        let mut state = HistoryState::open(&dir).unwrap();
        state.reload().unwrap();
        assert!(state.set_query("nothing matches this").unwrap());
        assert!(state.items().is_empty());
        assert_eq!(state.selected_id(), None);
        // Clearing the query brings the row back and re-selects it.
        state.set_query("").unwrap();
        assert_eq!(state.selected_id(), Some("clip-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_query_is_not_a_change() {
        let dir = root("same-query");
        let mut state = HistoryState::open(&dir).unwrap();
        assert!(state.set_query("clip").unwrap());
        assert!(!state.set_query("clip").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The screen shows rows the moment it opens; this is the guard for the bug the UI test
    /// caught, where opening produced an empty list until the user typed something.
    #[test]
    fn opening_reads_the_first_page() {
        let dir = root("opens-loaded");
        {
            let store = Store::open(&dir).unwrap();
            save(&store, "clip-1", "already stored");
        }
        let state = HistoryState::open(&dir).unwrap();
        assert_eq!(state.items().len(), 1);
        assert_eq!(state.selected_id(), Some("clip-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_matching_query_keeps_only_that_row() {
        let dir = root("matching");
        {
            let store = Store::open(&dir).unwrap();
            save(&store, "clip-1", "alpha note");
            save(&store, "clip-2", "beta note");
        }
        let mut state = HistoryState::open(&dir).unwrap();
        state.reload().unwrap();
        assert_eq!(state.items().len(), 2);
        assert!(state.set_query("alpha").unwrap());
        assert_eq!(state.items().len(), 1);
        assert_eq!(state.items()[0].id, "clip-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// More rows than one page exist only if the store's page size is smaller than the
    /// fixture; `PAGE_SIZE` is 50, so the fixture is built from the store's own cursor
    /// instead of assuming a number.
    #[test]
    fn load_more_appends_the_next_page_and_then_stops() {
        let dir = root("paging");
        {
            let store = Store::open(&dir).unwrap();
            for index in 0..120 {
                save(&store, &format!("clip-{index:03}"), &format!("entry {index}"));
            }
        }
        let mut state = HistoryState::open(&dir).unwrap();
        state.reload().unwrap();
        let first_page = state.items().len();
        assert_eq!(first_page, 50, "the shell asks for 50 rows per page");
        assert!(state.has_more());

        assert_eq!(state.load_more().unwrap(), 50);
        assert_eq!(state.items().len(), 100);
        assert_eq!(state.load_more().unwrap(), 20);
        assert_eq!(state.items().len(), 120);
        assert!(!state.has_more());
        // Past the end there is nothing more to add, and asking again is not an error.
        assert_eq!(state.load_more().unwrap(), 0);
        // No row was shown twice, which is what the id check in `load_more` is for.
        let unique: std::collections::HashSet<_> =
            state.items().iter().map(|item| item.id.clone()).collect();
        assert_eq!(unique.len(), state.items().len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_keeps_the_rows_the_user_had_loaded_and_picks_up_a_new_one() {
        let dir = root("refresh");
        {
            let store = Store::open(&dir).unwrap();
            for index in 0..60 {
                save(&store, &format!("clip-{index:03}"), &format!("entry {index}"));
            }
        }
        let mut state = HistoryState::open(&dir).unwrap();
        state.reload().unwrap();
        state.load_more().unwrap();
        assert_eq!(state.items().len(), 60);

        // A clip arrives while the window is open.
        {
            let store = Store::open(&dir).unwrap();
            save(&store, "clip-new", "the newest entry");
        }
        state.refresh().unwrap();

        // The new row is first, and the user's depth survived: the old page-1 rows are
        // still there rather than the list collapsing to 50.
        assert_eq!(state.items()[0].id, "clip-new");
        assert_eq!(state.items().len(), 61);
        assert!(state.items().iter().any(|item| item.id == "clip-059"));
        // The selection does not jump under the user: it is still the row they had chosen,
        // and only falls back to the top when that row is gone.
        assert_eq!(state.selected_id(), Some("clip-059"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_kind_filter_applies_with_and_without_a_search_query() {
        let dir = root("kind-filter");
        {
            let store = Store::open(&dir).unwrap();
            save(&store, "clip-text", "a note");
            save_image(&store, "clip-image", b"png bytes");
        }
        let mut state = HistoryState::open(&dir).unwrap();
        assert_eq!(state.items().len(), 2, "no filter shows everything");

        assert!(state.set_kind(Some(PayloadKind::Image)).unwrap());
        assert_eq!(state.items().len(), 1);
        assert_eq!(state.items()[0].id, "clip-image");
        // Setting the same filter again is not a change (and must not re-query).
        assert!(!state.set_kind(Some(PayloadKind::Image)).unwrap());

        // The filter survives a search, which is the combination that used to lose it: the
        // searched text belongs to the *text* row, so a matching query must still come back
        // empty while the image filter is on.
        state.set_query("a note").unwrap();
        assert!(
            state.items().is_empty(),
            "a text row must not appear while the image filter is on"
        );

        assert!(state.set_kind(None).unwrap());
        assert_eq!(state.items().len(), 1, "the text row is back once unfiltered");
        assert_eq!(state.items()[0].id, "clip-text");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_the_selection_removes_it_and_moves_the_selection_on() {
        let dir = root("delete-selected");
        {
            let store = Store::open(&dir).unwrap();
            save(&store, "clip-1", "first");
            save(&store, "clip-2", "second");
        }
        let mut state = HistoryState::open(&dir).unwrap();
        assert_eq!(state.selected_id(), Some("clip-2"));
        assert_eq!(state.delete_selected().unwrap().as_deref(), Some("clip-2"));
        assert_eq!(state.items().len(), 1);
        assert_eq!(state.selected_id(), Some("clip-1"));

        // With nothing left, deleting is a no-op rather than an error.
        state.select("clip-1");
        assert_eq!(state.delete_selected().unwrap().as_deref(), Some("clip-1"));
        assert!(state.items().is_empty());
        assert_eq!(state.delete_selected().unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_image_row_offers_its_png_bytes_and_a_text_row_offers_none() {
        let dir = root("image-bytes");
        {
            let store = Store::open(&dir).unwrap();
            save(&store, "clip-text", "a note");
            save_image(&store, "clip-image", b"png bytes");
        }
        let state = HistoryState::open(&dir).unwrap();
        let image = state
            .items()
            .iter()
            .find(|item| item.id == "clip-image")
            .unwrap();
        let bytes = state.image_bytes(image).expect("image row has bytes").unwrap();
        assert_eq!(bytes, b"png bytes");
        let text = state
            .items()
            .iter()
            .find(|item| item.id == "clip-text")
            .unwrap();
        assert!(state.image_bytes(text).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
