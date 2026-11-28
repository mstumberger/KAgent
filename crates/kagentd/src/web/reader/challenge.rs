//! Is this page an interstitial that asks for a human check (web.md section 9)? Pure:
//! the title and the text of the page, and its markup. A challenge page is short; a long
//! article that talks about captchas is not one.

/// A title of an interstitial starts with one of these (lowercase). English and Slovenian.
const TITLES: &[&str] = &[
    "just a moment",
    "attention required",
    "verify you are human",
    "verifying you are human",
    "security check",
    "checking your browser",
    "checking if the site connection is secure",
    "one more step",
    "are you a robot",
    "robot check",
    "human verification",
    "bot verification",
    "pardon our interruption",
    "access denied",
    "request blocked",
    "ddos protection",
    "please wait",
    "samo trenutek",
    "trenutek, prosim",
    "preverjanje varnosti",
    "varnostno preverjanje",
    "preverjanje brskalnika",
    "preverjamo vaš brskalnik",
    "potrdite, da ste človek",
    "dostop zavrnjen",
    "še en korak",
];

/// Markers in the markup (lowercase).
const MARKUP: &[&str] = &[
    "cf-challenge",
    "challenge-platform",
    "cf-turnstile",
    "/cdn-cgi/challenge",
    "g-recaptcha",
    "h-captcha",
    "hcaptcha.com",
    "px-captcha",
    "captcha-delivery.com",
    "_incapsula_resource",
    "anomaly-modal",
];

/// Markers in the text of the page (lowercase).
const WORDS: &[&str] = &[
    "verify you are human",
    "verify that you are human",
    "bots use duckduckgo too",
    "checking your browser before accessing",
    "enable javascript and cookies to continue",
    "unusual traffic from your computer network",
    "are you a robot",
    "complete the security check",
    "needs to review the security of your connection",
    "potrdite, da ste človek",
    "preverjamo vaš brskalnik",
    "preverjamo, ali ste človek",
    "nenavaden promet iz vašega omrežja",
];

/// A page with more text than this is not an interstitial.
const SHORT_TITLE: usize = 3000;
const SHORT_MARKER: usize = 2000;

/// Whether the page is a challenge. `text` is the visible text, `html` the markup.
pub fn is_challenge(title: &str, text: &str, html: &str) -> bool {
    let length = text.chars().count();
    let title = title.trim().to_lowercase();
    if length < SHORT_TITLE && TITLES.iter().any(|t| title.starts_with(t)) {
        return true;
    }
    if length >= SHORT_MARKER {
        return false;
    }
    let (text, html) = (text.to_lowercase(), html.to_lowercase());
    MARKUP.iter().any(|m| html.contains(m)) || WORDS.iter().any(|w| text.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Titles and sentences of the interstitials as they are known: written from the pages
    /// of Cloudflare, DDoS-Guard, Akamai, Incapsula, DuckDuckGo and Google, not downloaded.
    #[test]
    fn the_known_interstitials_are_recognised() {
        let cases: &[(&str, &str, &str)] = &[
            ("Just a moment...", "Checking if the site connection is secure", "<div id=\"challenge-platform\">"),
            ("Just a moment...", "Enable JavaScript and cookies to continue", ""),
            ("Attention Required! | Cloudflare", "Please enable cookies. Sorry, you have been blocked", ""),
            ("Verify you are human", "example.com needs to review the security of your connection before proceeding.", ""),
            ("Security Check", "", ""),
            ("Checking your browser before accessing example.com", "This process is automatic.", ""),
            ("One more step", "Please complete the security check to access example.com", "<div class=\"h-captcha\">"),
            ("Robot Check", "Enter the characters you see below", ""),
            ("Access denied", "You don't have permission to access this server.", ""),
            ("Pardon Our Interruption", "As you were browsing something about your browser made us think you were a bot.", ""),
            ("DuckDuckGo", "Unfortunately, bots use DuckDuckGo too. Please complete the following challenge to confirm this search was made by a human.", "<div class=\"anomaly-modal__mask\">"),
            ("Google", "Our systems have detected unusual traffic from your computer network.", ""),
            ("Please wait...", "", "<script src=\"https://geo.captcha-delivery.com/captcha/\"></script>"),
            ("Samo trenutek ...", "Preverjamo vaš brskalnik, preden odprete stran.", ""),
            ("Preverjanje varnosti", "Potrdite, da ste človek", ""),
            ("Dostop zavrnjen", "Nimate dovoljenja za dostop do tega strežnika.", ""),
            ("Še en korak", "Prosimo, dokončajte varnostno preverjanje.", "<div class=\"cf-turnstile\"></div>"),
            ("Varnostno preverjanje | stran", "Preverjamo, ali ste človek.", ""),
            ("", "", "<script src=\"/cdn-cgi/challenge-platform/h/b/orchestrate/jsch/v1\"></script>"),
        ];
        for (title, text, html) in cases {
            assert!(is_challenge(title, text, html), "{title:?} {text:?}");
        }
    }

    #[test]
    fn normal_pages_are_not_challenges() {
        let article = "A long article about web security. ".repeat(200);
        let cases: &[(&str, &str, &str)] = &[
            ("Linux kernel - Wikipedia", "The Linux kernel is a free and open-source Unix-like kernel.", ""),
            ("Cloudflare - Wikipedia", "Cloudflare, Inc. is an American company.", "<a href=\"https://www.cloudflare.com\">"),
            ("How to pass a security check at the airport", &article, ""),
            ("Access denied errors in Nginx: a guide", &article, ""),
            ("Why captchas are annoying", &article, "<div class=\"g-recaptcha\" data-sitekey=\"x\"></div>"),
            ("Contact us", "Fill in the form. This form is protected by reCAPTCHA.", ""),
            ("Prijava", "Prijavite se v svoj račun. Pozabljeno geslo?", "<form>"),
            ("Novice: preverjanje dejstev", &article, ""),
            ("Moment of truth", "A short story.", ""),
            ("", "", ""),
            ("A page", "Please wait while we load your account. The page has plenty of text. ", ""),
        ];
        for (title, text, html) in cases {
            assert!(!is_challenge(title, text, html), "{title:?}");
        }
    }
}
