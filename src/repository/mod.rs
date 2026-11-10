//! Repository layer, mirrors `backend/repository/`.

pub mod user_repository;

pub use user_repository::{NewUserRecord, UserRepository, UserRow};
