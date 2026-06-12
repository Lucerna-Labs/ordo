//! Stage 2 — strip invisible content.
//!
//! This is where the classic hidden-injection tricks die. We parse
//! with a real HTML5 parser (never regex), walk the DOM, and remove:
//!
//!   1. Element subtrees that don't render anything an operator
//!      would see (`<script>`, `<style>`, `<noscript>`, `<iframe>`,
//!      `<object>`, `<embed>`, `<template>`, comments).
//!   2. Elements styled invisible (`display: none`,
//!      `visibility: hidden`, `opacity: 0`, `font-size: 0`,
//!      offscreen-positioned).
//!   3. Elements explicitly marked as decorative
//!      (`aria-hidden="true"`).
//!   4. Attributes that are common injection carriers (every
//!      `on*` event handler, `style`, `data-*`).
//!   5. Zero-width / direction-override / format-control characters
//!      from the surviving text.
//!
//! No element / attribute / character filter is supplied by the
//! caller. The mesh is hardcoded — that's the point.

use std::cell::RefCell;
use std::collections::HashSet;

use scraper::{Html, Selector};

use crate::types::StrainResult;

/// Element names whose entire subtree gets dropped. None of them
/// render visible content; all are common injection carriers.
const DROP_ELEMENTS: &[&str] = &[
    "script", "style", "noscript", "iframe", "object", "embed", "template",
    // SVG can carry scripts and event handlers; out of scope to
    // sanitize properly, drop wholesale.
    "svg", // <link> and <meta> in body context are weird; drop them too.
    "link", "meta",
];

/// Attribute names removed from every surviving element. Event
/// handlers (`on*`) and `style` go via prefix/exact rules below; the
/// list here is the residual set worth pruning explicitly.
const DROP_ATTRIBUTES_EXACT: &[&str] = &[
    "style", // Common analytics / tracking that occasionally carries injections.
    "ping",
];

/// Characters stripped from all text nodes. These are zero-width or
/// direction-override / format-control — invisible to a reader, but
/// can hide payload text from a reviewer skimming the page source.
const ZERO_WIDTH_CHARS: &[char] = &[
    '\u{200B}', // zero-width space
    '\u{200C}', // zero-width non-joiner
    '\u{200D}', // zero-width joiner
    '\u{FEFF}', // zero-width no-break space (BOM)
    '\u{202E}', // right-to-left override
    '\u{202D}', // left-to-right override
    '\u{2066}', // left-to-right isolate
    '\u{2067}', // right-to-left isolate
    '\u{2068}', // first-strong isolate
    '\u{2069}', // pop directional isolate
    '\u{00AD}', // soft hyphen
    '\u{2060}', // word joiner
    '\u{180E}', // mongolian vowel separator
];

/// Strip hidden / invisible content from the supplied HTML and
/// return cleaned HTML.
///
/// We re-serialize from the parsed DOM so the output is well-formed
/// HTML even if the input was sloppy. Markdown conversion (Stage 3)
/// then runs over predictable input.
pub fn strip_invisible(html: &str) -> StrainResult<String> {
    // scraper's parser is html5ever, which is permissive — it doesn't
    // fail on malformed input, it just builds a best-effort tree.
    // That matches our threat model (hostile pages will absolutely
    // be malformed on purpose).
    let doc = Html::parse_fragment(html);

    // Walk the DOM, collecting node IDs to drop. We can't mutate
    // scraper's Html in place (its tree is owned by ego_tree::Tree),
    // so we re-emit by recursive serialization that skips dropped
    // nodes.
    let drop_set: RefCell<HashSet<ego_tree::NodeId>> = RefCell::new(HashSet::new());
    mark_drops(&doc, &drop_set);

    // Now re-serialize. We build a cleaned HTML string from the root
    // by walking the tree in order, skipping dropped subtrees and
    // applying attribute / text filters.
    let mut out = String::with_capacity(html.len());
    let root = doc.tree.root();
    serialize_clean(root, &drop_set.borrow(), &mut out);
    Ok(out)
}

/// Walk the parsed DOM and add nodes to the drop set.
///
/// We use scraper's CSS selectors for the simple cases and a manual
/// walk for the cases that need attribute / style inspection.
fn mark_drops(doc: &Html, drop_set: &RefCell<HashSet<ego_tree::NodeId>>) {
    // 1. Element-name drops — match by tag name.
    for tag in DROP_ELEMENTS {
        let selector = Selector::parse(tag).expect("static selector");
        for el in doc.select(&selector) {
            drop_set.borrow_mut().insert(el.id());
        }
    }

    // 2. aria-hidden="true" drops.
    let aria_hidden = Selector::parse("[aria-hidden=\"true\"]").expect("static selector");
    for el in doc.select(&aria_hidden) {
        drop_set.borrow_mut().insert(el.id());
    }

    // 3. Comment nodes are tracked separately during serialization
    //    (scraper exposes them as a different node kind, not via
    //    selectors).

    // 4. Style-based hiding — inspect the `style` attribute on every
    //    element. Manual walk because scraper doesn't compute
    //    rendered styles; we look at inline style declarations only.
    //    External CSS isn't applied (no DOM rendering happens here),
    //    but inline style is where most hiding tricks land in the
    //    wild.
    for node in doc.tree.nodes() {
        if let Some(el) = node.value().as_element() {
            if let Some(style) = el.attr("style") {
                if style_hides_content(style) {
                    drop_set.borrow_mut().insert(node.id());
                }
            }
        }
    }
}

/// Best-effort inline-style hiding detector. Inspects the `style`
/// attribute string for declarations that hide content.
fn style_hides_content(style: &str) -> bool {
    // Normalize: lowercase, strip whitespace around colons and
    // semicolons, split into declarations.
    let normalized = style.to_ascii_lowercase();
    for decl in normalized.split(';') {
        let mut parts = decl.splitn(2, ':');
        let prop = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        match (prop, value) {
            ("display", "none") => return true,
            ("visibility", v) if v == "hidden" || v == "collapse" => return true,
            ("opacity", v) => {
                if let Some(num) = parse_opacity(v) {
                    if num <= 0.001 {
                        return true;
                    }
                }
            }
            ("font-size", v) => {
                if is_zero_size(v) {
                    return true;
                }
            }
            ("position", v) if v == "absolute" || v == "fixed" => {
                // We can't fully judge offscreen without parsing
                // left/top together. Heuristic: if any positional
                // declaration is far negative, the element is
                // probably offscreen. Caller already split on `;`,
                // so we'd need to look at sibling decls. Skip for
                // now — `display:none` and friends catch the common
                // cases.
            }
            ("left", v) | ("top", v) | ("right", v) | ("bottom", v) if is_far_offscreen(v) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn parse_opacity(v: &str) -> Option<f32> {
    let v = v.trim_end_matches('%');
    v.parse::<f32>().ok().map(|n| {
        // Treat "0%" notation as a fraction of 100.
        if n > 1.0 {
            n / 100.0
        } else {
            n
        }
    })
}

fn is_zero_size(v: &str) -> bool {
    let trimmed = v.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
    matches!(trimmed, "0" | "0.0" | "0.00")
}

fn is_far_offscreen(v: &str) -> bool {
    // Catches `-9999px`, `-99999em`, etc. Anything more negative
    // than -1000 in any unit is offscreen for any sane viewport.
    let digits: String = v
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
        .collect();
    digits.parse::<f32>().map(|n| n <= -1000.0).unwrap_or(false)
}

/// Re-emit the DOM as cleaned HTML, skipping dropped subtrees and
/// applying attribute / text-content filters along the way.
fn serialize_clean(
    node: ego_tree::NodeRef<'_, scraper::node::Node>,
    drop_set: &HashSet<ego_tree::NodeId>,
    out: &mut String,
) {
    if drop_set.contains(&node.id()) {
        return;
    }
    match node.value() {
        // Comment nodes: never emit. Comments are invisible to
        // readers and a classic injection carrier.
        scraper::node::Node::Comment(_) => {}

        // Doctype, processing instructions: drop. We're emitting an
        // HTML fragment, not a full document.
        scraper::node::Node::Doctype(_) | scraper::node::Node::ProcessingInstruction(_) => {}

        scraper::node::Node::Document | scraper::node::Node::Fragment => {
            for child in node.children() {
                serialize_clean(child, drop_set, out);
            }
        }

        scraper::node::Node::Text(text) => {
            let clean = strip_zero_width(&text.text);
            // HTML-escape for safety. We're feeding into a markdown
            // converter next, but the markdown converter expects
            // well-formed HTML, so escape `<` `>` `&`.
            push_html_escaped(out, &clean);
        }

        scraper::node::Node::Element(el) => {
            let tag = el.name();
            out.push('<');
            out.push_str(tag);
            for attr in el.attrs() {
                let (name, value) = attr;
                if !attr_should_drop(name) {
                    out.push(' ');
                    out.push_str(name);
                    out.push_str("=\"");
                    push_attr_escaped(out, value);
                    out.push('"');
                }
            }
            out.push('>');
            for child in node.children() {
                serialize_clean(child, drop_set, out);
            }
            // Self-closing tags don't need closing tags. List the
            // common ones.
            if !is_void_element(tag) {
                out.push_str("</");
                out.push_str(tag);
                out.push('>');
            }
        }
    }
}

fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
    )
}

fn attr_should_drop(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // Event handlers: any attribute starting with `on`.
    if lower.starts_with("on") && lower.len() > 2 {
        return true;
    }
    // data-* attributes — common carrier of structured payloads
    // that occasionally include instruction-shaped strings.
    if lower.starts_with("data-") {
        return true;
    }
    if DROP_ATTRIBUTES_EXACT.contains(&lower.as_str()) {
        return true;
    }
    false
}

/// Strip every code point in [`ZERO_WIDTH_CHARS`] from `s`.
pub fn strip_zero_width(s: &str) -> String {
    s.chars()
        .filter(|c| !ZERO_WIDTH_CHARS.contains(c))
        .collect()
}

fn push_html_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            other => out.push(other),
        }
    }
}

fn push_attr_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(html: &str) -> String {
        strip_invisible(html).expect("strip")
    }

    #[test]
    fn drops_script_tag_and_contents() {
        let out = run("<p>visible</p><script>alert('inject')</script>");
        assert!(out.contains("visible"));
        assert!(!out.contains("alert"));
        assert!(!out.contains("script"));
    }

    #[test]
    fn drops_style_tag() {
        let out = run("<p>visible</p><style>body{display:none}</style>");
        assert!(!out.contains("body{"));
    }

    #[test]
    fn drops_iframe() {
        let out = run("<p>p</p><iframe src='https://evil.com'>nested</iframe>");
        assert!(!out.contains("iframe"));
        assert!(!out.contains("evil.com"));
    }

    #[test]
    fn drops_html_comments() {
        let out = run("<p>visible</p><!-- hidden injection -->");
        assert!(out.contains("visible"));
        assert!(!out.contains("hidden injection"));
    }

    #[test]
    fn drops_aria_hidden_subtree() {
        let out = run(r#"<p>visible</p><span aria-hidden="true">SECRET PAYLOAD</span>"#);
        assert!(out.contains("visible"));
        assert!(!out.contains("SECRET PAYLOAD"));
    }

    #[test]
    fn drops_display_none_subtree() {
        let out = run(r#"<p>visible</p><span style="display: none">SECRET</span>"#);
        assert!(out.contains("visible"));
        assert!(!out.contains("SECRET"));
    }

    #[test]
    fn drops_visibility_hidden() {
        let out = run(r#"<p>v</p><span style="visibility:hidden">SECRET</span>"#);
        assert!(!out.contains("SECRET"));
    }

    #[test]
    fn drops_zero_opacity() {
        for value in ["opacity:0", "opacity: 0", "opacity:0.0", "opacity: 0%"] {
            let html = format!(r#"<span style="{value}">SECRET</span>visible"#);
            let out = run(&html);
            assert!(!out.contains("SECRET"), "opacity={value} did not hide");
            assert!(out.contains("visible"));
        }
    }

    #[test]
    fn drops_zero_font_size() {
        for value in ["font-size:0", "font-size: 0px", "font-size:0pt"] {
            let html = format!(r#"<span style="{value}">SECRET</span>visible"#);
            let out = run(&html);
            assert!(!out.contains("SECRET"), "{value} did not hide");
        }
    }

    #[test]
    fn drops_offscreen_positioned() {
        let html = r#"<span style="position:absolute;left:-9999px">SECRET</span>visible"#;
        let out = run(html);
        assert!(!out.contains("SECRET"));
    }

    #[test]
    fn strips_zero_width_characters_from_text() {
        // Inject a zero-width space inside otherwise normal text.
        // After stripping, the text should be intact but the ZWS gone.
        let html = "<p>hello\u{200B}world</p>";
        let out = run(html);
        assert!(out.contains("helloworld"));
        assert!(!out.contains('\u{200B}'));
    }

    #[test]
    fn strips_rtl_override() {
        // Right-to-left override is a classic phishing/injection
        // hiding trick. Any of the directional override codepoints
        // should be stripped from text content.
        let html = "<p>safe\u{202E}drowssap</p>";
        let out = run(html);
        assert!(!out.contains('\u{202E}'));
    }

    #[test]
    fn drops_event_handler_attributes() {
        let out = run(r#"<button onclick="alert(1)" onmouseover="x()">click</button>"#);
        assert!(!out.contains("onclick"));
        assert!(!out.contains("onmouseover"));
        assert!(!out.contains("alert"));
        assert!(out.contains("click"));
    }

    #[test]
    fn drops_data_attributes() {
        let out = run(r#"<div data-tracking="payload">visible</div>"#);
        assert!(!out.contains("data-tracking"));
        assert!(!out.contains("payload"));
        assert!(out.contains("visible"));
    }

    #[test]
    fn drops_style_attribute_after_visibility_check() {
        // A non-hiding style attribute should still be removed in
        // the final output (we don't carry style into markdown).
        let out = run(r#"<p style="color: red">visible</p>"#);
        assert!(!out.contains("color: red"));
        assert!(out.contains("visible"));
    }

    #[test]
    fn keeps_visible_text_intact() {
        let out = run("<h1>Title</h1><p>Body paragraph with <em>emphasis</em>.</p>");
        assert!(out.contains("Title"));
        assert!(out.contains("Body paragraph"));
        assert!(out.contains("emphasis"));
    }

    #[test]
    fn malformed_input_does_not_panic() {
        // html5ever is permissive; even garbage shouldn't crash us.
        let _ = strip_invisible("<<<<not really html<><><>");
        let _ = strip_invisible("");
        let _ = strip_invisible("<p>unclosed");
    }
}
