//! The graphics card and its memory (docs/specs/local-models.md section 1): NVIDIA through
//! `nvidia-smi`, AMD through the sysfs files of the amdgpu driver, and "no GPU" as `None`.
//! The parsers are pure and tested on recorded output; the probe only reads and calls them.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    pub name: String,
    pub vram_total_mb: u64,
    pub vram_free_mb: u64,
    /// `nvidia-smi` or `sysfs`.
    pub source: String,
}

/// The arguments of `nvidia-smi` that answer section 1 (one line per card).
pub const NVIDIA_ARGS: [&str; 2] = ["--query-gpu=name,memory.total,memory.used", "--format=csv,noheader,nounits"];

/// `nvidia-smi --query-gpu=name,memory.total,memory.used --format=csv,noheader,nounits`.
/// A card that does not report its memory (`N/A`, `[N/A]`) is skipped; several cards give
/// the one with the most memory.
pub fn parse_nvidia_smi(out: &str) -> Option<Gpu> {
    let mut best: Option<Gpu> = None;
    for line in out.lines() {
        let parts: Vec<&str> = line.rsplitn(3, ',').collect();
        if parts.len() != 3 {
            continue;
        }
        let (used, total, name) = (parts[0].trim(), parts[1].trim(), parts[2].trim());
        let (Ok(total), Ok(used)) = (total.parse::<u64>(), used.parse::<u64>()) else { continue };
        if name.is_empty() || total == 0 {
            continue;
        }
        let gpu = Gpu { name: name.to_string(), vram_total_mb: total, vram_free_mb: total.saturating_sub(used), source: "nvidia-smi".into() };
        if best.as_ref().is_none_or(|b| gpu.vram_total_mb > b.vram_total_mb) {
            best = Some(gpu);
        }
    }
    best
}

/// What `/sys/class/drm/card*/device/` holds for one card.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrmCard {
    /// `vendor` file, e.g. `0x1002`.
    pub vendor: String,
    /// `product_name` file when the driver gives one.
    pub product: String,
    /// `mem_info_vram_total` (bytes).
    pub vram_total: Option<u64>,
    /// `mem_info_vram_used` (bytes).
    pub vram_used: Option<u64>,
}

/// AMD cards (vendor 0x1002) with a `mem_info_vram_total`; the largest.
pub fn pick_amd(cards: &[DrmCard]) -> Option<Gpu> {
    cards
        .iter()
        .filter(|c| c.vendor.trim().eq_ignore_ascii_case("0x1002"))
        .filter_map(|c| {
            let total = c.vram_total? / (1024 * 1024);
            if total == 0 {
                return None;
            }
            let used = c.vram_used.unwrap_or(0) / (1024 * 1024);
            let name = if c.product.trim().is_empty() { "AMD graphics card".to_string() } else { c.product.trim().to_string() };
            Some(Gpu { name, vram_total_mb: total, vram_free_mb: total.saturating_sub(used), source: "sysfs".into() })
        })
        .max_by_key(|g| g.vram_total_mb)
}

fn read_card(dir: &Path) -> DrmCard {
    let text = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    let num = |f: &str| text(f).trim().parse::<u64>().ok();
    DrmCard { vendor: text("vendor"), product: text("product_name"), vram_total: num("mem_info_vram_total"), vram_used: num("mem_info_vram_used") }
}

/// The cards under `drm_root` (`/sys/class/drm`): `card0`, `card1` ... (not `card0-DP-1`).
pub fn read_drm(drm_root: &Path) -> Vec<DrmCard> {
    let Ok(rd) = std::fs::read_dir(drm_root) else { return vec![] };
    let mut names: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.strip_prefix("card").is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit())))
        .collect();
    names.sort();
    names.iter().map(|n| read_card(&drm_root.join(n).join("device"))).collect()
}

/// `MemTotal` of `/proc/meminfo` in MB.
pub fn parse_meminfo_total_mb(text: &str) -> Option<u64> {
    let kb = text.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(kb / 1024)
}

pub fn ram_total_mb() -> u64 {
    parse_meminfo_total_mb(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default()).unwrap_or(0)
}

/// This machine's graphics card: NVIDIA first, then AMD, else none (CPU inference).
/// `KAGENT_GPU=none` forces none (tests and a machine whose card must not be used).
pub async fn probe() -> Option<Gpu> {
    if std::env::var("KAGENT_GPU").is_ok_and(|v| v == "none") {
        return None;
    }
    let run = tokio::process::Command::new("nvidia-smi").args(NVIDIA_ARGS).kill_on_drop(true).output();
    if let Ok(Ok(o)) = tokio::time::timeout(Duration::from_secs(5), run).await {
        if o.status.success() {
            if let Some(g) = parse_nvidia_smi(&String::from_utf8_lossy(&o.stdout)) {
                return Some(g);
            }
        }
    }
    let cards = tokio::task::spawn_blocking(|| read_drm(Path::new("/sys/class/drm"))).await.unwrap_or_default();
    pick_amd(&cards)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded on the owner's machine (2026-11-28).
    const ONE: &str = "NVIDIA GeForce RTX 3060, 12288, 2738\n";
    // Written from the documented format (two cards, one without memory reporting).
    const TWO: &str = "NVIDIA GeForce RTX 3060, 12288, 2738\nNVIDIA GeForce RTX 4090, 24564, 512\n";
    const NA: &str = "NVIDIA Tegra Orin, [N/A], [N/A]\nNVIDIA GeForce GTX 1650, 4096, 300\n";

    #[test]
    fn one_card_recorded() {
        let g = parse_nvidia_smi(ONE).unwrap();
        assert_eq!((g.name.as_str(), g.vram_total_mb, g.vram_free_mb, g.source.as_str()), ("NVIDIA GeForce RTX 3060", 12288, 9550, "nvidia-smi"));
    }

    #[test]
    fn several_cards_the_largest() {
        let g = parse_nvidia_smi(TWO).unwrap();
        assert_eq!((g.name.as_str(), g.vram_total_mb, g.vram_free_mb), ("NVIDIA GeForce RTX 4090", 24564, 24052));
    }

    #[test]
    fn card_without_memory_reporting_is_skipped() {
        let g = parse_nvidia_smi(NA).unwrap();
        assert_eq!(g.name, "NVIDIA GeForce GTX 1650");
        assert_eq!(parse_nvidia_smi("NVIDIA Tegra Orin, N/A, N/A\n"), None);
    }

    #[test]
    fn garbage_and_empty_are_none() {
        assert_eq!(parse_nvidia_smi(""), None);
        assert_eq!(parse_nvidia_smi("NVIDIA-SMI has failed because it couldn't communicate with the NVIDIA driver.\n"), None);
    }

    #[test]
    fn name_with_a_comma() {
        let g = parse_nvidia_smi("NVIDIA RTX A2000, Laptop, 4096, 100\n").unwrap();
        assert_eq!(g.name, "NVIDIA RTX A2000, Laptop");
    }

    // Written from the amdgpu sysfs format (bytes, one number per file).
    #[test]
    fn amd_sysfs() {
        let gib = 1024u64 * 1024 * 1024;
        let cards = vec![
            DrmCard { vendor: "0x8086\n".into(), product: String::new(), vram_total: None, vram_used: None },
            DrmCard { vendor: "0x1002\n".into(), product: "Radeon RX 7600\n".into(), vram_total: Some(8 * gib), vram_used: Some(gib) },
            DrmCard { vendor: "0x1002\n".into(), product: String::new(), vram_total: Some(gib / 2), vram_used: Some(0) },
        ];
        let g = pick_amd(&cards).unwrap();
        assert_eq!((g.name.as_str(), g.vram_total_mb, g.vram_free_mb, g.source.as_str()), ("Radeon RX 7600", 8192, 7168, "sysfs"));
    }

    #[test]
    fn no_amd_card_is_none() {
        assert_eq!(pick_amd(&[]), None);
        assert_eq!(pick_amd(&[DrmCard { vendor: "0x10de".into(), vram_total: Some(1 << 33), ..Default::default() }]), None);
    }

    #[test]
    fn sysfs_files_are_read() {
        let d = tempfile::tempdir().unwrap();
        let dev = d.path().join("card1/device");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::create_dir_all(d.path().join("card1-DP-1")).unwrap();
        std::fs::write(dev.join("vendor"), "0x1002\n").unwrap();
        std::fs::write(dev.join("mem_info_vram_total"), "4294967296\n").unwrap();
        std::fs::write(dev.join("mem_info_vram_used"), "1073741824\n").unwrap();
        let cards = read_drm(d.path());
        assert_eq!(cards.len(), 1);
        let g = pick_amd(&cards).unwrap();
        assert_eq!((g.vram_total_mb, g.vram_free_mb), (4096, 3072));
        assert!(read_drm(&d.path().join("missing")).is_empty());
    }

    #[test]
    fn meminfo() {
        assert_eq!(parse_meminfo_total_mb("MemTotal:       65536000 kB\nMemFree: 1 kB\n"), Some(64000));
        assert_eq!(parse_meminfo_total_mb(""), None);
    }
}
