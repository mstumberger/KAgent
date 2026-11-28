//! The shipped list and the two pure rules on it (local-models.md section 2): how well an
//! entry fits this machine, and which entry of each job is "Recommended".

use std::collections::BTreeMap;

use kagent_core::platform::gpu::Gpu;
use serde::{Deserialize, Serialize};

/// A job of a local model. Embedding and speech to text have no chat role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Job {
    Chat,
    Vision,
    Embedding,
    Stt,
}

impl Job {
    pub const ALL: [Job; 4] = [Job::Chat, Job::Vision, Job::Embedding, Job::Stt];
    pub fn name(self) -> &'static str {
        match self {
            Job::Chat => "chat",
            Job::Vision => "vision",
            Job::Embedding => "embedding",
            Job::Stt => "stt",
        }
    }
}

/// One entry of `config/local-models.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub jobs: Vec<Job>,
    pub size_mb: u64,
    /// What it needs loaded at its default quantisation with `context`.
    pub vram_mb: u64,
    pub context: u64,
    /// Calls tools reliably: a chat model without it is a helper, never the Brain.
    pub tools: bool,
    pub languages: Vec<String>,
    pub note: String,
    /// `ollama` (downloadable) or `whisper` (listed only).
    pub engine: String,
}

impl Entry {
    pub fn downloadable(&self) -> bool {
        self.engine == "ollama"
    }
    pub fn has(&self, job: Job) -> bool {
        self.jobs.contains(&job)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct File {
    #[allow(dead_code)]
    schema_version: u32,
    models: Vec<Entry>,
}

/// The shipped list (`config/local-models.json`, read-only).
pub fn shipped() -> Vec<Entry> {
    serde_json::from_str::<File>(include_str!("../../../../config/local-models.json")).expect("config/local-models.json is well formed").models
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Fit {
    Fits,
    Tight,
    TooBig,
    /// No graphics card: runs on the processor.
    Slow,
}

/// `fits`: at most 85 % of the card's memory; `tight`: up to all of it; else `tooBig`. Without
/// a card the entry is judged against RAM: `slow` up to half of it, else `tooBig`.
pub fn fit(vram_mb: u64, gpu: Option<&Gpu>, ram_total_mb: u64) -> Fit {
    match gpu {
        Some(g) if vram_mb * 100 <= g.vram_total_mb * 85 => Fit::Fits,
        Some(g) if vram_mb <= g.vram_total_mb => Fit::Tight,
        Some(_) => Fit::TooBig,
        None if vram_mb * 2 <= ram_total_mb => Fit::Slow,
        None => Fit::TooBig,
    }
}

/// The second line of a row: what other programs hold of the card right now.
pub fn in_use_note(vram_mb: u64, gpu: Option<&Gpu>) -> Option<String> {
    let g = gpu?;
    (vram_mb > g.vram_free_mb).then(|| format!("{} MB are in use by other programs at the moment", g.vram_total_mb.saturating_sub(g.vram_free_mb)))
}

fn speaks(e: &Entry, language: &str) -> bool {
    let want = language.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    !want.is_empty() && want != "auto" && e.languages.iter().any(|l| l.eq_ignore_ascii_case(&want))
}

/// The "Recommended" entry of each job: the largest that `fits`, preferring the ones that list
/// the owner's language. The chat recommendation is the Brain, so a chat model without tools
/// is never recommended (it can still be installed as a helper). Speech to text has none: it
/// cannot be downloaded. `fits[i]` is the fit of `models[i]`.
pub fn recommend(models: &[Entry], fits: &[Fit], language: &str) -> BTreeMap<Job, String> {
    let mut out = BTreeMap::new();
    for job in [Job::Chat, Job::Vision, Job::Embedding] {
        let mut pool: Vec<&Entry> = models.iter().zip(fits).filter(|(e, f)| e.has(job) && e.downloadable() && **f == Fit::Fits).map(|(e, _)| e).collect();
        if job == Job::Chat {
            pool.retain(|e| e.tools);
        }
        if pool.iter().any(|e| speaks(e, language)) {
            pool.retain(|e| speaks(e, language));
        }
        // The largest; the first of the list on a tie.
        if let Some(best) = pool.iter().fold(None::<&&Entry>, |best, e| if best.is_none_or(|b| e.size_mb > b.size_mb) { Some(e) } else { best }) {
            out.insert(job, best.id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(total: u64, free: u64) -> Gpu {
        Gpu { name: "Test".into(), vram_total_mb: total, vram_free_mb: free, source: "nvidia-smi".into() }
    }

    fn picks(g: Option<&Gpu>, ram: u64, language: &str) -> (BTreeMap<Job, String>, Vec<(String, Fit)>) {
        let list = shipped();
        let fits: Vec<Fit> = list.iter().map(|e| fit(e.vram_mb, g, ram)).collect();
        let table = list.iter().zip(&fits).map(|(e, f)| (e.id.clone(), *f)).collect();
        (recommend(&list, &fits, language), table)
    }

    fn of(table: &[(String, Fit)], id: &str) -> Fit {
        table.iter().find(|(i, _)| i == id).unwrap().1
    }

    #[test]
    fn the_shipped_file_parses_with_unique_ids() {
        let list = shipped();
        assert!(list.len() >= 8);
        let mut ids: Vec<&str> = list.iter().map(|e| e.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), list.len(), "every id is unique");
        for e in &list {
            assert!(!e.jobs.is_empty() && e.size_mb > 0 && e.vram_mb > 0, "{}", e.id);
            assert!(e.engine == "ollama" || e.engine == "whisper", "{}", e.id);
            assert_eq!(e.downloadable(), !e.has(Job::Stt), "{}: only speech models are not downloadable", e.id);
        }
    }

    #[test]
    fn unknown_keys_fail_loudly() {
        let bad = r#"{"schemaVersion":1,"models":[{"id":"x","name":"X","jobs":["chat"],"sizeMb":1,"vramMb":1,"context":1,"tools":true,"languages":[],"note":"","engine":"ollama","typo":1}]}"#;
        assert!(serde_json::from_str::<File>(bad).is_err());
        let bad_job = bad.replace("\"typo\":1", "\"x\":1").replace("\"chat\"", "\"chatty\"");
        assert!(serde_json::from_str::<File>(&bad_job).is_err());
    }

    #[test]
    fn the_rule_at_its_edges() {
        let g = gpu(10000, 10000);
        assert_eq!(fit(8500, Some(&g), 0), Fit::Fits);
        assert_eq!(fit(8501, Some(&g), 0), Fit::Tight);
        assert_eq!(fit(10000, Some(&g), 0), Fit::Tight);
        assert_eq!(fit(10001, Some(&g), 0), Fit::TooBig);
        assert_eq!(fit(500, None, 1000), Fit::Slow);
        assert_eq!(fit(501, None, 1000), Fit::TooBig);
    }

    #[test]
    fn twelve_gb() {
        let g = gpu(12288, 9550);
        let (rec, t) = picks(Some(&g), 64000, "auto");
        assert_eq!(of(&t, "qwen2.5:7b-instruct"), Fit::Fits);
        assert_eq!(of(&t, "qwen2.5:14b-instruct"), Fit::Tight);
        assert_eq!(of(&t, "gemma3:12b"), Fit::Tight);
        assert_eq!(rec[&Job::Chat], "llama3.1:8b");
        assert_eq!(rec[&Job::Vision], "gemma3:4b");
        assert_eq!(rec[&Job::Embedding], "bge-m3");
        assert!(!rec.contains_key(&Job::Stt));
    }

    #[test]
    fn eight_gb() {
        let g = gpu(8192, 8000);
        let (rec, t) = picks(Some(&g), 32000, "auto");
        assert_eq!(of(&t, "qwen2.5:7b-instruct"), Fit::Fits);
        assert_eq!(of(&t, "llama3.1:8b"), Fit::Fits);
        assert_eq!(of(&t, "gemma3:12b"), Fit::TooBig);
        assert_eq!(of(&t, "qwen2.5:14b-instruct"), Fit::TooBig);
        assert_eq!(rec[&Job::Chat], "llama3.1:8b");
        assert_eq!(rec[&Job::Vision], "gemma3:4b");
    }

    #[test]
    fn four_gb() {
        let g = gpu(4096, 4000);
        let (rec, t) = picks(Some(&g), 16000, "auto");
        assert_eq!(of(&t, "gemma3:4b"), Fit::TooBig);
        assert_eq!(of(&t, "nomic-embed-text"), Fit::Fits);
        assert!(!rec.contains_key(&Job::Chat), "nothing for chat fits");
        assert!(!rec.contains_key(&Job::Vision));
        assert_eq!(rec[&Job::Embedding], "bge-m3");
    }

    #[test]
    fn no_gpu_is_slow_and_recommends_nothing() {
        let (rec, t) = picks(None, 16000, "auto");
        assert_eq!(of(&t, "qwen2.5:7b-instruct"), Fit::Slow);
        assert_eq!(of(&t, "qwen2.5:14b-instruct"), Fit::TooBig);
        assert!(rec.is_empty(), "slow is not fits");
    }

    #[test]
    fn the_owners_language_wins_among_the_fitting() {
        let g = gpu(12288, 12000);
        let (rec, _) = picks(Some(&g), 64000, "sl");
        assert_eq!(rec[&Job::Vision], "gemma3:4b");
        assert_eq!(rec[&Job::Embedding], "bge-m3");
        let (rec, _) = picks(Some(&g), 64000, "sl-SI");
        assert_eq!(rec[&Job::Embedding], "bge-m3");
        let (rec, _) = picks(Some(&g), 64000, "en");
        assert_eq!(rec[&Job::Embedding], "bge-m3", "the larger one when both list the language");
    }

    #[test]
    fn a_chat_model_without_tools_is_never_the_brain_while_one_with_tools_fits() {
        let g = gpu(12288, 12000);
        // Slovenian is listed only by Gemma (no tools); the Brain stays a tools model.
        let (rec, _) = picks(Some(&g), 64000, "sl");
        assert_eq!(rec[&Job::Chat], "llama3.1:8b");
        // Only a tools-less chat model fits: no Brain is recommended.
        let g = gpu(5400, 5400);
        let (rec, t) = picks(Some(&g), 64000, "auto");
        assert_eq!(of(&t, "gemma3:4b"), Fit::Fits);
        assert!(!rec.contains_key(&Job::Chat));
    }

    #[test]
    fn the_note_about_other_programs() {
        let g = gpu(12288, 9550);
        assert_eq!(in_use_note(6000, Some(&g)), None);
        assert_eq!(in_use_note(10500, Some(&g)).unwrap(), "2738 MB are in use by other programs at the moment");
        assert_eq!(in_use_note(10500, None), None);
    }
}
