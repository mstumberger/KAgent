//! Which installed model reads screenshots (screen-watch.md section 2, step 3): the
//! recommendation of the list, and what Ollama has of it.

use super::fit::{self, Job};
use super::{ollama, LocalModels};

impl LocalModels {
    /// The installed models of the shipped list that read images, the recommended one first (else the
    /// largest first), and the id the list recommends for this machine (installed or not). Ollama
    /// not reachable: nothing is installed.
    pub async fn vision_picks(&self) -> (Vec<String>, Option<String>) {
        let (gpu, ram) = self.machine().await;
        let fits: Vec<_> = self.list.iter().map(|e| fit::fit(e.vram_mb, gpu.as_ref(), ram)).collect();
        let language = self.settings.read().unwrap().persona.language.clone();
        let recommended = fit::recommend(&self.list, &fits, &language).get(&Job::Vision).cloned();
        let installed = ollama::tags(&self.base()).await.unwrap_or_default();
        let mut have: Vec<(&super::fit::Entry, String)> = self
            .list
            .iter()
            .filter(|e| e.has(Job::Vision) && e.downloadable())
            .filter_map(|e| installed.iter().find(|i| ollama::same_id(&e.id, &i.id)).map(|i| (e, i.id.clone())))
            .collect();
        have.sort_by(|(a, _), (b, _)| (Some(&b.id) == recommended.as_ref()).cmp(&(Some(&a.id) == recommended.as_ref())).then(b.size_mb.cmp(&a.size_mb)));
        (have.into_iter().map(|(_, id)| id).collect(), recommended)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{gpu, rig};

    #[tokio::test]
    async fn the_recommended_installed_vision_model_comes_first() {
        let r = rig(gpu(12288, 10000), 64000).await;
        r.ollama.install("qwen2.5:7b-instruct", 4000 * super::super::tests::MB);
        r.ollama.install("gemma3:12b", 8000 * super::super::tests::MB);
        r.ollama.install("gemma3:4b", 3000 * super::super::tests::MB);
        let (installed, recommended) = r.lm.vision_picks().await;
        assert!(!installed.contains(&"qwen2.5:7b-instruct".to_string()), "a chat model does not read images: {installed:?}");
        assert_eq!(installed.first(), recommended.as_ref(), "the recommendation first: {installed:?} {recommended:?}");
        assert_eq!(installed.len(), 2);
    }

    #[tokio::test]
    async fn nothing_installed_is_an_empty_list_and_still_a_recommendation() {
        let r = rig(gpu(12288, 10000), 64000).await;
        let (installed, recommended) = r.lm.vision_picks().await;
        assert!(installed.is_empty());
        assert!(recommended.is_some());
    }
}
