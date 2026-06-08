//! Stage 3 — convert cleaned HTML to markdown.
//!
//! Why custom (not html2md or markdownify): we get tight control
//! over what survives. The crate's value comes from the mesh being
//! predictable; an opaque third-party converter would let formats
//! through that we'd have to chase later. The conversion below is
//! ~200 lines, deterministic, and honors the doc's allowlist exactly:
//!
//!   keeps:   headings, paragraphs, bold/italic, lists, links
//!            (with scheme guard), tables, code blocks, block quotes
//!   drops:   everything else (custom elements, residual attributes,
//!            inline styles)
//!
//! Markdown is the right target because it has no execution
//! semantics, no hidden state, and very limited ambiguity. The LLM
//! reads structured prose, not HTML.

use scraper::{Html, Selector};

use crate::types::StrainResult;

/// Schemes we accept for links. Anything else gets the link
/// stripped down to its display text — the URL is dropped entirely
/// rather than emitted with a suspicious scheme.
const ALLOWED_LINK_SCHEMES: &[&str] = &["http", "https", "mailto"];

pub fn to_markdown(html: &str) -> StrainResult<String> {
    let doc = Html::parse_fragment(html);
    let mut out = String::with_capacity(html.len() / 2);
    let mut ctx = Ctx::new();
    walk(doc.tree.root(), &mut out, &mut ctx);
    // Collapse runs of >2 blank lines into 2 (markdown convention)
    // and trim surrounding whitespace.
    let collapsed = collapse_blank_lines(&out);
    Ok(collapsed.trim().to_string())
}

/// Tracks list nesting + inside-pre state during the walk so we
/// don't accidentally re-format pre-formatted text or mis-number
/// nested ordered lists.
struct Ctx {
    list_stack: Vec<ListKind>,
    /// Counter per ordered-list level. Independent stacks because
    /// nested lists each get their own enumeration.
    ol_counters: Vec<u32>,
    in_pre: bool,
}

#[derive(Clone, Copy)]
enum ListKind {
    Ul,
    Ol,
}

impl Ctx {
    fn new() -> Self {
        Self {
            list_stack: Vec::new(),
            ol_counters: Vec::new(),
            in_pre: false,
        }
    }
    fn indent(&self) -> String {
        "  ".repeat(self.list_stack.len().saturating_sub(1))
    }
}

fn walk(node: ego_tree::NodeRef<'_, scraper::node::Node>, out: &mut String, ctx: &mut Ctx) {
    match node.value() {
        scraper::node::Node::Document | scraper::node::Node::Fragment => {
            for child in node.children() {
                walk(child, out, ctx);
            }
        }
        scraper::node::Node::Comment(_)
        | scraper::node::Node::Doctype(_)
        | scraper::node::Node::ProcessingInstruction(_) => {
            // Stage 2 should have already removed comments. Belt
            // and suspenders.
        }
        scraper::node::Node::Text(t) => {
            if ctx.in_pre {
                out.push_str(&t.text);
            } else {
                // Collapse whitespace runs to single spaces — HTML
                // semantics. Skip if the text is purely whitespace
                // and we're at a block boundary.
                let collapsed = collapse_whitespace(&t.text);
                if !collapsed.is_empty() {
                    out.push_str(&collapsed);
                }
            }
        }
        scraper::node::Node::Element(el) => walk_element(node, el, out, ctx),
    }
}

fn walk_element(
    node: ego_tree::NodeRef<'_, scraper::node::Node>,
    el: &scraper::node::Element,
    out: &mut String,
    ctx: &mut Ctx,
) {
    let tag = el.name();
    match tag {
        // ── headings ──────────────────────────────────────────────
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = tag.chars().last().unwrap().to_digit(10).unwrap() as usize;
            ensure_blank_line(out);
            out.push_str(&"#".repeat(level));
            out.push(' ');
            walk_inline(node, out, ctx);
            out.push('\n');
        }

        // ── block text ────────────────────────────────────────────
        "p" | "div" | "section" | "article" | "main" | "header" | "footer" => {
            ensure_blank_line(out);
            walk_inline(node, out, ctx);
            out.push('\n');
        }

        // ── line break ────────────────────────────────────────────
        "br" => {
            out.push_str("  \n");
        }

        // ── horizontal rule ───────────────────────────────────────
        "hr" => {
            ensure_blank_line(out);
            out.push_str("---\n");
        }

        // ── emphasis ──────────────────────────────────────────────
        "strong" | "b" => {
            out.push_str("**");
            walk_inline(node, out, ctx);
            out.push_str("**");
        }
        "em" | "i" => {
            out.push('*');
            walk_inline(node, out, ctx);
            out.push('*');
        }
        "u" => {
            // Markdown has no underline; render as italic to
            // preserve the emphasis intent without losing content.
            out.push('*');
            walk_inline(node, out, ctx);
            out.push('*');
        }

        // ── inline code & blocks ──────────────────────────────────
        "code" if !ctx.in_pre => {
            out.push('`');
            for child in node.children() {
                walk(child, out, ctx);
            }
            out.push('`');
        }
        "code" => {
            // Inside <pre> — emit verbatim, no fence (the pre
            // handler already opened the fence).
            for child in node.children() {
                walk(child, out, ctx);
            }
        }
        "pre" => {
            ensure_blank_line(out);
            out.push_str("```\n");
            ctx.in_pre = true;
            for child in node.children() {
                walk(child, out, ctx);
            }
            ctx.in_pre = false;
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n");
        }

        // ── lists ─────────────────────────────────────────────────
        "ul" => {
            ensure_blank_line(out);
            ctx.list_stack.push(ListKind::Ul);
            for child in node.children() {
                walk(child, out, ctx);
            }
            ctx.list_stack.pop();
        }
        "ol" => {
            ensure_blank_line(out);
            ctx.list_stack.push(ListKind::Ol);
            ctx.ol_counters.push(0);
            for child in node.children() {
                walk(child, out, ctx);
            }
            ctx.list_stack.pop();
            ctx.ol_counters.pop();
        }
        "li" => {
            // Indent based on current depth, then prefix.
            let indent = ctx.indent();
            out.push_str(&indent);
            match ctx.list_stack.last() {
                Some(ListKind::Ol) => {
                    let depth = ctx.ol_counters.len();
                    if depth > 0 {
                        ctx.ol_counters[depth - 1] += 1;
                        let n = ctx.ol_counters[depth - 1];
                        out.push_str(&format!("{n}. "));
                    } else {
                        out.push_str("1. ");
                    }
                }
                _ => out.push_str("- "),
            }
            walk_inline(node, out, ctx);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }

        // ── blockquote ────────────────────────────────────────────
        "blockquote" => {
            ensure_blank_line(out);
            // Capture the inner content separately so we can prefix
            // each line with `> `.
            let mut inner = String::new();
            for child in node.children() {
                walk(child, &mut inner, ctx);
            }
            for line in inner.trim().lines() {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
        }

        // ── links ─────────────────────────────────────────────────
        "a" => {
            let href = el.attr("href").unwrap_or_default();
            if scheme_allowed(href) {
                out.push('[');
                walk_inline(node, out, ctx);
                out.push_str("](");
                out.push_str(href);
                out.push(')');
            } else {
                // Reject the URL but keep the display text.
                walk_inline(node, out, ctx);
            }
        }

        // ── images ────────────────────────────────────────────────
        "img" => {
            let src = el.attr("src").unwrap_or_default();
            let alt = el.attr("alt").unwrap_or_default();
            if scheme_allowed(src) {
                out.push_str("![");
                out.push_str(alt);
                out.push_str("](");
                out.push_str(src);
                out.push(')');
            } else if !alt.is_empty() {
                // No usable URL but the alt text is meaningful prose;
                // emit it as plain text.
                out.push_str(alt);
            }
        }

        // ── tables ────────────────────────────────────────────────
        "table" => {
            ensure_blank_line(out);
            emit_table(node, out, ctx);
        }

        // ── span-like wrappers ────────────────────────────────────
        // Pass the children through; drop the wrapper.
        "span" | "font" | "small" | "sub" | "sup" | "abbr" | "cite" | "kbd" | "samp" | "var"
        | "mark" | "time" | "label" | "dfn" | "q" | "ruby" | "rb" | "rt" | "rp" | "bdi" | "bdo" => {
            walk_inline(node, out, ctx);
        }

        // ── nav / chrome stuff ─────────────────────────────────────
        // These can appear inside main content (post-extraction)
        // sometimes; treat as block container.
        "nav" | "aside" => {
            ensure_blank_line(out);
            for child in node.children() {
                walk(child, out, ctx);
            }
        }

        // ── unknown / unhandled ────────────────────────────────────
        // Drop the element, but pass content through. That keeps
        // text the operator might care about while removing
        // structure we don't understand.
        _ => {
            for child in node.children() {
                walk(child, out, ctx);
            }
        }
    }
}

/// Emit a `<table>` as a GitHub-flavored markdown table. We pull
/// the first row of cells (whether `<thead>`-wrapped or not) as the
/// header; remaining rows are body.
fn emit_table(node: ego_tree::NodeRef<'_, scraper::node::Node>, out: &mut String, ctx: &mut Ctx) {
    // Collect rows by walking <tr> descendants. We don't use
    // selectors here because we need a stable traversal order.
    let mut rows: Vec<Vec<String>> = Vec::new();
    let tr_sel = Selector::parse("tr").expect("static");
    let tmp = Html::parse_fragment(""); // unused; we use scraper's
                                        // ElementRef, not selectors,
                                        // to traverse this subtree.
    let _ = (tr_sel, tmp); // suppress warnings on stub above

    for descendant in node.descendants() {
        if let Some(el) = descendant.value().as_element() {
            if el.name() == "tr" {
                let mut row = Vec::new();
                for cell in descendant.children() {
                    if let Some(cel) = cell.value().as_element() {
                        if cel.name() == "td" || cel.name() == "th" {
                            let mut text = String::new();
                            for inner in cell.children() {
                                walk(inner, &mut text, ctx);
                            }
                            // Markdown table cells can't contain
                            // raw pipes or newlines.
                            let cleaned = text
                                .replace('|', "\\|")
                                .replace('\n', " ")
                                .trim()
                                .to_string();
                            row.push(cleaned);
                        }
                    }
                }
                if !row.is_empty() {
                    rows.push(row);
                }
            }
        }
    }

    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return;
    }

    // Header.
    let header = &rows[0];
    out.push('|');
    for i in 0..cols {
        let cell = header.get(i).map(String::as_str).unwrap_or("");
        out.push(' ');
        out.push_str(cell);
        out.push_str(" |");
    }
    out.push('\n');
    out.push('|');
    for _ in 0..cols {
        out.push_str(" --- |");
    }
    out.push('\n');
    // Body.
    for row in &rows[1..] {
        out.push('|');
        for i in 0..cols {
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            out.push(' ');
            out.push_str(cell);
            out.push_str(" |");
        }
        out.push('\n');
    }
}

/// Walk an element's children for inline contexts (no leading
/// blank-line insertion).
fn walk_inline(node: ego_tree::NodeRef<'_, scraper::node::Node>, out: &mut String, ctx: &mut Ctx) {
    for child in node.children() {
        walk(child, out, ctx);
    }
}

fn scheme_allowed(href: &str) -> bool {
    // Trim, case-fold the scheme prefix, check against allowlist.
    let trimmed = href.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Relative URLs (no scheme) — accept; they'll resolve against
    // the source URL when the assistant references them, no
    // execution risk on the markdown layer itself.
    if !trimmed.contains(':') {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    for scheme in ALLOWED_LINK_SCHEMES {
        let prefix = format!("{scheme}:");
        if lower.starts_with(&prefix) {
            return true;
        }
    }
    false
}

fn ensure_blank_line(out: &mut String) {
    if out.is_empty() {
        return;
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            last_was_space = false;
        }
    }
    out
}

fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.split_inclusive('\n') {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push_str(line);
            }
        } else {
            blank_run = 0;
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        to_markdown(html).expect("md")
    }

    #[test]
    fn headings_and_paragraphs() {
        let s = md("<h1>Title</h1><p>Body.</p><h2>Sub</h2><p>More.</p>");
        assert!(s.contains("# Title"));
        assert!(s.contains("## Sub"));
        assert!(s.contains("Body."));
        assert!(s.contains("More."));
    }

    #[test]
    fn emphasis_renders() {
        let s = md("<p><strong>bold</strong> and <em>italic</em></p>");
        assert!(s.contains("**bold**"));
        assert!(s.contains("*italic*"));
    }

    #[test]
    fn links_with_safe_schemes_pass() {
        assert!(md(r#"<a href="https://example.com">x</a>"#).contains("(https://example.com)"));
        assert!(md(r#"<a href="http://x.test">x</a>"#).contains("(http://x.test)"));
        assert!(md(r#"<a href="mailto:a@b">x</a>"#).contains("(mailto:a@b)"));
    }

    #[test]
    fn rejects_javascript_url() {
        let s = md(r#"<a href="javascript:alert(1)">click</a>"#);
        assert!(!s.contains("javascript:"));
        assert!(s.contains("click"), "display text should survive");
    }

    #[test]
    fn rejects_data_url() {
        let s = md(r#"<a href="data:text/html,inject">link</a>"#);
        assert!(!s.contains("data:"));
        assert!(s.contains("link"));
    }

    #[test]
    fn relative_urls_pass() {
        let s = md(r#"<a href="/about">about</a>"#);
        assert!(s.contains("(/about)"));
    }

    #[test]
    fn unordered_list() {
        let s = md("<ul><li>a</li><li>b</li><li>c</li></ul>");
        assert!(s.contains("- a"));
        assert!(s.contains("- b"));
        assert!(s.contains("- c"));
    }

    #[test]
    fn ordered_list_renumbers() {
        let s = md("<ol><li>first</li><li>second</li><li>third</li></ol>");
        assert!(s.contains("1. first"));
        assert!(s.contains("2. second"));
        assert!(s.contains("3. third"));
    }

    #[test]
    fn code_block_emits_fence() {
        let s = md("<pre><code>let x = 1;</code></pre>");
        assert!(s.contains("```"));
        assert!(s.contains("let x = 1;"));
    }

    #[test]
    fn blockquote_prefixes_each_line() {
        let s = md("<blockquote><p>quoted line one.</p><p>line two.</p></blockquote>");
        assert!(s.lines().any(|l| l.starts_with("> quoted line one.")));
        assert!(s.lines().any(|l| l.starts_with("> line two.")));
    }

    #[test]
    fn table_renders_as_gfm() {
        let s = md("<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>");
        assert!(s.contains("| A | B |"));
        assert!(s.contains("| --- | --- |"));
        assert!(s.contains("| 1 | 2 |"));
    }

    #[test]
    fn drops_unknown_elements_keeps_text() {
        let s = md("<custom-element>visible content</custom-element>");
        assert!(s.contains("visible content"));
    }
}
