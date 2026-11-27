//! `place_search` (calendar.md section 3): up to three candidates for a place, with the
//! address and a Maps link, from Google Places or, without it, from the web.

use async_trait::async_trait;
use kagent_core::tool::{CallAssessment, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use serde_json::{json, Value};

use super::places::Source;
use super::tools::{failed, ready, CalendarCell, UNTRUSTED};

pub struct PlaceSearch {
    contract: ToolContract,
    cell: CalendarCell,
}

impl PlaceSearch {
    pub fn new(cell: CalendarCell) -> Self {
        let mut contract = ToolContract::read_only(
            "place_search",
            "Find a place (a restaurant, an office, an address) for a calendar event: up to 3 candidates with name, address and a Maps link. Read the best one back to the owner ('Thai Orchid, Glavni trg 5, Maribor: that one?') and use its name, address and mapsUrl as the location of calendar_change. near: a city or address to look around.",
            json!({"type":"object","required":["query"],"properties":{
                "query":{"type":"string","description":"what the owner said: a name, or a name and a town"},
                "near":{"type":"string","description":"a town or address to search around (default: the owner's home)"}
            }}),
        );
        contract.side_effect = SideEffect::Network;
        Self { contract: contract.timeout(30_000, 60_000), cell }
    }
}

#[async_trait]
impl Tool for PlaceSearch {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let query = input.get("query").and_then(Value::as_str).unwrap_or("");
        CallAssessment::from_contract(&self.contract, format!("place search {}", kagent_core::util::truncate_str(query, 80)), "place_search".to_string())
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let cal = match ready(&self.cell) {
            Ok(cal) => cal,
            Err(out) => return Ok(out),
        };
        let query = input.get("query").and_then(Value::as_str).map(str::trim).filter(|q| !q.is_empty());
        let Some(query) = query else { return Ok(ToolOutput::error("query required: the name of the place, and the town if the owner said it")) };
        if query.chars().count() > 200 {
            return Ok(ToolOutput::error("query is longer than 200 characters"));
        }
        let near = input.get("near").and_then(Value::as_str);
        let (found, note) = match cal.find_place(query, near).await {
            Ok(r) => r,
            Err(e) => return Ok(failed(e)),
        };
        let web = found.first().is_some_and(|c| c.source == Source::Web);
        let mut out = format!("{UNTRUSTED}\nCandidates for \"{query}\"{} (source: {}):\n", near.map(|n| format!(" near {n}")).unwrap_or_default(), if web { "web search" } else { "Google Places" });
        for (n, c) in found.iter().enumerate() {
            out.push_str(&format!("{}. {} | address: {} | mapsUrl {} | source {}\n", n + 1, c.name, if c.address.is_empty() { "(none)" } else { &c.address }, c.maps_url, c.source.name()));
        }
        if web {
            out.push_str("I could not check this on the map: these come from a web search, so say that when you read the address back.\n");
            if let Some(n) = note {
                out.push_str(&format!("(Google Maps: {n})\n"));
            }
        }
        out.push_str("Read the best candidate back to the owner and ask if it is the right place; with several plausible ones, name up to three.");
        Ok(ToolOutput::ok(out))
    }
}
