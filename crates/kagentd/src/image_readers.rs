//! Who reads the images of a message (vision.md section 2), from the settings and the
//! providers: `supportsImage` of the Brain, and `visionModel` when it can be used.

use std::sync::{Arc, RwLock};

use kagent_core::agent::vision::{VisionPlan, VisionSource};
use kagent_providers::ProviderStore;
use serde_json::Value;

use crate::settings::Settings;

/// The plan for `brain`: it reads images when its resolved config says so; `vision_model`
/// describes them once when it is a model that can be used.
pub fn plan(providers: &ProviderStore, vision_model: &str, brain: &str) -> VisionPlan {
    VisionPlan {
        brain_reads: providers.usable(brain).is_some_and(|r| r.config.supports_image),
        describer: providers.usable(vision_model).map(|_| vision_model.to_string()),
    }
}

/// What `Agent::set_vision_source` is given: the settings are read on every question.
pub fn source(settings: Arc<RwLock<Settings>>, providers: Arc<ProviderStore>) -> VisionSource {
    Arc::new(move |brain| {
        let vision = settings.read().unwrap().extra.get("visionModel").and_then(Value::as_str).unwrap_or("").to_string();
        plan(&providers, &vision, brain)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_brain_reads_with_supports_image_and_the_vision_model_must_be_usable() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProviderStore::load(dir.path().join("providers.json"), ProviderStore::builtin_catalog().unwrap(), &[]).unwrap();
        assert!(crate::api::add_provider_to_store(&store, &json!({"templateId": "nvidia"})).is_ok());
        let models: Vec<(String, bool)> = store.enabled_models().into_iter().map(|(p, m, c)| (format!("{p}::{}", m.id), c.supports_image)).collect();
        let reads = models.iter().find(|(_, img)| *img).expect("nvidia has a model that reads images").0.clone();
        let blind = models.iter().find(|(_, img)| !*img).expect("and one that does not").0.clone();

        assert_eq!(plan(&store, "", &reads), VisionPlan { brain_reads: true, describer: None }, "no vision model set");
        assert_eq!(plan(&store, &reads, &blind), VisionPlan { brain_reads: false, describer: Some(reads.clone()) }, "the Brain cannot, the vision model describes");
        assert_eq!(plan(&store, "nvidia::nope", &blind), VisionPlan { brain_reads: false, describer: None }, "a vision model that cannot be used is none");
        assert_eq!(plan(&store, "", "nvidia::nope"), VisionPlan { brain_reads: false, describer: None }, "an unusable Brain reads nothing");
    }
}
