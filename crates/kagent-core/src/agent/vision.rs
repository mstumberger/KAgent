//! Who reads the images of a message (vision.md section 2): the Brain when it can, else
//! the vision model once (its description is stored with the message), else the turn is
//! refused before it starts. Only the last 6 messages of a request carry pixels.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::state::SessionState;
use super::Agent;
use crate::model::{ModelOptions, ModelRequest};
use crate::protocol::{AgentEvent, ImageRef, Message, Pixels, Role};

/// The notice of a turn refused for its images.
pub const NO_VISION: &str = "The Brain cannot read images: pick a vision model in Settings -> Models.";
/// What the vision model is asked for each image.
pub const DESCRIBE_PROMPT: &str = "Describe this image for another model: every text in it word for word, the layout, what is selected or wrong.";
/// Only the images of this many last messages of a request go as pixels.
pub const PIXEL_MESSAGES: usize = 6;

/// Who can read an image for a Brain (`Agent::set_vision_source`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionPlan {
    /// The Brain's resolved config has `supportsImage`.
    pub brain_reads: bool,
    /// `visionModel` as a ref, when it is usable.
    pub describer: Option<String>,
}

/// Asked with the ref of the model that answers.
pub type VisionSource = Arc<dyn Fn(&str) -> VisionPlan + Send + Sync>;

/// A turn refused before it started (the notice has been sent).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Refused(pub String);

/// `p::m` as `m`: the name a description is credited to.
fn model_name(model_ref: &str) -> &str {
    model_ref.rsplit("::").next().unwrap_or(model_ref)
}

/// The line that stands for an image in the text of a message.
pub fn image_note(index: usize, image: &ImageRef) -> String {
    match (&image.description, &image.read_by) {
        (Some(text), Some(by)) => format!("[image {}, read by {}: {}]", index + 1, model_name(by), text.trim()),
        (Some(text), None) => format!("[image {}: {}]", index + 1, text.trim()),
        _ => format!("[image {}: not shown to this model]", index + 1),
    }
}

impl Agent {
    /// Where `Brain reads` and `visionModel` are asked of. Unset: the Brain reads every image.
    pub fn set_vision_source(&self, source: VisionSource) {
        *self.vision.lock().unwrap() = Some(source);
    }

    pub(crate) fn vision_plan(&self, brain: &str) -> VisionPlan {
        match self.vision.lock().unwrap().clone() {
            Some(ask) => ask(brain),
            None => VisionPlan { brain_reads: true, describer: None },
        }
    }

    /// The model that answers a turn of the session now: its own choice, else the default.
    async fn brain_of(&self, state: &SessionState) -> String {
        match state.meta.lock().await.model.clone().filter(|m| !m.trim().is_empty()) {
            Some(choice) => choice,
            None => self.config().model_ref,
        }
    }

    /// Refuse, before the turn starts, images nobody can read: the Brain cannot and there
    /// is no vision model (a description made before does not need one).
    pub(super) async fn check_images(&self, session_id: &str, state: &SessionState, images: &[ImageRef]) -> Result<(), Refused> {
        if images.is_empty() {
            return Ok(());
        }
        let plan = self.vision_plan(&self.brain_of(state).await);
        if plan.brain_reads || plan.describer.is_some() || images.iter().all(|i| i.description.is_some()) {
            return Ok(());
        }
        let _ = self.events.send(AgentEvent::notice("warn", NO_VISION).in_session(session_id));
        Err(Refused(NO_VISION.into()))
    }

    /// The images of a message about to be stored: when the Brain cannot read them, each
    /// gets its description from the vision model, once.
    pub(super) async fn describe_for_brain(&self, session_id: &str, brain: &str, mut images: Vec<ImageRef>, cancel: &CancellationToken) -> anyhow::Result<Vec<ImageRef>> {
        let plan = self.vision_plan(brain);
        if plan.brain_reads {
            return Ok(images);
        }
        for image in images.iter_mut().filter(|i| i.description.is_none()) {
            let Some(describer) = plan.describer.as_deref() else { anyhow::bail!(NO_VISION) };
            image.description = Some(self.describe(session_id, image, describer, cancel).await?);
            image.read_by = Some(describer.to_string());
        }
        Ok(images)
    }

    /// Ask the vision model about one image.
    async fn describe(&self, session_id: &str, image: &ImageRef, describer: &str, cancel: &CancellationToken) -> anyhow::Result<String> {
        let key = format!("{session_id}:{}", image.id);
        if let Some(text) = self.described.lock().unwrap().get(&key) {
            return Ok(text.clone());
        }
        let (bytes, _) = self.store.image_file(session_id, &image.id).await?;
        let mut shown = image.clone();
        shown.data = Some(Pixels(Arc::new(bytes)));
        let request = ModelRequest {
            model_ref: describer.to_string(),
            system: String::new(),
            messages: vec![Message::user(DESCRIBE_PROMPT).with_images(vec![shown])],
            tools: vec![],
            options: ModelOptions { max_output_tokens: Some(1_200), ..Default::default() },
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let answer = self.model.complete(request, tx, cancel.clone()).await.map_err(|e| anyhow::anyhow!("the vision model {describer} could not read the image: {e:#}"))?;
        let text = answer.text.trim().to_string();
        anyhow::ensure!(!text.is_empty(), "the vision model {describer} returned no description: try again, or pick another vision model in Settings -> Models");
        self.described.lock().unwrap().insert(key, text.clone());
        Ok(text)
    }

    /// The messages of a request for `brain`: the images of the last 6 messages as pixels
    /// when the Brain reads them, every other image as the line of its description.
    pub(crate) async fn images_for_request(&self, session_id: &str, messages: &mut [Message], brain: &str, cancel: &CancellationToken) {
        let plan = self.vision_plan(brain);
        let first_recent = messages.len().saturating_sub(PIXEL_MESSAGES);
        for (i, message) in messages.iter_mut().enumerate() {
            if message.images.is_empty() {
                continue;
            }
            if plan.brain_reads && i >= first_recent {
                for image in &mut message.images {
                    match self.store.image_file(session_id, &image.id).await {
                        Ok((bytes, _)) => image.data = Some(Pixels(Arc::new(bytes))),
                        Err(e) => tracing::warn!(session_id, "image {} not sent: {e}", image.id),
                    }
                }
                message.images.retain(|i| i.data.is_some());
            } else {
                self.into_text(session_id, message, plan.describer.as_deref(), cancel).await;
            }
        }
    }

    /// Replace the images of the messages by the lines of their descriptions (compaction.md,
    /// vision.md section 2): made now when there is none and a vision model exists.
    pub(crate) async fn imageless(&self, session_id: &str, messages: &mut [Message], brain: &str, cancel: &CancellationToken) {
        let describer = self.vision_plan(brain).describer;
        for message in messages.iter_mut().filter(|m| m.role == Role::User && !m.images.is_empty()) {
            self.into_text(session_id, message, describer.as_deref(), cancel).await;
        }
    }

    async fn into_text(&self, session_id: &str, message: &mut Message, describer: Option<&str>, cancel: &CancellationToken) {
        let mut notes = Vec::new();
        for (n, image) in message.images.iter_mut().enumerate() {
            if image.description.is_none() {
                if let Some(model) = describer {
                    match self.describe(session_id, image, model, cancel).await {
                        Ok(text) => (image.description, image.read_by) = (Some(text), Some(model.to_string())),
                        Err(e) => tracing::warn!(session_id, "image {} not described: {e:#}", image.id),
                    }
                }
            }
            notes.push(image_note(n, image));
        }
        message.images.clear();
        if !message.content.is_empty() {
            message.content.push_str("\n\n");
        }
        message.content.push_str(&notes.join("\n"));
    }
}
