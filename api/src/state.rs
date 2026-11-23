//! Shared state handed to every handler.

use crate::store::Store;

#[derive(Debug, Clone)]
pub struct AppState {
    pub store: Store,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}
