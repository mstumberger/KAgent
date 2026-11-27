//! Places and travel (calendar.md section 5). With a Maps key: Places API (New) `searchText`
//! and Routes API `computeRoutes`. Without one, or when Places fails, the web search finds
//! an address and the link is a Maps search (`source: "web"`). The key travels in a header,
//! never in a URL, and no error or log line holds it.

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

use super::oauth::{scrub, urlencode};
use super::{state, CalError, Calendar};

/// The fields Places answers with (and nothing else is paid for).
pub const PLACES_MASK: &str = "places.displayName,places.formattedAddress,places.location,places.googleMapsUri";
pub const ROUTES_MASK: &str = "routes.duration,routes.staticDuration";
/// How far from the bias point a result may be.
const BIAS_RADIUS_M: f64 = 20_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Places,
    Web,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::Places => "places",
            Self::Web => "web",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub address: String,
    pub maps_url: String,
    pub source: Source,
    pub lat_lng: Option<[f64; 2]>,
}

/// The two durations of a drive, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drive {
    pub no_traffic: i64,
    pub with_traffic: i64,
}

/// `https://www.google.com/maps/search/?api=1&query=<name, address>`.
pub fn search_link(name: &str, address: &str) -> String {
    let text = if address.is_empty() { name.to_string() } else { format!("{name}, {address}") };
    format!("https://www.google.com/maps/search/?api=1&query={}", urlencode(&text))
}

/// "1500s" as seconds.
fn seconds(v: &Value) -> Option<i64> {
    v.as_str()?.strip_suffix('s')?.parse::<f64>().ok().map(|s| s.round() as i64)
}

impl Calendar {
    pub async fn maps_key(&self) -> Option<String> {
        if let Some(key) = self.maps_cache.lock().unwrap().clone() {
            return Some(key);
        }
        let key = self.maps.get().await.map_err(|e| tracing::warn!("calendar: the Maps key could not be read: {e:#}")).ok().flatten()?;
        *self.maps_cache.lock().unwrap() = Some(key.clone());
        Some(key)
    }

    pub async fn has_maps_key(&self) -> bool {
        self.maps_key().await.is_some()
    }

    /// `PUT /api/calendar/maps-key`: `None` forgets it.
    pub async fn set_maps_key(&self, key: Option<&str>) -> Result<(), CalError> {
        match key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(key) => {
                crate::web::settings::check_key(key).map_err(CalError::Setup)?;
                self.maps.set(key).await.map_err(|e| CalError::Upstream(format!("the Maps key could not be stored: {e:#}")))?;
                *self.maps_cache.lock().unwrap() = Some(key.to_string());
            }
            None => {
                self.maps.clear().await.map_err(|e| CalError::Upstream(format!("the Maps key could not be deleted: {e:#}")))?;
                *self.maps_cache.lock().unwrap() = None;
            }
        }
        // Another key, another answer for `home`.
        let mut saved = self.saved.lock().unwrap();
        saved.home_for.clear();
        saved.home_lat_lng = None;
        state::save(&self.data_dir, &saved);
        Ok(())
    }

    /// One call to Places or Routes: the key in a header, the answer as JSON.
    async fn maps_call(&self, key: &str, url: String, mask: &str, body: &Value) -> Result<Value, CalError> {
        let response = self
            .http
            .post(url)
            .header("X-Goog-Api-Key", key)
            .header("X-Goog-FieldMask", mask)
            .json(body)
            .send()
            .await
            .map_err(|e| CalError::Upstream(format!("Google Maps did not answer: {}", scrub(&e.without_url().to_string(), &[key]))))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.is_success() {
            return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
        }
        let said = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(String::from)).unwrap_or_default();
        let said = scrub(&said, &[key]);
        Err(CalError::Upstream(match status.as_u16() {
            400 | 403 => format!("Google Maps refused the key or the request ({status}): {said}. The owner checks the Maps key and that Places API (New) and Routes API are enabled for it"),
            429 => "Google Maps says too many requests: try again in a minute".into(),
            s => format!("Google Maps answered {s}: {said}"),
        }))
    }

    fn places_url(&self) -> String {
        format!("{}/v1/places:searchText", self.hosts.places)
    }

    /// Where the search is centred: `near` (looked up once), else `home`.
    async fn bias(&self, key: &str, near: Option<&str>) -> Option<[f64; 2]> {
        match near.map(str::trim).filter(|n| !n.is_empty()) {
            Some(near) => {
                let body = json!({"textQuery": near, "maxResultCount": 1});
                let answer = self.maps_call(key, self.places_url(), PLACES_MASK, &body).await.ok()?;
                lat_lng(answer["places"].as_array()?.first()?)
            }
            None => self.home_lat_lng().await,
        }
    }

    pub async fn places_search(&self, key: &str, query: &str, near: Option<&str>) -> Result<Vec<Candidate>, CalError> {
        let mut body = json!({"textQuery": query, "maxResultCount": 3});
        if let Some([lat, lng]) = self.bias(key, near).await {
            body["locationBias"] = json!({"circle": {"center": {"latitude": lat, "longitude": lng}, "radius": BIAS_RADIUS_M}});
        }
        let answer = self.maps_call(key, self.places_url(), PLACES_MASK, &body).await?;
        Ok(answer["places"]
            .as_array()
            .into_iter()
            .flatten()
            .take(3)
            .map(|p| {
                let name = p["displayName"]["text"].as_str().unwrap_or(query).to_string();
                let address = p["formattedAddress"].as_str().unwrap_or("").to_string();
                let maps_url = p["googleMapsUri"].as_str().map(String::from).unwrap_or_else(|| search_link(&name, &address));
                Candidate { name, address, maps_url, source: Source::Places, lat_lng: lat_lng(p) }
            })
            .collect())
    }

    /// The candidates for a place, and a note when they are not from the map.
    pub async fn find_place(&self, query: &str, near: Option<&str>) -> Result<(Vec<Candidate>, Option<String>), CalError> {
        let mut note = None;
        if let Some(key) = self.maps_key().await {
            match self.places_search(&key, query, near).await {
                Ok(found) if !found.is_empty() => return Ok((found, None)),
                Ok(_) => note = Some("Google Maps found nothing for this".to_string()),
                Err(e) => {
                    tracing::warn!("calendar: the Places lookup failed, the web is asked instead: {e}");
                    note = Some(e.to_string());
                }
            }
        }
        match self.web_places(query, near).await {
            Ok(found) => Ok((found, note)),
            Err(why) => Err(CalError::Upstream(match note {
                Some(n) => format!("no place was found: {n}; and the web search failed too ({why})"),
                None => format!("no place was found: no Maps key is set, and the web search failed ({why})"),
            })),
        }
    }

    /// The web as the fallback: the address from the first result that has one.
    async fn web_places(&self, query: &str, near: Option<&str>) -> Result<Vec<Candidate>, String> {
        let web = self.web.clone().ok_or("web search is not available")?;
        let asked = match near.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => format!("{query} {n} address"),
            None => format!("{query} address"),
        };
        let (_, hits) = web.search_hits(&asked, 6).await?;
        let mut out: Vec<Candidate> = Vec::new();
        for hit in hits {
            let Some(address) = address_in(&hit.snippet).or_else(|| address_in(&hit.title)) else { continue };
            if out.iter().any(|c| c.address == address) {
                continue;
            }
            let name = hit.title.split([' ', '\u{a0}']).collect::<Vec<_>>().join(" ");
            let cut = [" - ", " | ", " – "].iter().filter_map(|sep| name.find(sep)).min().unwrap_or(name.len());
            let name = name[..cut].trim().to_string();
            let name = if name.is_empty() { query.to_string() } else { name };
            out.push(Candidate { maps_url: search_link(&name, &address), name, address, source: Source::Web, lat_lng: None });
            if out.len() == 3 {
                break;
            }
        }
        if out.is_empty() {
            return Err(format!("no result of the web search holds an address for \"{query}\""));
        }
        Ok(out)
    }

    /// The coordinates of `home`: resolved once per text and key (calendar.md section 2).
    pub async fn home_lat_lng(&self) -> Option<[f64; 2]> {
        let home = self.config().home;
        if home.trim().is_empty() {
            return None;
        }
        {
            let saved = self.saved.lock().unwrap();
            if saved.home_for == home {
                return saved.home_lat_lng;
            }
        }
        let key = self.maps_key().await?;
        let body = json!({"textQuery": home, "maxResultCount": 1});
        let found = self.maps_call(&key, self.places_url(), PLACES_MASK, &body).await.ok().and_then(|a| lat_lng(a["places"].as_array()?.first()?));
        // A failed lookup is asked again next time; a place that does not exist is remembered.
        if found.is_some() {
            let mut saved = self.saved.lock().unwrap();
            saved.home_for = home;
            saved.home_lat_lng = found;
            state::save(&self.data_dir, &saved);
        }
        found
    }

    /// The drive from `from` to `to` that arrives at `arrive`: the no-traffic duration first,
    /// then the duration with traffic for a departure that far before it.
    pub async fn drive(&self, from: [f64; 2], to: [f64; 2], arrive: DateTime<Utc>) -> Result<Drive, CalError> {
        let key = self.maps_key().await.ok_or_else(|| CalError::Setup("travel times need a Google Maps key (Settings -> Calendar)".into()))?;
        let url = format!("{}/directions/v2:computeRoutes", self.hosts.routes);
        let point = |p: [f64; 2]| json!({"location": {"latLng": {"latitude": p[0], "longitude": p[1]}}});
        let mut body = json!({"origin": point(from), "destination": point(to), "travelMode": "DRIVE", "routingPreference": "TRAFFIC_AWARE"});
        let first = self.maps_call(&key, url.clone(), ROUTES_MASK, &body).await?;
        let route = first["routes"].as_array().and_then(|r| r.first()).ok_or_else(|| CalError::Upstream("Google Maps found no route".into()))?;
        let no_traffic = seconds(&route["staticDuration"]).or_else(|| seconds(&route["duration"])).ok_or_else(|| CalError::Upstream("Google Maps gave no duration".into()))?;
        let leave = (arrive - Duration::seconds(no_traffic)).max(self.now() + Duration::seconds(30));
        body["departureTime"] = json!(leave.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        let second = self.maps_call(&key, url, ROUTES_MASK, &body).await?;
        let with_traffic = second["routes"].as_array().and_then(|r| r.first()).and_then(|r| seconds(&r["duration"])).unwrap_or(no_traffic);
        Ok(Drive { no_traffic, with_traffic })
    }
}

fn lat_lng(place: &Value) -> Option<[f64; 2]> {
    Some([place["location"]["latitude"].as_f64()?, place["location"]["longitude"].as_f64()?])
}

/// The first thing in a text that reads as a street address: `Glavni trg 5, 2000 Maribor`.
pub fn address_in(text: &str) -> Option<String> {
    static ADDRESS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\p{Lu}[\p{L}.'\-]*(?: \p{L}[\p{L}.'\-]*){0,3} \d{1,4}[a-zA-Z]?(?:/\d+)?,? (?:\d{4,5} )?\p{Lu}[\p{L}\-]+(?: \p{Lu}[\p{L}\-]+)?").unwrap()
    });
    ADDRESS.find(text).map(|m| m.as_str().replacen(" ,", ",", 1).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_found_in_the_text_of_a_result() {
        assert_eq!(address_in("Thai Orchid - Glavni trg 5, 2000 Maribor. Open daily 11-22").as_deref(), Some("Glavni trg 5, 2000 Maribor"));
        assert_eq!(address_in("Visit us at Cankarjeva ulica 12 Ljubljana for lunch").as_deref(), Some("Cankarjeva ulica 12 Ljubljana"));
        assert_eq!(address_in("Restavracija Šestica, Šubičeva ulica 3, Ljubljana").as_deref(), Some("Šubičeva ulica 3, Ljubljana"));
        assert_eq!(address_in("The best thai food in town, open daily"), None);
        assert_eq!(address_in("Since 1999 we serve 24 dishes"), None);
    }

    #[test]
    fn a_maps_search_link_carries_the_name_and_the_address() {
        assert_eq!(search_link("Thai Orchid", "Glavni trg 5, Maribor"), "https://www.google.com/maps/search/?api=1&query=Thai%20Orchid%2C%20Glavni%20trg%205%2C%20Maribor");
        assert_eq!(search_link("Home", ""), "https://www.google.com/maps/search/?api=1&query=Home");
    }

    #[test]
    fn durations_are_read_in_seconds() {
        assert_eq!(seconds(&json!("1500s")), Some(1500));
        assert_eq!(seconds(&json!("1500.4s")), Some(1500));
        assert_eq!(seconds(&json!(1500)), None);
    }
}
