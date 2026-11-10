//! Marker trait for the repository layer, mirrors `backend/model/Repository.py`.

/// Implemented by every `#[repository]`.
pub trait RepositoryBase: Send + Sync + 'static {}
