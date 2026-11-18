//! # stemson
//!
//! A Spring-Boot-like web framework on top of [Axum], ported from the Python
//! SanicBoot project (`legacy/python`), released as **Stemson**.
//!
//! It keeps the original design:
//!
//! * YAML configuration addressed with dotted keys, with hot reload;
//! * `#[derive(Autowired)]` field injection, built from link-time registries
//!   (`#[service]`, `#[repository]`, `#[datasource("...")]`, `#[controller]`);
//! * automatic route derivation from the module path, like `@Controller`;
//! * declarative parameter validation (`#[validate(...)]`);
//! * a unified `{ message, data, code }` response body with 4 digit business
//!   codes, plus JWT authentication.
//!
//! See `docs/ARCHITECTURE.md` for the mapping from the Python sources and
//! `docs/MIGRATION.md` for every intentional behavioral difference.

extern crate self as stemson;

pub use axum;
pub use http;
pub use inventory;

pub use stemson_macros::{controller, datasource, repository, service, Autowired};

pub mod app;
pub mod auth;
pub mod config;
pub mod controller;
pub mod core;
pub mod datasource;
pub mod exception;
pub mod model;
pub mod nacos;
pub mod openapi;
pub mod repository;
pub mod request;
pub mod service;
pub mod utils;

#[cfg(test)]
pub mod test_support;

pub use app::{build_router, run};
pub use auth::{AuthUser, Claims, JwtService};
pub use config::{BaseConfig, Config};
pub use core::context::AppContext;
pub use core::state::AppState;
pub use exception::{ApiError, ApiResult};
pub use model::identity::UserIdentity;
pub use model::response::ApiResponse;
pub use model::status_code::StatusCode;
pub use request::context::{ParamScope, RequestContext, UploadedFile};
