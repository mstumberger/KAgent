use super::*;

fn run(tag: char, len: usize) -> String {
    std::iter::once(tag).cycle().take(len).collect()
}

#[test]
fn cut_never_splits_a_character() {
    assert_eq!(cut("abc", 5), "abc");
    assert_eq!(cut("abc", 2), "ab");
    assert_eq!(cut(&"é".repeat(5000), MAX_INPUT).chars().count(), MAX_INPUT);
    assert_eq!(cut(&run('x', 5000), MAX_INPUT).len(), MAX_INPUT, "a message is cut at 4000 characters");
}

#[test]
fn a_short_answer_is_one_message_as_it_is() {
    assert_eq!(split("Hello.", MAX_MESSAGE), vec!["Hello."]);
    assert_eq!(split("a\n\nb\n\nc", MAX_MESSAGE), vec!["a\n\nb\n\nc"], "blank lines inside a message stay");
    assert_eq!(split("", MAX_MESSAGE), Vec::<String>::new());
    assert_eq!(split("\n\n  \n", 100), Vec::<String>::new(), "blank lines alone are no message");
}

#[test]
fn the_limit_holds_and_a_cut_falls_between_paragraphs() {
    // Three paragraphs of 3000: no two share a message.
    let (a, b, c) = (run('a', 3000), run('b', 3000), run('c', 3000));
    assert_eq!(split(&format!("{a}\n\n{b}\n\n{c}"), MAX_MESSAGE), vec![a, b, c]);
    // Three of 2000: the first two share one (4002 characters), the third has its own.
    let (a, b, c) = (run('a', 2000), run('b', 2000), run('c', 2000));
    assert_eq!(split(&format!("{a}\n\n{b}\n\n{c}"), MAX_MESSAGE), vec![format!("{a}\n\n{b}"), c]);
    // With a smaller limit the same rule shows: "one two" does not fit 8, so it is cut between them.
    assert_eq!(split("one\n\ntwo\n\nthree", 8), vec!["one\n\ntwo", "three"]);
    assert_eq!(split("one\n\ntwo\n\nthree", 7), vec!["one", "two", "three"]);
    // A last message does not end in the blank lines that followed it.
    assert_eq!(split("one\n\ntwo\n\n\n", 8), vec!["one\n\ntwo"]);
}

#[test]
fn a_paragraph_is_cut_at_line_ends_only_when_it_is_too_long_on_its_own() {
    // Three lines of 1997: two fit a message (3995 characters), the third is one of its own.
    let lines = (0..3).map(|i| format!("{}{}", run('x', 1996), i)).collect::<Vec<_>>();
    assert_eq!(split(&lines.join("\n"), MAX_MESSAGE), vec![format!("{}\n{}", lines[0], lines[1]), lines[2].clone()]);
    // Short lines pack: five characters a line, so four lines to a message of 23.
    let one = (0..10).map(|i| format!("a{i:04}")).collect::<Vec<_>>().join("\n");
    assert_eq!(
        split(&one, 23),
        vec!["a0000\na0001\na0002\na0003", "a0004\na0005\na0006\na0007", "a0008\na0009"],
        "a cut is after a line while lines are shorter than a message"
    );
}

#[test]
fn one_line_longer_than_a_message_is_cut_at_a_character() {
    assert_eq!(split(&run('x', 9000), MAX_MESSAGE), vec![run('x', 4096), run('x', 4096), run('x', 808)]);
    let wide = "é".repeat(5000);
    let pieces = split(&wide, MAX_MESSAGE);
    assert_eq!(pieces.iter().map(|p| p.chars().count()).collect::<Vec<_>>(), vec![4096, 904], "never inside a character");
}

#[test]
fn a_cut_never_falls_inside_a_code_block() {
    // A paragraph and a code block with blank lines in it: neither shares a message with the other.
    let code = format!("```rust\n{}\n\n{}\n```", run('c', 1500), run('d', 1490));
    assert_eq!(code.chars().count(), 8 + 1500 + 2 + 1490 + 4, "3004 characters: too long to share a message with the paragraph");
    let text = format!("{}\n\n{}", run('p', 2000), code);
    assert_eq!(split(&text, MAX_MESSAGE), vec![run('p', 2000), code.clone()], "the blank lines inside the code block do not split it");
    // Short ones are packed with what comes before and after, blank line by blank line.
    let text = "intro\n\n```\na = 1;\n\nb = 2;\n```\n\noutro";
    assert_eq!(split(text, 100), vec![text.to_string()]);
    assert_eq!(split(text, 24), vec!["intro", "```\na = 1;\n\nb = 2;\n```", "outro"], "the cut is never inside the code block (22 characters)");
}

#[test]
fn a_code_block_longer_than_a_message_is_closed_and_opened_again() {
    let body = (0..40).map(|i| format!("let x{i} = \"{}\";", run('y', 100))).collect::<Vec<_>>().join("\n");
    let text = format!("````rust\n{body}\n````");
    let pieces = split(&text, MAX_MESSAGE);
    assert!(pieces.len() > 1, "40 lines of some 112 characters do not fit one message");
    for piece in &pieces {
        assert!(piece.starts_with("````rust\n") && piece.ends_with("\n````"), "each piece is a whole code block: {piece:?}");
        assert!(piece.chars().count() <= MAX_MESSAGE, "{} characters", piece.chars().count());
    }
    let joined = pieces.iter().map(|p| p.trim_start_matches("````rust\n").trim_end_matches("\n````").to_string()).collect::<Vec<_>>().join("\n");
    assert_eq!(joined, body, "nothing of the code is lost");
    // A code block left open is one block to the end, like a closed one.
    assert_eq!(split("```\na\n```", 100), vec!["```\na\n```"]);
    assert_eq!(split("```\na\n\nb", 100), vec!["```\na\n\nb"]);
}

#[test]
fn directive_lines_are_dropped_and_nothing_else() {
    assert_eq!(drop_directives("::widget{id=\"s/2\"}"), "");
    assert_eq!(drop_directives("Before\n::widget{id=\"s/2\"}\nAfter"), "Before\nAfter");
    assert_eq!(drop_directives("::widget{id=\"s/2\"\n  title=\"x\"}\nDone."), "Done.", "a directive runs to its closing brace, over lines");
    assert_eq!(drop_directives("::widget{id=\"s/2\"} Done."), "Done.", "what follows the brace on the line is text");
    assert_eq!(drop_directives("```\n::widget{id=\"s/2\"}\n```"), "```\n::widget{id=\"s/2\"}\n```", "one quoted in a code block is code");
    for (text, why) in [("::: note", "a fenced div"), (":: note", "no name and no brace"), ("see ::widget{id=\"s/2\"}", "not at the start of the line"), ("::name", "no brace")] {
        assert_eq!(drop_directives(text), text, "{why}");
    }
}

#[test]
fn markdown_becomes_telegram_html() {
    assert_eq!(to_html("a < b & c > d"), "a &lt; b &amp; c &gt; d", "`<`, `>` and `&` are escaped");
    assert_eq!(to_html("**bold** and *it* and _us_ and __also__"), "<b>bold</b> and <i>it</i> and <i>us</i> and <b>also</b>");
    assert_eq!(to_html("run `x < y` now"), "run <code>x &lt; y</code> now", "code spans are kept, escaped like text");
    assert_eq!(to_html("# Title #\n## Disk report\n### Deep\n#Not a heading"), "Title\nDisk report\nDeep\n#Not a heading", "headings are plain text");
    assert_eq!(to_html("| a | b |\n|---|:---:|\n| 1 | 2 |\n\n|just pipes|"), "a | b\n1 | 2\n\n|just pipes|", "tables are plain text: no rule row, no outer pipes; pipes alone make no table");
    assert_eq!(to_html("- one\n- two\n> quoted"), "- one\n- two\n&gt; quoted", "lists and quotes are text");
    assert_eq!(
        to_html("```rust\nfn main() { let x = a & b; }\n```"),
        "<pre><code>fn main() { let x = a &amp; b; }</code></pre>",
        "a code block stays code, escaped, with no blank line inside its tags"
    );
    assert_eq!(to_html("```\n**not bold** <b>\n```"), "<pre><code>**not bold** &lt;b&gt;</code></pre>", "nothing inside a code block is markup");
    assert_eq!(to_html("Text:\n```\na = 1;\n```\nAfter."), "Text:\n<pre><code>a = 1;</code></pre>\nAfter.", "a blank line opens and closes nothing extra");
    assert_eq!(to_html("[docs](https://x.y/z) and [x](y \"t\")"), "docs (https://x.y/z) and x (y \"t\")", "links are plain text: Telegram makes the address tappable");
    assert_eq!(to_html("<https://example.com>"), "&lt;https://example.com&gt;");
    assert_eq!(to_html("file_name_here, 2*3*4 and a * b"), "file_name_here, 2<i>3</i>4 and a * b", "an underscore between letters is not a marker");
    assert_eq!(to_html("\\*not italic\\*"), "*not italic*", "a backslash escapes a marker");
}

#[test]
fn every_tag_the_converter_opens_it_closes() {
    for text in ["**open", "*a **b* c**", "__x__y", "`unclosed", "a ** b ** c", "_ _", "***both***", "[a](b", "```, `x`, ```", "`a ``` b`"] {
        let html = to_html(text);
        for tag in ["b", "i", "code", "pre"] {
            assert_eq!(html.matches(&format!("<{tag}>")).count(), html.matches(&format!("</{tag}>")).count(), "{text:?} became {html:?}: <{tag}> is balanced");
        }
        assert!(!html.contains("<b></b>") && !html.contains("<i></i>"), "{text:?} became {html:?}: no empty tag");
    }
    assert_eq!(to_html("***both***"), "<b><i>both</i></b>");
    assert_eq!(to_html("*a **b* c**"), "<i>a </i><b><i>b</i></b><b> c</b>");
    assert_eq!(to_html("`a ``` b`"), "<code>a ``` b</code>", "a span closes with as many backticks as it opened");
}

#[test]
fn plain_is_the_same_text_with_no_tag() {
    assert_eq!(to_plain("**bold** <x> `y` [d](https://u)"), "bold &lt;x&gt; y d (https://u)");
    assert_eq!(to_plain("```\na<b\n```"), "a&lt;b", "the fence lines go, the code stays");
    assert_eq!(to_plain("| a | b |\n|---|---|"), "a | b");
    assert_eq!(escape("a<b>&c\"'"), "a&lt;b&gt;&amp;c\"'");
}
