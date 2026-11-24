//! Provider registry, model rules and streaming API adapters.

pub mod adapters;
pub mod client;
pub mod config;
pub mod discovery;
pub mod rules;
pub mod sse;
pub mod store;

pub use client::ProvidersClient;
pub use config::{ApiType, ProviderEntry, Template};
pub use rules::ModelConfig;
pub use store::{split_ref, ProviderStore, SecretSink};
