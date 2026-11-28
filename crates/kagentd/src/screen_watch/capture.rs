//! Taking the frame of a watch (screen-watch.md section 2, step 1) and finding the window to
//! watch. A window is read through the X server (`xwd -id`, to stdout: no file). A monitor
//! goes through `spectacle`, which can only write a file: it is made in the runtime
//! directory (memory, mode 0700) and deleted before the function returns. The rules of
//! `vision.md` section 3 (no lock screen, no own cards) are the loop's, before it asks for a frame.
//! `KAGENT_FAKE_SCREENSHOT` (a PNG path) stands in for any capture in the tests, which never
//! take a picture of the owner's screen; `KAGENT_FAKE_WINDOW` (`<id> <title>`) for the active window.

use std::time::Duration;

use kagent_core::tool::ToolContext;
use kagent_core::tools::exec::exec;

use super::Target;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// The window of the watch is gone.
    WindowGone,
    Other(String),
}

/// The window that is active when a watch starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    /// `0x0460000a`.
    pub id: String,
    pub title: String,
    /// It is a window of KAgent itself: never watched.
    pub own: bool,
}

fn ctx() -> ToolContext {
    ToolContext::standalone(Duration::from_secs(8), 4_000_000)
}

/// The id as the tools print it: `0x` and eight hex digits.
pub fn hex_id(decimal: &str) -> Option<String> {
    decimal.trim().parse::<u64>().ok().map(|n| format!("0x{n:08x}"))
}

/// The active window (X11: `xdotool`), or why there is none.
pub async fn active_window() -> Result<WindowInfo, String> {
    if let Ok(fake) = std::env::var("KAGENT_FAKE_WINDOW") {
        let (id, title) = fake.split_once(' ').unwrap_or((fake.as_str(), ""));
        return Ok(WindowInfo { id: id.into(), title: title.into(), own: false });
    }
    let need = "a window is found through xdotool on X11: use target \"monitor\" instead, or install xdotool";
    if kagent_core::tools::xdg::which("xdotool").is_none() {
        return Err(need.into());
    }
    let c = ctx();
    let id = exec(&c, &["xdotool", "getactivewindow"]).run().await.map_err(|e| e.to_string())?;
    let Some(id) = hex_id(&id.stdout) else { return Err(format!("no active window was found ({need})")) };
    let title = exec(&c, &["xdotool", "getwindowname", &id]).run().await.map(|r| r.stdout.trim().to_string()).unwrap_or_default();
    let class = exec(&c, &["xprop", "-id", &id, "WM_CLASS"]).run().await.map(|r| r.stdout).unwrap_or_default();
    Ok(WindowInfo { id, title, own: class.to_ascii_lowercase().contains("kagent") })
}

/// The title of the active window, found now and in a blocking way (a call assessment is not async): the
/// summary of a `start` names it. `None` when it cannot be found.
pub fn active_title_blocking() -> Option<String> {
    if let Ok(fake) = std::env::var("KAGENT_FAKE_WINDOW") {
        return Some(fake.split_once(' ').map(|(_, t)| t).unwrap_or("").to_string());
    }
    kagent_core::tools::xdg::which("xdotool")?;
    let out = std::process::Command::new("xdotool").args(["getactivewindow", "getwindowname"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|t| !t.is_empty())
}

/// The PNG of the target.
pub async fn grab(target: Target, window_id: Option<&str>) -> Result<Vec<u8>, CaptureError> {
    if let Ok(fake) = std::env::var("KAGENT_FAKE_SCREENSHOT") {
        return std::fs::read(&fake).map_err(|e| CaptureError::Other(format!("the fake screenshot {fake}: {e}")));
    }
    match (target, window_id) {
        (Target::Window, Some(id)) => grab_window(id).await,
        (Target::Window, None) => Err(CaptureError::Other("no window to watch".into())),
        (Target::Monitor, _) => grab_monitor().await,
    }
}

async fn grab_window(id: &str) -> Result<Vec<u8>, CaptureError> {
    let c = ctx();
    let other = |e: String| CaptureError::Other(e);
    if kagent_core::tools::xdg::which("xwd").is_none() {
        return Err(other("xwd is not installed: use target \"monitor\" or install x11-apps".into()));
    }
    // A window that is gone has no name; xdotool says so with a failure.
    match exec(&c, &["xdotool", "getwindowname", id]).run().await {
        Ok(r) if r.success() => {}
        _ => return Err(CaptureError::WindowGone),
    }
    let out = tokio::process::Command::new("xwd")
        .args(["-silent", "-id", id])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| other(e.to_string()))?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(CaptureError::WindowGone);
    }
    tokio::task::spawn_blocking(move || png_of_xwd(&out.stdout)).await.map_err(|e| other(e.to_string()))?.map_err(other)
}

async fn grab_monitor() -> Result<Vec<u8>, CaptureError> {
    let other = |e: String| CaptureError::Other(e);
    if kagent_core::tools::xdg::which("spectacle").is_none() {
        return Err(other("spectacle is not installed: a monitor is captured with it".into()));
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).filter(|d| d.is_dir()).unwrap_or_else(std::env::temp_dir).join(format!("kagent-watch-{}", std::process::id()));
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| other(e.to_string()))?;
    }
    let path = dir.join("frame.png");
    let argv = ["spectacle".to_string(), "-b".into(), "-n".into(), "-m".into(), "-o".into(), path.display().to_string()];
    let _ = exec(&ctx(), &argv).run().await;
    let bytes = std::fs::read(&path);
    // Gone before anything else happens, whether it was read or not.
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    bytes.map_err(|e| other(format!("the monitor could not be captured: {e}")))
}

/// The PNG of an `xwd` dump (a TrueColor window of 24 or 32 bits a pixel).
pub fn png_of_xwd(dump: &[u8]) -> Result<Vec<u8>, String> {
    let word = |i: usize| -> Result<u32, String> {
        dump.get(i * 4..i * 4 + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| "the window dump is cut short".to_string())
    };
    let (header, depth_bits, w, h) = (word(0)? as usize, word(11)?, word(4)? as usize, word(5)? as usize);
    let (lsb, bpl, masks, ncolors) = (word(7)? == 0, word(12)? as usize, [word(14)?, word(15)?, word(16)?], word(19)? as usize);
    if !(depth_bits == 32 || depth_bits == 24) || w == 0 || h == 0 || w > 16_384 || h > 16_384 {
        return Err(format!("a window dump of {w}x{h} at {depth_bits} bits is not supported"));
    }
    let bytes_pp = (depth_bits / 8) as usize;
    let start = header + ncolors * 12;
    let pixels = dump.get(start..start + bpl * h).ok_or("the window dump is cut short")?;
    let shift = |m: u32| (m.trailing_zeros(), (m >> m.trailing_zeros()).max(1));
    let chans = masks.map(shift);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let row = &pixels[y * bpl..y * bpl + w * bytes_pp];
        for px in row.chunks_exact(bytes_pp) {
            let mut v = 0u32;
            if lsb {
                for (i, b) in px.iter().enumerate() {
                    v |= u32::from(*b) << (8 * i);
                }
            } else {
                for b in px {
                    v = (v << 8) | u32::from(*b);
                }
            }
            for (s, max) in chans {
                rgb.push((((v >> s) & max) * 255 / max) as u8);
            }
        }
    }
    kagent_core::session::images::png_of_rgb(w as u32, h as u32, rgb).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
