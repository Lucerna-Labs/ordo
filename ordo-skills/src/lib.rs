//! Skill metadata model + discovery for Ordo `SKILL.md` playbooks.
//!
//! A "skill" is a markdown file at `<root>/<id>/skill.md` (or `SKILL.md`)
//! carrying self-describing metadata used to route it to assistant modes. This
//! crate is the foundation for hybrid skill routing (see
//! `docs/skill-routing.md`): it turns the messy on-disk frontmatter into one
//! typed [`SkillManifest`]. The *routing decision* lives in `ordo-modes`
//! (`allows_skill`), which consumes these fields; this crate only models and
//! discovers — it makes no policy decision and grants no authority.
//!
//! Parsing is deliberately TOLERANT of the three formats found in the wild:
//!   1. a top YAML frontmatter block delimited by `---` lines;
//!   2. a bare `lane: <label>` line (and similar single-token keys) at the very
//!      top, before the first heading;
//!   3. fenced ```` ```yaml ```` blocks in the body (the de-facto
//!      `## Installation Metadata` / `## Mode Assignment Guidance` convention
//!      carrying `id`, `category`, `available_to_modes`, `risk_level`, …).
//!
//! Unknown keys are ignored; missing keys take safe defaults. The parser never
//! fails — a malformed skill yields a manifest with whatever could be read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Coarse risk rank used by a mode's `max_skill_risk` veto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    /// Monotonic rank for ceiling comparisons (`low` < `medium` < `high`).
    pub fn rank(self) -> u8 {
        match self {
            RiskLevel::Low => 0,
            RiskLevel::Medium => 1,
            RiskLevel::High => 2,
        }
    }

    /// Parse a free-text risk level. Returns `None` for anything unrecognized
    /// so callers can fall back to the default rather than guess.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Some(RiskLevel::Low),
            "medium" | "med" | "moderate" => Some(RiskLevel::Medium),
            "high" => Some(RiskLevel::High),
            _ => None,
        }
    }
}

impl Default for RiskLevel {
    fn default() -> Self {
        // Unmarked skills are treated as medium risk: not trusted enough to
        // bypass a low-ceiling isolation mode, not so high they're vetoed
        // everywhere.
        RiskLevel::Medium
    }
}

/// The normalized metadata for one skill. Field provenance is documented in
/// `docs/skill-routing.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Stable id — the skill's directory name (always wins over any `id:` key).
    pub id: String,
    /// Display name (`name:` frontmatter, else `id`).
    pub name: String,
    /// What it is / when to use it (`description:`, else the `## Loader Hook` or
    /// `## Purpose` section body).
    pub description: String,
    /// Routing tags (`category` + `tags`), deduped.
    pub tags: Vec<String>,
    /// Modes the skill SELF-DECLARES for (`available_to_modes`). Empty means the
    /// skill made no declaration — admission then falls to the mode's default.
    pub modes: Vec<String>,
    /// Risk level (`risk_level`), defaults to [`RiskLevel::Medium`].
    pub risk_level: RiskLevel,
    /// Whether the skill says it needs tools (`requires_tools`). Informational.
    pub requires_tools: bool,
    /// UI grouping label (`lane:`), defaults to "Installed Skills".
    pub lane_label: String,
    /// Absolute path to the parsed `skill.md`, when produced by [`discover_skills`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl SkillManifest {
    /// Parse a skill's markdown into a manifest. `id` is the directory name and
    /// always wins as the canonical id. Never fails.
    pub fn from_markdown(id: &str, content: &str) -> Self {
        let mut scalars: BTreeMap<String, String> = BTreeMap::new();
        let mut lists: BTreeMap<String, Vec<String>> = BTreeMap::new();

        // (1) top frontmatter block
        if let Some(block) = frontmatter_block(content) {
            parse_yaml_subset(block, &mut scalars, &mut lists);
        }
        // (2) leading bare `key: value` lines (e.g. `lane: ...`)
        parse_yaml_subset(leading_bare_lines(content), &mut scalars, &mut lists);
        // (3) every fenced ```yaml block
        for block in yaml_fenced_blocks(content) {
            parse_yaml_subset(&block, &mut scalars, &mut lists);
        }

        let name = scalars
            .get("name")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| id.to_string());

        let description = scalars
            .get("description")
            .filter(|s| !s.is_empty())
            .cloned()
            .or_else(|| markdown_section(content, "## Loader Hook"))
            .or_else(|| markdown_section(content, "## Purpose"))
            .unwrap_or_default();

        let mut tags = lists.get("category").cloned().unwrap_or_default();
        tags.extend(lists.get("tags").cloned().unwrap_or_default());
        dedupe(&mut tags);

        let mut modes = lists.get("available_to_modes").cloned().unwrap_or_default();
        modes.extend(lists.get("modes").cloned().unwrap_or_default());
        dedupe(&mut modes);

        let risk_level = scalars
            .get("risk_level")
            .and_then(|s| RiskLevel::parse(s))
            .unwrap_or_default();

        let requires_tools = scalars
            .get("requires_tools")
            .map(|s| matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "1"))
            .unwrap_or(false);

        let lane_label = scalars
            .get("lane")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| "Installed Skills".to_string());

        SkillManifest {
            id: id.to_string(),
            name,
            description,
            tags,
            modes,
            risk_level,
            requires_tools,
            lane_label,
            path: None,
        }
    }
}

/// Scan `root` for skill directories, parsing each `skill.md` / `SKILL.md` into
/// a [`SkillManifest`]. Results are sorted by id and deduped (first wins).
/// Returns an empty vec if `root` does not exist.
pub fn discover_skills(root: &Path) -> std::io::Result<Vec<SkillManifest>> {
    let mut out: Vec<SkillManifest> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let id = match dir.file_name().and_then(|n| n.to_str()) {
            Some(name) if !name.trim().is_empty() => name.trim().to_string(),
            _ => continue,
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let skill_path = skill_file_in(&dir);
        let Some(skill_path) = skill_path else {
            continue;
        };
        let content = match std::fs::read_to_string(&skill_path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let mut manifest = SkillManifest::from_markdown(&id, &content);
        manifest.path = Some(skill_path.display().to_string());
        out.push(manifest);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

fn skill_file_in(dir: &Path) -> Option<PathBuf> {
    for candidate in ["skill.md", "SKILL.md"] {
        let path = dir.join(candidate);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

// ── parsing helpers ──────────────────────────────────────────────────────────

/// Return the inside of a leading `---\n … \n---` frontmatter block, if present.
fn frontmatter_block(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---")?;
    // require the delimiter to be its own line
    let rest = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// The contiguous run of `key: value` lines at the very top of the file, before
/// the first blank line, heading, or frontmatter fence. Captures the bare
/// `lane:` style without mis-reading prose colons elsewhere.
fn leading_bare_lines(content: &str) -> &str {
    let mut end = 0usize;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" {
            break;
        }
        // a bare metadata line is `single_token: ...`
        let is_kv = trimmed
            .split_once(':')
            .map(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(false);
        if !is_kv {
            break;
        }
        end += line.len();
    }
    &content[..end]
}

/// Every fenced ```` ```yaml ```` (or ```` ```yml ````) block body in the content.
fn yaml_fenced_blocks(content: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut current = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if in_block {
            if trimmed.starts_with("```") {
                blocks.push(std::mem::take(&mut current));
                in_block = false;
            } else {
                current.push_str(line);
                current.push('\n');
            }
        } else if trimmed == "```yaml" || trimmed == "```yml" {
            in_block = true;
            current.clear();
        }
    }
    blocks
}

/// Parse a YAML *subset* — scalar `key: value`, block lists (`key:` then
/// `  - item` lines), and flow lists (`key: [a, b]`) — into the maps. Tolerant:
/// unrecognized lines are skipped.
fn parse_yaml_subset(
    block: &str,
    scalars: &mut BTreeMap<String, String>,
    lists: &mut BTreeMap<String, Vec<String>>,
) {
    let lines: Vec<&str> = block.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        i += 1;
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("- ") {
            continue;
        }
        let Some((key_raw, rest_raw)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key_raw.trim();
        // keys are single tokens; skip prose like "Note: ..."
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let key = key.to_string();
        let rest = rest_raw.trim();

        if rest.is_empty() {
            // block list: collect following `- item` lines
            let mut items = Vec::new();
            while i < lines.len() {
                let item_line = lines[i].trim();
                if let Some(item) = item_line.strip_prefix("- ") {
                    items.push(unquote(item.trim()).to_string());
                    i += 1;
                } else {
                    break;
                }
            }
            if !items.is_empty() {
                lists.entry(key).or_default().extend(items);
            }
        } else if rest.starts_with('[') && rest.ends_with(']') {
            // flow list
            let inner = &rest[1..rest.len() - 1];
            let items: Vec<String> = inner
                .split(',')
                .map(|s| unquote(s.trim()).to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !items.is_empty() {
                lists.entry(key).or_default().extend(items);
            }
        } else {
            scalars.insert(key, unquote(rest).to_string());
        }
    }
}

/// Extract the body of a markdown section by heading, flattened to one line —
/// mirrors the studio's existing `## Loader Hook` / `## Purpose` extraction.
fn markdown_section(content: &str, heading: &str) -> Option<String> {
    let mut lines = content.lines();
    // find the heading
    for line in lines.by_ref() {
        if line.trim() == heading {
            break;
        }
    }
    let mut body: Vec<&str> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            break;
        }
        if trimmed.is_empty() || trimmed.starts_with("```") {
            continue;
        }
        body.push(trimmed);
    }
    if body.is_empty() {
        None
    } else {
        Some(body.join(" "))
    }
}

/// Remove specific mode ids from every block-style `available_to_modes:` list
/// in a skill's markdown, leaving everything else — prose, and `category:` /
/// `tags:` lists that might share a value like "orchestration" — untouched.
/// Returns `(new_content, removed_count)`.
///
/// This is the surgical edit behind the diagnostic mode's bounded "safe
/// repair": dropping phantom-mode declarations (modes that don't exist) from a
/// skill without rewriting anything else. Inline/flow lists
/// (`available_to_modes: [a, b]`) are intentionally NOT touched — those are left
/// for the operator, so the transform can never misparse one.
pub fn remove_modes_from_frontmatter(content: &str, remove: &[String]) -> (String, usize) {
    let remove_set: std::collections::BTreeSet<&str> =
        remove.iter().map(String::as_str).collect();
    let mut out = String::with_capacity(content.len());
    let mut removed = 0usize;
    let mut in_modes_block = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        if in_modes_block {
            if let Some(item) = trimmed.strip_prefix("- ") {
                if remove_set.contains(unquote(item.trim())) {
                    removed += 1;
                    continue; // drop only this list item
                }
                out.push_str(line);
                continue;
            }
            // anything that isn't a `- item` ends the block (blank line, the
            // next key, a closing fence, dedent, …).
            in_modes_block = false;
        }
        if trimmed == "available_to_modes:" {
            in_modes_block = true;
        }
        out.push_str(line);
    }
    (out, removed)
}

fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if value.len() >= 2 {
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn dedupe(values: &mut Vec<String>) {
    let mut seen = std::collections::BTreeSet::new();
    values.retain(|v| !v.is_empty() && seen.insert(v.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_frontmatter_style() {
        // ordo-build-* style: --- name/description ---
        let md = "---\nname: ordo-build-blueprint\ndescription: \"Step 2 of the pipeline. Use after intake.\"\n---\n\n# Title\n\nbody";
        let s = SkillManifest::from_markdown("ordo-build-blueprint", md);
        assert_eq!(s.id, "ordo-build-blueprint");
        assert_eq!(s.name, "ordo-build-blueprint");
        assert!(s.description.starts_with("Step 2 of the pipeline"));
        assert!(s.modes.is_empty(), "no available_to_modes declared");
        assert_eq!(s.risk_level, RiskLevel::Medium); // default
    }

    #[test]
    fn parses_lane_plus_installation_metadata_style() {
        // ordo_rust_architecture style: bare `lane:` + fenced yaml metadata.
        let md = "\
lane: Ordo Architecture

# Ordo Rust Architecture Skill

## Purpose

Teaches a model how to build Rust projects.

## Installation Metadata

```yaml
id: ordo_rust_architecture
category:
  - rust
  - architecture
risk_level: medium
requires_tools: false
available_to_modes:
  - coding
  - orchestration
  - runtime
```
";
        let s = SkillManifest::from_markdown("ordo_rust_architecture", md);
        assert_eq!(s.lane_label, "Ordo Architecture");
        assert_eq!(s.description, "Teaches a model how to build Rust projects.");
        assert!(s.tags.contains(&"rust".to_string()));
        assert!(s.tags.contains(&"architecture".to_string()));
        assert_eq!(s.modes, vec!["coding", "orchestration", "runtime"]);
        assert_eq!(s.risk_level, RiskLevel::Medium);
        assert!(!s.requires_tools);
    }

    #[test]
    fn flow_list_and_quotes_and_high_risk() {
        let md = "---\nname: x\ncategory: [\"sec\", 'exploit']\nrisk_level: high\navailable_to_modes: [security]\n---\n";
        let s = SkillManifest::from_markdown("x", md);
        assert_eq!(s.tags, vec!["sec", "exploit"]);
        assert_eq!(s.modes, vec!["security"]);
        assert_eq!(s.risk_level, RiskLevel::High);
    }

    #[test]
    fn prose_colons_are_not_parsed_as_metadata() {
        let md = "# Title\n\nStep 2: do the thing. Note: be careful.\n\n## Purpose\n\nDo work.\n";
        let s = SkillManifest::from_markdown("p", md);
        // no spurious tags/modes from prose; description from ## Purpose
        assert!(s.tags.is_empty());
        assert!(s.modes.is_empty());
        assert_eq!(s.description, "Do work.");
        assert_eq!(s.lane_label, "Installed Skills");
    }

    #[test]
    fn risk_rank_orders_correctly() {
        assert!(RiskLevel::Low.rank() < RiskLevel::Medium.rank());
        assert!(RiskLevel::Medium.rank() < RiskLevel::High.rank());
    }

    #[test]
    fn discover_scans_a_directory() {
        let base = std::env::temp_dir().join("ordo-skills-discover-test");
        let _ = std::fs::remove_dir_all(&base);
        let alpha = base.join("alpha");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::write(
            alpha.join("skill.md"),
            "---\nname: Alpha\navailable_to_modes: [coding]\n---\n# Alpha",
        )
        .unwrap();
        // a non-skill dir (no skill.md) is ignored
        std::fs::create_dir_all(base.join("empty")).unwrap();

        let found = discover_skills(&base).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "alpha");
        assert_eq!(found[0].name, "Alpha");
        assert_eq!(found[0].modes, vec!["coding"]);
        assert!(found[0].path.as_deref().unwrap().ends_with("skill.md"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_root_yields_empty() {
        let nope = std::env::temp_dir().join("ordo-skills-does-not-exist-xyz");
        let _ = std::fs::remove_dir_all(&nope);
        assert!(discover_skills(&nope).unwrap().is_empty());
    }

    #[test]
    fn remove_modes_drops_only_available_to_modes_entries() {
        // `orchestration` appears in BOTH category and available_to_modes — the
        // repair must drop it ONLY from available_to_modes.
        let md = "\
lane: X

## Installation Metadata

```yaml
category:
  - rust
  - orchestration
available_to_modes:
  - coding
  - orchestration
  - runtime
  - research
```
";
        let (out, removed) =
            remove_modes_from_frontmatter(md, &["orchestration".into(), "runtime".into()]);
        assert_eq!(removed, 2);
        // re-parse: modes lost the phantoms, tags KEPT orchestration
        let s = SkillManifest::from_markdown("x", &out);
        assert_eq!(s.modes, vec!["coding", "research"]);
        assert!(s.tags.contains(&"orchestration".to_string()));
        assert!(s.tags.contains(&"rust".to_string()));
    }

    #[test]
    fn remove_modes_handles_two_blocks_and_quotes() {
        let md = "\
## Mode Assignment Guidance

```yaml
available_to_modes:
  - coding
  - legal_admin
```

## Installation Metadata

```yaml
available_to_modes:
  - \"coding\"
  - 'legal_admin'
```
";
        let (out, removed) = remove_modes_from_frontmatter(md, &["legal_admin".into()]);
        assert_eq!(removed, 2, "both blocks' phantom entry removed");
        assert_eq!(SkillManifest::from_markdown("x", &out).modes, vec!["coding"]);
    }

    #[test]
    fn remove_modes_noop_when_nothing_matches() {
        let md = "---\nname: x\navailable_to_modes: [inline, list]\n---\n";
        let (out, removed) = remove_modes_from_frontmatter(md, &["inline".into()]);
        // inline/flow lists are intentionally untouched
        assert_eq!(removed, 0);
        assert_eq!(out, md);
    }
}
