//! Optional Nacos service registration using the HTTP v1 OpenAPI.

pub(crate) mod client;
mod registry;

pub use registry::Registration;
