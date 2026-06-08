//! Stage 1 — extract main content.
//!
//! Discard the page chrome (nav, header, footer, sidebars, ads,
//! related-content widgets). Most injection attacks live in chrome,
//! not body content — sites bolt a hijacked widget onto a thousand
//! pages by editing one template, then stuff payloads in the bolted
//! widget. Ditching chrome alone removes ~70% of injection real
//! estate per the design doc.
//!
//! ## Approach
//!
//! Heuristic, not Mozilla Readability. The doc lists `readability`
//! as an option; we go with a smaller in-tree heuristic instead so
//! the strainer's behavior stays predictable and we don't pull in a
//! full readability dep tree for a 30-line job. If page-by-page
//! extraction quality becomes a problem, a future revision can swap
//! in `readability` behind the same `extract_main_content` API.
//!
//! Selection order (first hit wins):
//!
//!   1. `<article>` — the semantic main-content element. When
//!      present, almost always the right answer.
//!   2. `[role="main"]` — accessibility convention.
//!   3. `<main>` — HTML5 main-content element.
//!   4. `<body>` minus chrome (`<nav>`, `<header>`, `<footer>`,
//!      `<aside>`) — fallback when the page has no semantic
//!      structure.
//!
//! In all cases we re-emit the chosen subtree as HTML so Stage 2
//! has a clean fragment to walk.

use scraper::{Html, Selector};

use crate::types::{StrainError, StrainResult};

pub fn extract_main_content(html: &str) -> StrainResult<String> {
    extract_main_content_with_anchor(html, None)
}

/// Variant that narrows the extracted subtree to a specific anchor
/// (URL fragment) when one is supplied. Resolution order:
///
///   1. Pick the main-content subtree as `extract_main_content` does.
///   2. If `anchor` is `Some`, look for an element with `id=<anchor>`
///      INSIDE the picked subtree. If found, narrow to that element's
///      outer HTML.
///   3. If the anchor isn't found inside the main subtree (or is
///      `None`), return the main subtree unchanged.
///
/// We scope the anchor lookup to the picked subtree (not the whole
/// document) so an anchor that points at a chrome element doesn't
/// accidentally pull chrome back into the strained output. If an
/// anchor isn't reachable from the main content, the strainer treats
/// the URL as if no anchor was supplied — same conservative shape
/// as fragments getting stripped on HTTP fetch.
pub fn extract_main_content_with_anchor(html: &str, anchor: Option<&str>) -> StrainResult<String> {
    let doc = Html::parse_document(html);

    // 1. <article>
    let main = if let Some(out) = first_match(&doc, "article") {
        out
    } else if let Some(out) = first_match(&doc, "[role=\"main\"]") {
        out
    } else if let Some(out) = first_match(&doc, "main") {
        out
    } else {
        extract_body_minus_chrome(html)?
    };

    // No anchor → return main as-is (legacy path).
    let Some(anchor) = anchor.filter(|a| !a.is_empty()) else {
        return Ok(main);
    };

    // Anchor lookup INSIDE the main subtree only.
    match narrow_to_anchor(&main, anchor) {
        Some(narrowed) => {
            tracing::debug!(
                target: "ordo_strainer",
                anchor,
                chars_main = main.len(),
                chars_narrowed = narrowed.len(),
                "stage 1 narrowed to anchor"
            );
            Ok(narrowed)
        }
        None => {
            tracing::debug!(
                target: "ordo_strainer",
                anchor,
                "stage 1 anchor not found in main; returning full main"
            );
            Ok(main)
        }
    }
}

fn narrow_to_anchor(html_subtree: &str, anchor: &str) -> Option<String> {
    // CSS attribute selector — escape the anchor minimally. CSS spec
    // allows alphanumerics + `-` + `_` unescaped; anything else gets
    // ignored to keep the selector well-formed. (An attacker who
    // controls the anchor can't inject a selector this way; worst
    // case is the lookup returns None and we fall through.)
    let safe: String = anchor
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if safe.is_empty() {
        return None;
    }
    let selector_str = format!("#{safe}");
    let frag = Html::parse_fragment(html_subtree);
    let sel = Selector::parse(&selector_str).ok()?;
    let el = frag.select(&sel).next()?;
    Some(el.html())
}

fn first_match(doc: &Html, sel: &str) -> Option<String> {
    let selector = Selector::parse(sel).ok()?;
    let el = doc.select(&selector).next()?;
    Some(el.html())
}

/// Re-emit `<body>` content minus chrome elements. We need to walk
/// the DOM here because `body.html()` would include the chrome we
/// want to drop. Implemented as a fresh fragment built from body's
/// children, skipping chrome tags entirely.
fn extract_body_minus_chrome(html: &str) -> StrainResult<String> {
    const CHROME: &[&str] = &["nav", "header", "footer", "aside"];

    let doc = Html::parse_document(html);
    let body_sel =
        Selector::parse("body").map_err(|e| StrainError::Parse(format!("body selector: {e:?}")))?;
    let body = doc.select(&body_sel).next();
    let root = match body {
        Some(b) => b,
        // No <body> at all — return the whole document as fragment
        // (the parser may still have built useful content).
        None => return Ok(html.to_string()),
    };

    let mut out = String::with_capacity(html.len());
    for child in root.children() {
        if let Some(el) = child.value().as_element() {
            if CHROME.contains(&el.name()) {
                continue;
            }
            // Re-emit this child subtree by serializing via scraper's
            // ElementRef. We build an ElementRef from the node and
            // call .html(). For non-element children (text, etc.)
            // we emit text content directly.
            if let Some(eref) = scraper::ElementRef::wrap(child) {
                out.push_str(&eref.html());
            }
        } else if let scraper::node::Node::Text(t) = child.value() {
            out.push_str(&t.text);
        }
    }

    if out.trim().is_empty() {
        // Nothing salvageable — fall back to the raw input. Stage 2
        // is paranoid enough to handle a full document.
        return Ok(html.to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_narrows_to_matching_id() {
        let html = r#"
            <html><body>
              <article>
                <h1>Top</h1>
                <section id="intro"><p>Intro body.</p></section>
                <section id="api"><h2>API</h2><p>API body.</p></section>
                <section id="tools"><h2>Tools</h2><p>Tools body.</p></section>
              </article>
            </body></html>
        "#;
        let out = extract_main_content_with_anchor(html, Some("tools")).expect("ok");
        // Narrowed to the #tools section only.
        assert!(out.contains("Tools body."));
        // Other sections excluded.
        assert!(!out.contains("Intro body."));
        assert!(!out.contains("API body."));
    }

    #[test]
    fn anchor_not_found_falls_through_to_main() {
        let html = r#"
            <html><body>
              <article>
                <p>Real content.</p>
              </article>
            </body></html>
        "#;
        let out = extract_main_content_with_anchor(html, Some("nonexistent")).expect("ok");
        // Anchor not present → return full article body.
        assert!(out.contains("Real content."));
    }

    #[test]
    fn anchor_in_chrome_does_not_pull_chrome_back() {
        // Anchor lookup is scoped to the main subtree, NOT the
        // full document. If a malicious URL points at #nav, the
        // anchor lookup misses (because the chrome already got
        // dropped at Stage 1) and we fall through to main.
        let html = r#"
            <html><body>
              <nav id="nav"><p>NAV CONTENT — should not appear</p></nav>
              <article>
                <p>Real article.</p>
              </article>
            </body></html>
        "#;
        let out = extract_main_content_with_anchor(html, Some("nav")).expect("ok");
        assert!(out.contains("Real article."));
        assert!(!out.contains("NAV CONTENT"));
    }

    #[test]
    fn anchor_with_disallowed_chars_is_ignored() {
        // Selector-injection guard: if the fragment contains
        // characters outside [a-zA-Z0-9_-], the lookup short-
        // circuits and we fall through to main. An attacker who
        // controls the URL can't break the CSS selector parser
        // this way.
        let html = r#"
            <article><p>Body.</p></article>
        "#;
        let out = extract_main_content_with_anchor(html, Some("a]b{c}")).expect("ok");
        assert!(out.contains("Body."));
    }

    #[test]
    fn empty_anchor_treated_as_no_anchor() {
        let html = r#"
            <article><p>Body.</p></article>
        "#;
        let out = extract_main_content_with_anchor(html, Some("")).expect("ok");
        assert!(out.contains("Body."));
    }

    #[test]
    fn picks_article_when_present() {
        let html = r#"
            <html><body>
              <nav>nav</nav>
              <article><h1>Real</h1><p>Body.</p></article>
              <footer>footer</footer>
            </body></html>
        "#;
        let out = extract_main_content(html).expect("extract");
        assert!(out.contains("Real"));
        assert!(out.contains("Body."));
        assert!(!out.contains("nav"));
        assert!(!out.contains("footer"));
    }

    #[test]
    fn picks_role_main_when_no_article() {
        let html = r#"
            <html><body>
              <nav>nav</nav>
              <div role="main"><h1>RoleMain</h1></div>
              <footer>footer</footer>
            </body></html>
        "#;
        let out = extract_main_content(html).expect("extract");
        assert!(out.contains("RoleMain"));
        assert!(!out.contains("nav"));
    }

    #[test]
    fn picks_main_element() {
        let html = r#"
            <html><body>
              <header>h</header>
              <main><h1>MainEl</h1></main>
              <footer>f</footer>
            </body></html>
        "#;
        let out = extract_main_content(html).expect("extract");
        assert!(out.contains("MainEl"));
        assert!(!out.contains("<header>"));
    }

    #[test]
    fn falls_back_to_body_minus_chrome() {
        let html = r#"
            <html><body>
              <nav>nav links</nav>
              <header>site header</header>
              <div><h1>Soup</h1><p>Some content body.</p></div>
              <aside>related</aside>
              <footer>copyright</footer>
            </body></html>
        "#;
        let out = extract_main_content(html).expect("extract");
        assert!(out.contains("Soup"));
        assert!(out.contains("Some content body."));
        assert!(!out.contains("nav links"));
        assert!(!out.contains("site header"));
        assert!(!out.contains("related"));
        assert!(!out.contains("copyright"));
    }

    #[test]
    fn handles_pages_with_no_semantic_chrome() {
        let html = "<html><body><p>just paragraphs</p><p>and more</p></body></html>";
        let out = extract_main_content(html).expect("extract");
        assert!(out.contains("just paragraphs"));
        assert!(out.contains("and more"));
    }
}
