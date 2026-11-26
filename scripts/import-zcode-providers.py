#!/usr/bin/env python3
"""Convert ZCode's provider templates + model rules into KAgent's providers.builtin.json.

Reads:  /home/virus/SystemInterface/ZCode/config/provider/zcode-builtin.json
Writes: config/providers.builtin.json (repo root next to this script's parent).

Mapping (docs/specs/config.md section 3):
  modelRules         -> {match: {model}}
  modelApiRules      -> {match: {model, apiType}}
  providerSiteRules  -> {match: {model, baseUrl, apiType?}}
  templateModelRules -> {match: {templateId, model}}
Config flattening: contextWindow, supportsImage, supportsToolCall, reasoningLevels,
maxOutputTokens; `map` expressions dropped except plain dict literals whose values
contain a bare `reasoningLevel`, which render one patch per level into
reasoningPatches. Templates whose access type is not a plain API key
(`zhipu-account`-style) are skipped, as are builtinProviderModelRules.
"""

import json
import pathlib
import sys

ZCODE_FILE = "/home/virus/SystemInterface/ZCode/config/provider/zcode-builtin.json"

EXTRA_TEMPLATES = [
    {
        "id": "gemini",
        "name": "Google Gemini",
        "api": {"type": "openai-chat-completions",
                "baseUrl": "https://generativelanguage.googleapis.com/v1beta/openai",
                "headers": {}},
        "keyUrl": "https://aistudio.google.com/app/apikey",
        "models": ["gemini-3.8-pro", "gemini-3.8-flash"],
        # The second thinks longer before it speaks (voice.md section 8).
        "liveModels": ["gemini-3.8-live", "gemini-3.8-live-extended-thinking"],
        "voices": ["Kore", "Puck", "Charon", "Zephyr", "Fenrir", "Leda", "Orus", "Aoede",
                   "Callirrhoe", "Autonoe", "Enceladus", "Iapetus", "Umbriel", "Algieba",
                   "Despina", "Erinome", "Algenib", "Rasalgethi", "Laomedeia", "Achernar",
                   "Alnilam", "Schedar", "Gacrux", "Pulcherrima", "Achird"],
    },
    {
        "id": "ollama",
        "name": "Ollama (local)",
        # Native API: the OpenAI-compatible endpoint ignores num_ctx (see adapters/ollama.rs).
        "api": {"type": "ollama-native", "baseUrl": "http://127.0.0.1:11434",
                "headers": {}},
        "keyUrl": "",
        "models": [],
        "liveModels": [],
        "voices": [],
    },
    {
        "id": "lmstudio",
        "name": "LM Studio (local)",
        "api": {"type": "openai-chat-completions", "baseUrl": "http://127.0.0.1:1234/v1",
                "headers": {}},
        "keyUrl": "",
        "models": [],
        "liveModels": [],
        "voices": [],
    },
    {
        "id": "groq",
        "name": "Groq",
        "api": {"type": "openai-chat-completions", "baseUrl": "https://api.groq.com/openai/v1",
                "headers": {}},
        "keyUrl": "https://console.groq.com/keys",
        "models": [],
        "liveModels": [],
        "voices": [],
    },
    {
        "id": "mistral",
        "name": "Mistral",
        "api": {"type": "openai-chat-completions", "baseUrl": "https://api.mistral.ai/v1",
                "headers": {}},
        "keyUrl": "https://console.mistral.ai/api-keys",
        "models": [],
        "liveModels": [],
        "voices": [],
    },
    {
        # The owner names the address; name and baseUrl are required when it is added.
        "id": "custom",
        "name": "Custom (OpenAI-compatible)",
        "api": {"type": "openai-chat-completions", "baseUrl": "",
                "headers": {}},
        "keyUrl": "",
        "models": [],
        "liveModels": [],
        "voices": [],
    },
    {
        "id": "nvidia",
        "name": "NVIDIA",
        "api": {"type": "openai-chat-completions",
                "baseUrl": "https://integrate.api.nvidia.com/v1",
                "headers": {}},
        "keyUrl": "https://build.nvidia.com/settings/api-keys",
        "models": ["poolside/laguna-xs-2.1", "nvidia/nemotron-3.5-lightning-30b-a3b",
                   "nvidia/nemotron-3.5-super-vl-120b-a12b"],
        "liveModels": [],
        "voices": [],
    },
]

NVIDIA_BASE = r"https://integrate\.api\.nvidia\.com/v1/?"

# The `nvidia` extra: what ZCode holds for these two models (the catch-all rule gives
# every other NVIDIA model the 1M window). Output caps are not taken over: ZCode's is
# the top of a range the owner picks from, KAgent's is what the request sends.
EXTRA_RULES = [
    {"match": {"model": r"poolside/laguna-xs-2\.1", "apiType": "openai-chat-completions",
               "baseUrl": NVIDIA_BASE},
     "config": {"contextWindow": 262144}},
    {"match": {"model": r"nvidia/nemotron-3\.5-super-vl-120b-a12b",
               "apiType": "openai-chat-completions", "baseUrl": NVIDIA_BASE},
     "config": {"supportsImage": True}},
]


def flatten_config(cfg):
    """ZCode config -> flat KAgent ModelConfig patch."""
    out = {}
    props = cfg.get("properties", {})
    if "contextWindow" in props:
        out["contextWindow"] = props["contextWindow"]
    if props.get("inputFormat", {}).get("supportsImage"):
        out["supportsImage"] = True
    if "supportsToolCall" in props:
        out["supportsToolCall"] = props["supportsToolCall"]
    if "enabled" in cfg:
        out["enabled"] = cfg["enabled"]
    specs = cfg.get("optionSpecs", {})
    rl = specs.get("reasoningLevel", {})
    values = rl.get("values")
    if values:
        out["reasoningLevels"] = values
        patches = render_reasoning_patches(rl.get("map"), values)
        if patches:
            out["reasoningPatches"] = patches
    mot = specs.get("maxOutputTokens", {})
    if "max" in mot:
        out["maxOutputTokens"] = mot["max"]
    return out


def render_reasoning_patches(map_expr, values):
    """Plain dict literal with a bare reasoningLevel value -> one patch per level.

    Substituting the identifier with the JSON-encoded level and re-parsing only
    succeeds for trivial maps; ternaries and comparisons fail json.loads and are
    skipped (KAgent then uses its per-API default reasoning mapping)."""
    if not map_expr or map_expr == "{}":
        return {}
    patches = {}
    for level in values:
        rendered = map_expr.replace("reasoningLevel", json.dumps(level))
        try:
            patches[level] = json.loads(rendered)
        except (json.JSONDecodeError, ValueError):
            return {}  # not a plain literal; skip the whole map
    return patches


def main():
    src = json.load(open(ZCODE_FILE))
    templates = []
    for t in src["config"]["providerConfigRules"]["templateRules"]:
        cfg = t.get("config", {})
        access = cfg.get("access", {}).get("type", "")
        if access.startswith("zhipu-"):
            # account/coding-plan style access KAgent does not implement
            continue
        name = t.get("templateNameMap", {}).get("en-US") or t["templateId"]
        templates.append({
            "id": t["templateId"],
            "name": name,
            "api": {
                "type": cfg.get("api", {}).get("type", "openai-chat-completions"),
                "baseUrl": cfg.get("api", {}).get("baseUrl", ""),
                "headers": {},
            },
            "keyUrl": cfg.get("access", {}).get("apiKeyManagementUrl", ""),
            "models": cfg.get("builtinModelIds", []),
            "liveModels": [],
            "voices": [],
        })
    templates.extend(EXTRA_TEMPLATES)

    mcr = src["config"].get("modelConfigRules", {})
    rules = []

    def add(rules_list, key, match):
        for r in rules_list:
            flat = flatten_config(r.get("config", {}))
            if not flat:
                continue
            if key == "template":
                m = {"templateId": r["templateId"], "model": r["modelId"]}
            else:
                m = {"model": r["modelMatch"]}
                if key == "api":
                    m["apiType"] = r.get("apiTypeMatch")
                elif key == "site":
                    m["baseUrl"] = r.get("baseUrlMatch")
                    if r.get("apiTypeMatch"):
                        m["apiType"] = r["apiTypeMatch"]
            rules.append({"match": m, "config": flat})

    add(mcr.get("modelRules", []), "model", None)
    add(mcr.get("modelApiRules", []), "api", None)
    add(mcr.get("providerSiteRules", []), "site", None)
    add(mcr.get("templateModelRules", []), "template", None)

    # Local servers: ZCode's catch-all rule says 1M tokens, but Ollama / LM Studio serve
    # whatever context they were started with and silently drop the start of longer
    # prompts (including the system prompt). Assume a conservative window; raise it per
    # model in providers.json after configuring the server (e.g. OLLAMA_CONTEXT_LENGTH).
    # Measured on Ollama 0.40: the OpenAI endpoint runs with the server default
    # (4096) and keeps only ~2k tokens of longer prompts.
    for base in (r"https?://(127\.0\.0\.1|localhost):11434(/v1)?/?", r"https?://(127\.0\.0\.1|localhost):1234(/v1)?/?"):
        rules.append({"match": {"model": ".*", "apiType": "openai-chat-completions", "baseUrl": base}, "config": {"contextWindow": 4096, "maxOutputTokens": 1024}})
    # Native Ollama: KAgent sends num_ctx itself; 16k fits a 7-8B model in 12 GB VRAM.
    rules.append({"match": {"model": ".*", "apiType": "ollama-native"}, "config": {"contextWindow": 16384, "maxOutputTokens": 4096, "supportsToolCall": True}})
    rules.extend(EXTRA_RULES)

    out = {
        "schemaVersion": 1,
        "generatedFrom": f"ZCode zcode-builtin.json revision {src.get('revision', '?')}",
        "templates": templates,
        "modelRules": rules,
    }
    dest = pathlib.Path(__file__).resolve().parent.parent / "config" / "providers.builtin.json"
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(f"wrote {dest}: {len(templates)} templates, {len(rules)} model rules")


if __name__ == "__main__":
    sys.exit(main())
