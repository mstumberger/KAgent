//! Whether a provider is on this machine or this network, and whether a model is free (chooser.md section 2).

/// "Local" means the model's provider is on this machine or this network: the address of its base URL is a
/// loopback, private (10/8, 172.16/12, 192.168/16, fc00::/7) or link-local IP literal, or the name
/// `localhost`. A host name that needs a lookup (`box.lan`, `ollama.com`) is not decided here and counts as NOT local:
/// a frame is never sent to a name nobody checked. The API type does not decide: Ollama can run on a server
/// the owner does not own. This one function is the whole rule.
pub fn is_local_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else { return false };
    let Some(host) = url.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80,
        Err(_) => host == "localhost" || host.ends_with(".localhost"),
    }
}

/// A model is free when its id ends in `-free` (chooser.md section 2); no provider reports a price yet.
pub fn is_free_id(id: &str) -> bool {
    id.trim().to_ascii_lowercase().ends_with("-free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_is_the_end_of_the_id() {
        assert!(is_free_id("minimax-m2.5-free") && is_free_id("Big-Pickle-FREE"));
        assert!(!is_free_id("freedom-1") && !is_free_id("gemini-flash"));
    }
}
