pub mod auth;
pub mod domain;
pub mod error;
pub mod http;
pub mod ledger;
pub mod model;
pub mod money;
pub mod service;
pub mod transaction;

#[derive(Clone)]
pub struct AppState { pub pool: sqlx::PgPool }

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
