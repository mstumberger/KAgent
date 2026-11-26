//! The extractor on markup written for it, and on the saved pages under `fixtures/`:
//! real pages as downloaded on 2026-11-26 and cut to what these tests need (every byte
//! of a fixture is a byte of its page).
//! - `lwn-1101211.html`: https://lwn.net/Articles/1101211/ , the whole page.
//! - `github-ripgrep-readme.html`: https://github.com/BurntSushi/ripgrep : the title,
//!   the `<main>` start tag, the README's `<article>` up to the end of a table, then
//!   the page's own `</article>` and `</main>`.
//! - `bughr-2003-cp1250.html`: the copy of http://www.bug.hr/ of 2003-06-23 kept by
//!   the Wayback Machine: its `<head>` and one table of the body, in windows-1250.
//! - `kernel-org.html`: https://www.kernel.org/ : its title and its `<article>` (the page
//!   has no `<main>`), fetched for scenario S30 item 3.

use super::*;

const LWN: &str = include_str!("../fixtures/lwn-1101211.html");
const GITHUB: &str = include_str!("../fixtures/github-ripgrep-readme.html");
/// Not UTF-8, so `include_bytes!`: the charset the page declares is what makes it readable.
const BUGHR: &[u8] = include_bytes!("../fixtures/bughr-2003-cp1250.html");
const KERNEL: &str = include_str!("../fixtures/kernel-org.html");

fn page(html: &str) -> Page {
    to_text(html, &Url::parse("https://example.org/posts/1.html").unwrap())
}

#[test]
fn blocks_break_lines_entities_are_decoded_and_links_resolved() {
    let p = page(
        "<html><head><title>Caf&eacute; &amp; bar</title><style>p{}</style><script>var a = '<p>not text</p>';</script></head>\
         <body><h1>Hello&nbsp;  world</h1><p>A &lt;b&gt; tag, a &quot;quote&quot;, &#39;apostrophe&#39; and &#x2014; dash.</p>\
         <ul><li>one</li><li>two</li></ul><div>a<br>b</div><noscript>no js</noscript><template>t</template>\
         <svg><text>s</text></svg><iframe>fr</iframe><textarea>ta</textarea>\
         <table><tr><td>x</td><td>y</td></tr><tr><td>z</td><td>w</td></tr></table>\
         <p>See <a href=\"/about\">the about page</a>, <a href=\"https://other.example/x?a=1&amp;b=2\">other</a>, <a href=\"#top\">top</a>, \
         <a href=\"mailto:a@b.c\">mail</a>, <a href=\"javascript:void(0)\">js</a>, <a href=\"/about\">again</a> and <a>nothing</a>.</p>\
         <!-- a comment --><p>5 &lt; 6 &amp;&amp; 7 &gt; 6</p><p title=\"a > b\">ok</p></body></html>",
    );
    assert_eq!(p.title, "Café & bar");
    assert_eq!(
        p.text,
        "Hello world\nA <b> tag, a \"quote\", 'apostrophe' and \u{2014} dash.\none\ntwo\na\nb\nx y\nz w\nSee the about page, other, top, mail, js, again and nothing.\n5 < 6 && 7 > 6\nok"
        ,
        "dropped elements, one line per block, a row of a table on one line, whitespace collapsed"
    );
    assert_eq!(
        p.links,
        vec![("the about page".to_string(), "https://example.org/about".to_string()), ("other".to_string(), "https://other.example/x?a=1&b=2".to_string())],
        "http and https only, a fragment of the page is not a link, nor a second link to the same address"
    );
}

#[test]
fn main_is_the_page_else_the_first_article_and_the_title_comes_from_the_head() {
    let p = page("<title>T</title><body><nav>Home About</nav><main><p>In main</p></main><footer>Foot</footer></body>");
    assert_eq!((p.title.as_str(), p.text.as_str()), ("T", "In main"));
    let p = page("<body><header>Head</header><article><p>First</p></article><p>After</p></body>");
    assert_eq!(p.text, "First", "the first article is the page when there is no main");
    let p = page("<main><p>outer</p><main><p>inner</p></main></main>");
    assert_eq!(p.text, "outer\ninner", "a main inside the main is part of it");
    let p = page("<body><p>a</p><main><p>never closed</p><p>b</p></body>");
    assert_eq!(p.text, "a\nnever closed\nb", "a main that is never closed does not swallow the page");
    let p = page("<div><p>a</p></div>");
    assert_eq!(p.text, "a");
    let p = page("<nav>Menu</nav><main><script>x()</script><iframe src='x'></iframe></main><footer>The text that is left</footer>");
    assert_eq!(p.text, "Menu\nThe text that is left", "a main that says nothing is not the page: the whole page is read");
    let p = page("<main><a href='/only'>only a link</a></main>");
    assert_eq!((p.text.as_str(), p.links.len()), ("only a link", 1));
    assert_eq!(page("<p>x</p><head><title>late</title><style>x</style></head><p>y</p>").title, "late");
    assert_eq!(page("").text, "", "an empty body is an empty page");
    assert_eq!(page("a < b and 3<4 <div>c</div>").text, "a < b and 3<4\nc", "a `<` that opens no tag is text");
    assert_eq!(page("<div class='x>y' title=\"a'b\">q</div>").text, "q", "a quoted value may hold a > and the other quote");
}

#[test]
fn a_link_without_text_takes_the_alt_of_its_image_and_at_most_thirty_are_listed() {
    let p = page("<p><a href='/badge'><img src='b.png' alt='Build status'></a> <a href='/icon'><img src='i.png' alt=''></a> <a href='/empty'></a></p>");
    assert_eq!(p.links, vec![("Build status".to_string(), "https://example.org/badge".to_string())], "a link that says nothing is not listed");
    let html: String = (0..45).map(|n| format!("<a href='/l{n}'>link {n}</a><br>")).collect();
    let p = page(&html);
    assert_eq!((p.links.len(), p.links[29].clone()), (MAX_LINKS, ("link 29".to_string(), "https://example.org/l29".to_string())));
    let long = page(&format!("<a href='/x'>{}</a>", "x".repeat(90)));
    assert_eq!(long.links[0].0, format!("{}...", "x".repeat(80)), "the text of a link is cut at 80 characters");
}

#[test]
fn entities_are_decoded_and_what_is_none_stays() {
    assert_eq!(decode_entities("&amp;&lt;&gt;&quot;&#39;&nbsp;&nope; &copy; a & b &#65;&#x42;&#128;"), "&<>\"' &nope; © a & b AB\u{20AC}");
    assert_eq!(decode_entities("a &amp b &kind;of &longertextthanten; &x"), "a &amp b &kind;of &longertextthanten; &x", "no semicolon, an unknown name, a name too long to be one");
    assert_eq!(decode_entities("no entities"), "no entities");
    assert_eq!(decode_entities("&#0; &#xD800; &#1114112; &#x1F600;"), "\u{FFFD} &#xD800; &#1114112; \u{1F600}");
    assert_eq!(collapse("  a\n\t b  \n"), "a b");
    assert_eq!(cut("héllo wörld", 5), "héllo...");
    assert_eq!(cut("short", 5), "short");
}

#[test]
fn a_body_that_is_not_utf8_is_read_by_the_charset_it_declares() {
    assert_eq!(decode("café".as_bytes(), None), "café", "UTF-8 needs no charset");
    assert_eq!(decode(b"\xef\xbb\xbfcaf\xc3\xa9", Some("text/html; charset=utf-8")), "café", "the UTF-8 mark is dropped");
    assert_eq!(decode(b"caf\xe9", Some("text/html; charset=iso-8859-1")), "café", "the Content-Type names it");
    assert_eq!(decode(b"caf\xe9", Some("text/html; charset=windows-1252; format=flowed")), "café");
    assert_eq!(decode(b"caf\x92s", Some("text/plain; charset=latin1")), "caf\u{2019}s", "windows-1252 is what a page means by latin1");
    assert_eq!(decode(b"<meta charset=\"windows-1252\">\ncaf\x92s", Some("text/html")), "<meta charset=\"windows-1252\">\ncaf\u{2019}s", "else the meta names it");
    assert_eq!(
        decode(b"<html><head><meta name=\"x\" content=\"y\"><META HTTP-EQUIV=\"Content-Type\" content=\"text/html; charset=windows-1250\"></head><p>Te\xe8ajna \x9alista", Some("text/html")),
        "<html><head><meta name=\"x\" content=\"y\"><META HTTP-EQUIV=\"Content-Type\" content=\"text/html; charset=windows-1250\"></head><p>Tečajna šlista"
    );
    assert_eq!(decode(b"caf\xe9 \x81", Some("text/html; charset=windows-1252")), "café \u{81}", "a byte windows-1252 leaves open is itself");
    assert_eq!(decode(b"caf\xe9", Some("text/html")), "caf\u{FFFD}", "no charset named: the byte is not valid");
    assert_eq!(decode(b"caf\xe9", Some("text/html; charset=utf-8")), "caf\u{FFFD}", "a charset with no table here reads as UTF-8");
    assert_eq!(decode(b"caf\xe9", Some("text/html; charset=iso-8859-5")), "caf\u{FFFD}");
    assert_eq!(decode(&[0xff, 0xfe, 0xe9, 0x00, 0x3d, 0xd8, 0x00, 0xde, 0x00, 0xdc], None), "é\u{1F600}\u{FFFD}", "UTF-16 with its mark: a pair, and a half of one");
    assert_eq!(decode(&[0xfe, 0xff, 0x00, 0xe9], None), "é");
}

#[test]
fn fragments_attributes_and_addresses() {
    assert_eq!(strip_tags("The <b>Linux</b> <b>Kernel</b> &quot;Organization&quot;<span>  </span>is\n here <!-- c -->"), "The Linux Kernel \"Organization\" is here", "tags go, whitespace collapses; what is inside them stays");
    assert_eq!(
        attrs_of("<A HREF=\"/x?a=1&amp;b=2\" data-x='q > w' empty value=\"a>b\"/>"),
        vec![
            ("href".to_string(), "/x?a=1&b=2".to_string()),
            ("data-x".to_string(), "q > w".to_string()),
            ("empty".to_string(), String::new()),
            ("value".to_string(), "a>b".to_string())
        ]
    );
    assert!(attrs_of("<a href=/unquoted/>").contains(&("href".to_string(), "/unquoted/".to_string())));
    let base = Url::parse("https://example.org/a/b.html").unwrap();
    assert_eq!(resolve(&base, "../c"), Some("https://example.org/c".to_string()));
    assert_eq!(resolve(&base, "//cdn.example/x"), Some("https://cdn.example/x".to_string()));
    assert_eq!(resolve(&base, "#x"), None, "a fragment of the page");
    assert_eq!(resolve(&base, ""), None);
    assert_eq!(resolve(&base, "mailto:a@b.c"), None, "another scheme");
}

/// Scenario S30 item 2: a news article (LWN, a page with a `<main>`).
#[test]
fn a_news_article_gives_its_title_its_text_and_its_links() {
    let p = to_text(LWN, &Url::parse("https://lwn.net/Articles/1101211/").unwrap());
    assert_eq!(p.title, "The first part of the 7.4 merge window [LWN.net]");
    assert!(
        p.text.starts_with(
            "For humans, by humans\nEvery article on LWN.net is written for humans, by humans. If you've enjoyed this article and want to see more like it, your subscription goes a long way to keeping the slop at bay. We are offering a free one-month trial subscription (no credit card required) to get you started.\nBy Daroc Alden\nOctober 23, 2026\nSince the merge window for 7.4 opened, there have been 6,753 non-merge changesets pulled into the mainline repository."
        ),
        "{}",
        cut(&p.text, 700)
    );
    assert!(p.text.contains("The most important changes included in this release were:\nArchitecture-specific\nAlpha now supports a number of different optional architecture features"), "a block and a list item each end a line; a link split over lines reads as one");
    assert_eq!(p.links[0], ("a free one-month trial subscription".to_string(), "https://lwn.net/Promo/nst-bots1/claim".to_string()));
    assert!(
        p.links.iter().any(|(label, url)| url == "https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=742f62b63904" && label == "number of different optional architecture features"),
        "{:?}",
        &p.links[..5]
    );
    assert_eq!(p.links.len(), 30, "at most 30 links (web.md section 2)");
}

/// Scenario S30 item 2: only `<main>` is the page; its links are relative to the page's address.
#[test]
fn only_the_main_of_a_page_is_read() {
    let p = to_text(LWN, &Url::parse("https://lwn.net/Articles/1101211/").unwrap());
    assert!(!p.text.contains("News from the source") && !p.text.contains("Subscriptions"), "the menu before <main> is not the page");
    assert!(!p.text.contains("Copyright") && !p.text.contains("Eklektix"), "nor the footer after it");
    assert!(!p.text.contains("anomaly") && !p.text.contains("function("), "the scripts and styles of the head are dropped");
}

/// Scenario S30 item 2: a GitHub README. The repository page has a `<main>` around the README's `<article>`: main wins, the README is in it.
#[test]
fn a_github_readme_is_read_inside_the_main_of_its_page() {
    let p = to_text(GITHUB, &Url::parse("https://github.com/BurntSushi/ripgrep").unwrap());
    assert!(p.title.starts_with("GitHub - BurntSushi/ripgrep: ripgrep recursively searches directories for a regex pattern"), "{}", p.title);
    assert!(
        p.text.starts_with("ripgrep (rg)\nripgrep is a line-oriented search tool that recursively searches the current directory for a regex pattern. By default, ripgrep will respect gitignore rules and automatically skip hidden files/directories and binary files. (To disable all automatic filtering by default, use rg -uuu.) ripgrep has first class support on Windows, macOS and Linux, with binary downloads available for every release. ripgrep is similar to other popular search tools like The Silver Searcher, ack and grep.\nDual-licensed under MIT or the UNLICENSE."),
        "{}",
        cut(&p.text, 900)
    );
    assert_eq!(
        &p.links[..5],
        &[
            ("every release".to_string(), "https://github.com/BurntSushi/ripgrep/releases".to_string()),
            ("Build status".to_string(), "https://github.com/BurntSushi/ripgrep/actions".to_string()),
            ("Crates.io".to_string(), "https://crates.io/crates/ripgrep".to_string()),
            ("Packaging status".to_string(), "https://repology.org/project/ripgrep/badges".to_string()),
            ("UNLICENSE".to_string(), "https://unlicense.org/".to_string())
        ],
        "a badge link takes the alt of its image; the heading anchors (a fragment of the page) are not listed"
    );
    assert!(!p.text.contains("Octicon") && !p.text.contains("viewBox"), "the SVG icons are dropped");
}

/// Scenario S30 item 2: a page with bytes that are not UTF-8 and with entities, read by the charset it declares.
#[test]
fn a_page_that_is_not_utf8_reads_by_the_charset_it_declares() {
    assert!(String::from_utf8(BUGHR.to_vec()).is_err(), "the fixture is not UTF-8");
    let html = decode(BUGHR, Some("text/html"));
    let p = to_text(&html, &Url::parse("http://www.bug.hr/").unwrap());
    assert_eq!(p.title, "Bug On Line");
    let lines: Vec<&str> = p.text.lines().collect();
    for line in ["- korisni linkovi -", "CROSS tražilica", "Hrv. željeznice", "Večernji list", "Tečajna lista", "AN Riječ urednika", "Tražimo za vas", "Zločesta stranica", "Pretraživač tema"] {
        assert!(lines.contains(&line), "{line} is missing from:\n{}", cut(&p.text, 700));
    }
    assert!(p.text.contains("English version\nPosao\nImpresum\nKomentari i poruke"), "one line per link (the entities after each are only spaces):\n{}", cut(&p.text, 1400));
    assert!(p.links.contains(&("Tražimo za vas".to_string(), "http://www.bug.hr/#trazimo".to_string())), "{:?}", &p.links[..6]);
    assert!(p.links.contains(&("Vijesti".to_string(), "http://www.bug.hr/vijesti/".to_string())), "a relative address, against the page's");
    assert!(p.links.iter().all(|(label, _)| !label.is_empty()), "the links that are only images are not listed");
}

/// Scenario S30 item 3, the page: kernel.org has an `<article>` and no `<main>`; its table of releases is a row per line.
#[test]
fn the_kernel_org_page_gives_its_releases_and_at_most_thirty_links() {
    let p = to_text(KERNEL, &Url::parse("https://www.kernel.org/").unwrap());
    assert_eq!(p.title, "The Linux Kernel Archives");
    assert!(p.text.starts_with("Protocol Location\nHTTP https://www.kernel.org/pub/\nGIT https://git.kernel.org/\nRSYNC rsync://rsync.kernel.org/pub/\nLatest Release\n7.3.6\nmainline: 7.4-rc4 2026-11-22 [tarball] [patch] [inc. patch] [view diff] [browse]"), "{}", cut(&p.text, 400));
    assert!(p.text.contains("\nstable: 7.3.6 2026-11-23 [tarball] [pgp] [patch] [inc. patch] [view diff] [browse] [changelog]\n"), "a row of the table on one line, its cells apart");
    assert!(p.text.contains("\nlongterm: 6.12.119 2026-11-23 ") && p.text.ends_with("linux-next: next-20261125 2026-11-25 [browse]"), "{}", cut(&p.text, 1200));
    assert!(!p.text.contains("Download"), "the image of the download button is not text");
    assert_eq!(p.links.len(), 30, "at most 30 links (web.md section 2)");
    assert_eq!(p.links[5], ("7.3.6".to_string(), "https://cdn.kernel.org/pub/linux/kernel/v7.x/linux-7.3.6.tar.xz".to_string()));
    assert_eq!(p.links[10], ("browse".to_string(), "https://git.kernel.org/torvalds/h/v7.4-rc4".to_string()));
    assert_eq!(p.links[11], ("pgp".to_string(), "https://cdn.kernel.org/pub/linux/kernel/v7.x/linux-7.3.6.tar.sign".to_string()), "the tarball of the stable row is the address of link 6: it is listed once");
    assert_eq!(p.links[29], ("browse".to_string(), "https://git.kernel.org/stable/h/v6.18.65".to_string()));
    assert!(p.links.iter().any(|(label, url)| label == "inc. patch" && url == "https://git.kernel.org/torvalds/p/v7.4-rc4/v7.4-rc3"), "the entity inside a link's text");
}
