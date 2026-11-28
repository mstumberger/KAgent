//! Provider registry, model rules and streaming API adapters.

pub mod adapters;
pub mod client;
pub mod config;
pub mod discovery;
pub mod local;
pub mod rules;
pub mod sse;
pub mod store;
pub mod usage;

pub use client::ProvidersClient;
pub use config::{ApiType, ProviderEntry, Template};
pub use local::is_local_url;
pub use rules::ModelConfig;
pub use store::{split_ref, ProviderStore, SecretSink};
