//! The chain of engines (web.md section 3.1): cache, then each engine of the chain in
//! order, paced, skipped while it cools, put on a cooldown when it fails.

use super::super::chain::{normalise, MAX_QUEUE_MS};
use super::super::page_reader::ReaderError;
use super::super::settings::Engine;
use super::super::Web;
use super::{brave, duckduckgo, read_duckduckgo, searxng, EngineError, Hit};

/// How many results are asked of an engine and kept: a call with a smaller `limit` cuts them.
pub const FETCH: usize = 10;

/// An answer of the chain.
pub struct Found {
    pub engine: Engine,
    pub hits: Vec<Hit>,
    pub cached: bool,
}

/// Why the chain gave nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The chain is empty.
    Off,
    /// What each engine said, in order.
    Failed(Vec<(Engine, String)>),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => write!(f, "web search is off (Settings -> Web)"),
            Self::Failed(notes) => write!(f, "web search failed ({})", notes.iter().map(|(e, why)| format!("{}: {why}", e.name())).collect::<Vec<_>>().join("; ")),
        }
    }
}

impl Web {
    /// The results of the chain of settings.search, for the tool and for the module that
    /// looks a place up (calendar.md section 5). The error is in words for the model.
    pub(crate) async fn search_hits(&self, query: &str, limit: usize) -> Result<(Engine, Vec<Hit>), String> {
        let found = self.search_found(query).await.map_err(|e| e.to_string())?;
        Ok((found.engine, found.hits.into_iter().take(limit).collect()))
    }

    /// Ask the engines in order until one answers.
    pub(crate) async fn search_found(&self, query: &str) -> Result<Found, Failure> {
        let chain = self.search_config().chain();
        if chain.is_empty() {
            return Err(Failure::Off);
        }
        let key = normalise(query);
        if let Some((engine, hits)) = self.chain.cached(&key) {
            return Ok(Found { engine, hits, cached: true });
        }
        let mut notes = Vec::new();
        for engine in chain {
            if let Some(minutes) = self.chain.cooling(engine) {
                notes.push((engine, format!("on a cooldown for about {minutes} more minutes")));
                continue;
            }
            let Some(wait) = self.chain.reserve(engine) else {
                notes.push((engine, format!("busy: its queue is longer than {} s", MAX_QUEUE_MS / 1000)));
                continue;
            };
            if wait > 0 {
                self.chain.clock.sleep(wait).await;
            }
            match self.ask(engine, query).await {
                Ok(hits) => {
                    self.chain.answered(engine);
                    if !hits.is_empty() {
                        self.chain.keep(key, engine, hits.clone());
                    }
                    return Ok(Found { engine, hits, cached: false });
                }
                Err(error) => {
                    if !error.is_unset() {
                        self.chain.failed(engine);
                    }
                    notes.push((engine, error.to_string()));
                }
            }
        }
        Err(Failure::Failed(notes))
    }

    async fn ask(&self, engine: Engine, query: &str) -> Result<Vec<Hit>, EngineError> {
        match engine {
            Engine::DuckDuckGo => duckduckgo(&self.transport, &self.engines.duckduckgo, query, FETCH).await,
            Engine::Brave => {
                let key = self.brave_key().await.map_err(|e| EngineError::Key(format!("{e:#}")))?;
                brave(&self.transport, &self.engines.brave, key, query, FETCH).await
            }
            Engine::Searxng => searxng(&self.transport, &self.search_config().searx_url, query, FETCH).await,
            Engine::Reader => self.reader_search(query).await,
            Engine::None => Ok(Vec::new()),
        }
    }

    /// DuckDuckGo's result page, read by the reader like any page (web.md section 9).
    async fn reader_search(&self, query: &str) -> Result<Vec<Hit>, EngineError> {
        let Some(reader) = self.page_reader().filter(|_| self.reader_enabled()) else {
            return Err(EngineError::Skip("the reader is off or not available"));
        };
        let url = reqwest::Url::parse_with_params(&self.engines.reader, [("q", query)]).map_err(|e| EngineError::Reader(format!("the search address is not one ({e})")))?;
        match reader.render(url.as_str()).await {
            Ok(page) => read_duckduckgo(200, &page.html, FETCH),
            Err(ReaderError::Challenge(_)) => Err(EngineError::Challenge),
            Err(other) => Err(EngineError::Reader(format!("the reader could not read the results: {other}"))),
        }
    }
}
