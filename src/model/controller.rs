//! Controller abstraction, mirrors `backend/model/Controller.py`
//! (`sanic.views.HTTPMethodView`).

use std::sync::Arc;

use axum::Router;

use crate::core::route::RouteEntry;
use crate::core::state::AppState;

/// Implemented by every `#[controller]`. Controllers expose one or more HTTP
/// methods on a single path, like `HTTPMethodView` did.
pub trait ControllerBase: Send + Sync + 'static {
    /// Registry name (snake_case type name).
    fn name(&self) -> &'static str;

    /// Base path the controller is mounted on.
    fn base_path(&self) -> String;

    /// Routes this controller serves; recorded in the route table so that the
    /// router and the OpenAPI document can both be derived from it.
    fn route_entries(&self) -> Vec<RouteEntry>;

    /// Build the Axum router fragment for this controller.
    fn routes(self: Arc<Self>) -> Router<AppState>;
}
