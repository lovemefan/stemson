//! Marker trait for datasources, mirrors
//! `backend/core/datasource/DataBasePoolBase.py`.

/// Implemented by every `#[datasource("...")]`.
pub trait DatasourceBase: Send + Sync + 'static {}
