//! State shared with every Axum handler.

use std::sync::Arc;

use crate::config::Config;
use crate::core::context::AppContext;

/// Cloneable handle passed to handlers as `axum::extract::State`.
#[derive(Clone)]
pub struct AppState {
    ctx: Arc<AppContext>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

impl AppState {
    /// Wrap an application context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// The application context.
    pub fn ctx(&self) -> &Arc<AppContext> {
        &self.ctx
    }

    /// The live configuration.
    pub fn config(&self) -> &Arc<Config> {
        self.ctx.config()
    }
}

impl From<Arc<AppContext>> for AppState {
    fn from(ctx: Arc<AppContext>) -> Self {
        Self::new(ctx)
    }
}
