//! Marker trait for the service layer, mirrors `backend/model/Service.py`.

/// Implemented by every `#[service]`.
pub trait ServiceBase: Send + Sync + 'static {}
