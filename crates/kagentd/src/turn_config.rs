//! The configuration of one turn (`AgentConfig`), built for every turn from the
//! settings: persona, language, enabled tools, clock, memory and the chat model's
//! context window are always current.

use std::sync::{Arc, RwLock};

use kagent_core::agent::{AgentConfig, TurnContext};

use crate::{memory, prompt, settings, skills};

pub struct TurnConfig {
    pub settings: Arc<RwLock<settings::Settings>>,
    pub providers: Arc<kagent_providers::ProviderStore>,
    pub machine: serde_json::Value,
    pub skills: Arc<skills::SkillHub>,
    pub memory: Arc<memory::MemoryHub>,
    /// (name, group) of every registered tool.
    pub tool_index: Vec<(String, &'static str)>,
}

impl TurnConfig {
    pub fn build(&self, turn: TurnContext) -> AgentConfig {
        let s = self.settings.read().unwrap();
        self.skills.refresh(&s.skills_config()); // first: new or edited skills are in this prompt
        let enabled: Vec<String> = self
            .tool_index
            .iter()
            .filter(|(_, g)| s.tool_groups.is_empty() || s.tool_groups.iter().any(|x| x == g))
            .map(|(n, _)| n.clone())
            .collect();
        let memory_tools = enabled.iter().any(|n| kagent_core::tools::group_of(n) == "memory");
        let mode = serde_json::to_value(s.permission_mode).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        // Usable only: enabled, with the chat role, in a provider that is on (models.md section 2).
        let resolved = self.providers.usable(&s.chat_model);
        let context_tokens = resolved.as_ref().map(|r| r.config.context_window as usize).unwrap_or(0);
        // The memory block (memory.md section 2), recalled by the words of this turn's message.
        let (memory_block, memory_rule) = self.memory.prompt_parts(&s.memory_config(), &turn.user_text, context_tokens, memory_tools);
        let system_prompt = prompt::build(&prompt::PromptInput {
            persona_name: &s.persona.name,
            address: &s.persona.address,
            language: &s.persona.language,
            machine: &self.machine,
            tool_names: &enabled,
            skills_index: &self.skills.index(s.skills_config().index_budget_chars),
            memory_block: &memory_block,
            memory_rule,
            permission_mode: &mode,
            now: chrono::Local::now(),
        });
        // The model's own output limit is the only source: `options.maxOutputTokens` is no longer read (models.md section 6).
        let fallback = self.providers.usable(&s.fallback_model);
        AgentConfig {
            model_ref: s.chat_model.clone(),
            system_prompt,
            max_iterations: s.max_iterations,
            options: kagent_core::model::ModelOptions {
                reasoning_level: s.options.reasoning_level.clone(),
                max_output_tokens: resolved.as_ref().map(|r| r.config.max_output_tokens),
                temperature: s.options.temperature,
            },
            privilege: s.privilege_method(),
            context_tokens,
            // A fallback that cannot be used is none: the turn then fails with the model's own error (models.md section 5).
            fallback_context_tokens: fallback.as_ref().map(|r| r.config.context_window as usize).unwrap_or(0),
            fallback_max_output: fallback.as_ref().map(|r| r.config.max_output_tokens),
            fallback_model_ref: fallback.as_ref().map(|_| s.fallback_model.clone()).unwrap_or_default(),
        }
    }
}
