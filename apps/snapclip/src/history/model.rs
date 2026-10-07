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
}

impl HistoryState {
    /// Open the same database the Tauri host writes (`<app local data>`).
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self {
            store: Store::open(root)?,
            items: Vec::new(),
            next_cursor: None,
            selected: None,
            query: String::new(),
        })
    }

    pub fn items(&self) -> &[ClipSummary] {
        &self.items
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
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

    // Paging (`next_cursor` → a `load_more` that appends) lands with the scroll wiring:
    // the store already returns the cursor, and the model keeps it for that step.
    fn fetch(&self, cursor: Option<String>) -> Result<snapclip_model::HistoryPage, StoreError> {
        if self.query.trim().is_empty() {
            self.store.history_page(cursor, PAGE_SIZE)
        } else {
            self.store
                .search_history_page(self.query.clone(), None, cursor, PAGE_SIZE)
        }
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
}
