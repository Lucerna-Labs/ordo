# Ordo UXI Design Specification

This is the design source of truth for `ordo-uxi`. Every surface, every region, every visual element is defined here in a form that translates directly to Vello layout code.

**Use this document, not `index.html`/`styles.css`, when wiring a surface.** The static HTML/CSS is the original visual reference; this document is what HTML/CSS would look like if it were written for a Vello renderer that needs absolute coordinates. The math has already been done.

---

## Part 1: Layout system

### How anchors work

Every region in the UXI is positioned with an **anchor rule** — a description of where it lives relative to its parent. Anchors are computed at render time from the current window size, so layouts reflow correctly when the window resizes.

A region is a rectangle. Its position and size come from applying its anchor rule to the parent's current rectangle. Children of the region then anchor to the region's rectangle. Layout cascades top-down: the root is the window itself.

### Anchor primitives

| Anchor | Meaning | Fixed dimensions needed |
|---|---|---|
| `fill` | Region occupies entire parent | none |
| `edge:top` | Hugs top edge, full parent width | `height` |
| `edge:bottom` | Hugs bottom edge, full parent width | `height` |
| `edge:left` | Hugs left edge, full parent height | `width` |
| `edge:right` | Hugs right edge, full parent height | `width` |
| `corner:top-left` | Top-left corner of parent | `width`, `height` |
| `corner:top-right` | Top-right corner | `width`, `height` |
| `corner:bottom-left` | Bottom-left corner | `width`, `height` |
| `corner:bottom-right` | Bottom-right corner | `width`, `height` |
| `fill-between(left=A, right=B)` | Fills horizontal space between siblings A and B | `height` (or matches parent) |
| `fill-below(A)` | Fills vertical space below sibling A | `width` (or matches parent) |

### Computing a region

Given a parent rectangle `{x, y, w, h}` and an anchor rule, here's how to compute the region's rectangle:

```
fill:                    { x, y, w, h }
edge:top, H:             { x, y, w, H }
edge:bottom, H:          { x, y + h - H, w, H }
edge:left, W:            { x, y, W, h }
edge:right, W:           { x + x + w - W, y, W, h }
corner:top-left, W, H:   { x, y, W, H }
corner:top-right, W, H:  { x + w - W, y, W, H }
fill-between(A, B):      { x: A.right, y, w: B.left - A.right, h }
fill-below(A):           { x, y: A.bottom, w, h: h - A.height }
```

(These are the actual math; the model can transcribe them directly into Rust helper functions.)

### Padding

Padding inside a region shrinks the rectangle children get. `padding: { left: 22, right: 22, top: 0, bottom: 0 }` on a region with rectangle `{0, 0, 800, 82}` produces an inner rectangle of `{22, 0, 756, 82}` for children to anchor against.

### Child positions inside a region

Some children don't need anchors — they have fixed positions inside their parent's inner rectangle (the rectangle after padding). For these, the spec gives an `(x, y)` offset from the parent's inner top-left, plus a `width`/`height`.

---

## Part 2: Design tokens

These are constants. Every surface references them by name. The model defines them once in `ordo-uxi/src/theme.rs` (already done in Cycle 1) and reuses them everywhere.

### Colors

```
INK              #0a0c10   primary background
INK_2            #0e1117   secondary background
PANEL            #13161c   card/panel background
PANEL_RAISED     #181c24   elevated card background
PARCHMENT        #f4ecd8   primary text
MUTED            #f4ecd8 at 60% alpha   secondary text
DIM              #f4ecd8 at 40% alpha   tertiary text, labels
LINE             #ffffff at  7% alpha   subtle borders
LINE_STRONG      #ffffff at 12% alpha   stronger borders, focus
GOLD             #f4c95d   primary accent, active state, send button
GOLD_SOFT        #f4c95d at 14% alpha   accent backgrounds
JADE             #7fd1c5   ok signal, status dot
VIOLET           #a99af0   reserved (currently unused in shell)
DANGER           #e85d5d   error, critical state
```

### Glow colors (for dots and accents)

Vello 0.6 doesn't have blur. Where the CSS uses `box-shadow` for glow, the spec specifies a halo: a larger transparent circle behind the solid one, in the same hue at reduced opacity. The model paints two circles, one slightly bigger, the back one at the halo opacity.

```
JADE_HALO        JADE at 35% alpha, radius = dot_radius * 2
GOLD_HALO        GOLD at 35% alpha, radius = dot_radius * 2
```

### Typography

Three fonts, embedded in `ordo-uxi/assets/`:

```
SERIF       Fraunces-Regular.ttf       headings, body text, message content, nav items
MONO        JetBrainsMono-Regular.ttf  labels, metadata, breadcrumbs, signal labels, tags
SANS        Sora-Regular.ttf           default UI text (currently used minimally; reserved)
```

### Type scale

```
LABEL_TINY       MONO    9px   uppercase, letter-spacing 0.18em (1.62px)
LABEL_SMALL      MONO   10px   uppercase, letter-spacing 0.18em (1.8px)
LABEL_MED        MONO   11px   uppercase, letter-spacing 0.05em (0.55px) — for the profile ".4141"
META             MONO    9px   uppercase, message timestamps, tags
NAV_ITEM        SERIF   14px   sidebar nav labels
BODY            SERIF   13px   message body, dock summary (italic when summary)
BODY_LARGE      SERIF   15px   message card body
LEDE            SERIF   16px   italic, hero subtitle
HEADING_BRAND   SERIF   24px   "Ordo" wordmark, letter-spacing -0.02em
HEADING_HERO    SERIF   42px   hero headline (CSS clamps to 32–54px; pick 42 as default)
MINI_TITLE      SERIF   20px   mini-card titles
```

### Spacing scale

The CSS uses a mix of pixel and rem values. For the spec, all spacing is in pixels.

```
SPACE_1     2px
SPACE_2     4px
SPACE_3     8px      gap between tight elements
SPACE_4    10px      gap inside controls
SPACE_5    12px      standard inner gap
SPACE_6    16px      standard between-section gap
SPACE_7    20px
SPACE_8    24px      generous gap, padding
SPACE_9    28px
SPACE_10   32px
SPACE_LG   38px      hero top padding
```

### Radii

```
RADIUS_SM      4px      tags, inputs (small)
RADIUS_MED     8px      nav items
RADIUS_LG     10px      composer controls
RADIUS_XL     12px      mini-cards, status card
RADIUS_2XL    14px      message cards
RADIUS_PILL  999px      gold rail accent, dots
```

### Borders

Standard border is `1px solid LINE` unless specified. The model draws this as a stroked rectangle inset by 0.5px on each side to keep the line on-pixel.

---

## Part 3: Root layout

The window's root rectangle is the entire window. Three top-level regions cascade from it.

```
Region: window-root
  Anchor: fill (the window itself)
  Background: gradient (see below)
  Children:
    - topbar
    - sidebar
    - workspace
```

### Window background

The CSS uses three layered radial/linear gradients to produce a subtle atmospheric background. In Vello, paint the window with a single linear gradient from `#07090d` (top-left) to `#0a0c10` (center) to `#090b10` (bottom-right). The radial highlights (gold at top-left, jade at top-right) are optional polish — skip in early cycles.

Above the gradient, paint a 24×24 dot grid texture at very low alpha (rgba 255/255/255/2.5%). Implementation: a tiled small rectangle pattern or a Vello fill with a tiled image brush. Skip in early cycles if not trivial.

---

## Part 4: Topbar

**Status:** Wired in Cycle 1. This section documents the design as built, so future modifications have a stable spec.

```
Region: topbar
  Anchor: edge:top
  Height: 82px
  Background: INK at 88% alpha (no backdrop blur — Vello can't)
  Border: 1px solid LINE on bottom edge

  Children:
    - brand
    - signals-strip
    - profile
```

### Brand region

```
Region: brand
  Anchor: corner:top-left (of topbar's inner rect)
  Width: 188
  Height: 82
  Padding: { left: 22, right: 0, top: 0, bottom: 0 }

  Layout: horizontal, lamp glyph + brand copy, gap = SPACE_5 (12)
  Vertically centered
```

**Lamp glyph** — fixed-position child of brand:
```
Position: { x: 0, y: (region_height - 28) / 2 } = { 0, 27 }
Size: 28 × 28
Paint:
  - Outer halo: circle radius 14, fill GOLD at 38% alpha
  - Mid ring: circle radius 11, fill GOLD at 70% alpha
  - Inner dot: circle radius 5, fill GOLD solid
  (no blur; the layered circles approximate the radial gradient)
```

**Brand copy** — fixed-position child of brand:
```
Position: { x: 28 + SPACE_5, y: (region_height - text_block_height) / 2 }
  where text_block_height = 9 + 2 + 24 = 35
  so y ≈ 23

Stack: two text rows
  Row 1: "LUCERNA LABS"  style: LABEL_TINY, color: PARCHMENT at 45% alpha
  Row 2: "Ordo"          style: HEADING_BRAND, color: PARCHMENT
Vertical gap between rows: 2px
```

### Profile region

```
Region: profile
  Anchor: corner:top-right (of topbar's inner rect)
  Width: 120 (sized to content; adjust if labels change)
  Height: 82
  Padding: { left: 0, right: 22, top: 0, bottom: 0 }

  Layout: vertical stack, right-aligned
  Vertically centered
```

**Profile copy** — fixed-position child:
```
Right-anchored stack:
  Row 1: "STANDARD"   style: LABEL_TINY, color: DIM
  Row 2: ".4141"      style: LABEL_MED, color: MUTED, font-weight slightly heavier
Vertical gap: 1px
Both lines right-aligned to the region's inner right edge
```

(Both strings are currently hardcoded with TODOs. ".4141" should eventually pull from `RuntimeConfig.control_api_port`. "STANDARD" should eventually pull from the active runtime mode via `ordo-modes`.)

### Signals strip

```
Region: signals-strip
  Anchor: fill-between(left=brand, right=profile)
  Height: 82
  Padding: { left: 24, right: 24, top: 0, bottom: 0 }

  Layout: horizontal row of seven signal cells
  Gap between cells: 22px (was clamp(14, 3.2vw, 30) in CSS — fixed value chosen)
  Vertically centered
```

**Seven signal cells**, in order: Gateway, Bus, Vault, MCP, Embed, Heal, LLM.

Each signal cell:
```
Width: sized to content (dot + gap + label)
Height: matches region height, content vertically centered

Layout inside cell:
  - Dot: circle radius 3.5, fill JADE (ok) or GOLD (warn)
    Behind dot: halo circle radius 7, fill JADE_HALO or GOLD_HALO
  - Gap: 9px
  - Label: LABEL_SMALL, color: PARCHMENT at 70% alpha
```

Signal state mapping (current as of Cycle 1):
```
Gateway   ok        TODO: no protocol message yet
Bus       ok        TODO: no protocol message yet
Vault     dynamic   warn when SecretsSealTierDegraded received (sticky until restart)
MCP       dynamic   warn when McpClientAuthDegraded received (sticky until restart)
Embed     ok        TODO: no protocol message yet
Heal      dynamic   warn when SelfHealRequested with urgency >= High (sticky until restart)
LLM       warn      TODO: no protocol message yet (hardcoded warn for now)
```

---

## Part 5: Sidebar

**Status:** Not yet wired. This is the Cycle 2 spec.

```
Region: sidebar
  Anchor: corner:bottom-left of window-root, with height = window_height - topbar_height
  Width: 188
  Height: window_height - 82
  (Equivalent anchor: edge:left, width 188, but starts below the topbar — implementation
   detail: pass `window_root` as parent and compute `y = topbar.bottom`, `h = window_h - topbar.bottom`)

  Background: INK at 52% alpha
  Border: 1px solid LINE on right edge

  Padding: { left: 8, right: 8, top: 18, bottom: 20 }

  Children: vertical stack of two sections, gap between sections = 28
```

### Section structure

Each section has a header label and a vertical list of nav items.

```
Section header:
  Text style: LABEL_TINY, color: PARCHMENT at 32% alpha
  Position: 12px from left edge of section (matches nav item padding)
  Bottom margin: 10px before first nav item
```

### Nav items

```
Nav item rectangle:
  Width: full section width (after sidebar padding) = 188 - 8 - 8 = 172
  Height: 34
  Padding: { left: 12, right: 12, top: 8, bottom: 8 }
  Layout inside: horizontal, icon + gap + label
  Gap: 11px

Background:
  Default: transparent
  Hover: linear gradient left-to-right, GOLD at 14% alpha → white at 2.5% alpha, RADIUS_MED
  Active: same as hover

Active accent (left rail):
  When item is active: paint a 2px-wide pill on the left edge of the item
  Position: { x: 0, y: item.y + 5, w: 2, h: item.h - 10 }
  Color: GOLD
  No glow yet (Vello can't blur) — TODO: layered halo strokes

Icon:
  Size: 15 × 15
  Currently a simple rounded square outline in CSS (1.5px border, RADIUS_SM, opacity 0.8)
  In Vello: stroked rectangle, color = current text color, stroke width 1.5
  Each icon variant has slightly different shape — see icon catalog below

Label:
  Style: NAV_ITEM (SERIF 14px)
  Color: PARCHMENT at 74% alpha (default)
  Color: PARCHMENT (hover, active)
  Vertical center inside item
```

### Nav item list

**Section 1 — "PRIMARY":**

| Order | Label | Icon variant | Active by default |
|---|---|---|---|
| 1 | Provider | cloud | no |
| 2 | Assistant | chat | **yes** |
| 3 | Review | eye | no |

**Section 2 — "AGENT":**

| Order | Label | Icon variant |
|---|---|---|
| 1 | Skills | spark |
| 2 | Persona | user |
| 3 | Agent Persona | bot |
| 4 | Agent Memory | book |
| 5 | Apps | boxes |
| 6 | Webhooks | hook |
| 7 | Plugins | plug |
| 8 | MCP | server |

Diagnostic UXI note:

```text
Diagnostic mode is the operator-requested maintenance surface for peripheral
components. When approved tools are available, it can install, delete, repair,
trust, quarantine, or re-authorize MCP servers, skills, plugins, provider
profiles, and related integrations on behalf of the user. The UXI must make
those actions explicit and should never present diagnostic mode as permission
to silently mutate Ordo's core runtime, security, hook, credential, or UXI
boundaries.
```

### Icon catalog

Icons are currently simple geometric shapes — rounded rectangles, circles, transformed shapes. Each is a 15×15 box rendered with stroked geometry. For Cycle 2, render each as a placeholder rectangle. Replace with custom paths in a later polish cycle.

For Cycle 2 placeholder rendering, use `RADIUS_SM` rounded rectangles for all icons. Future cycles refine individual glyphs (the eye, the lamp variations, the plug, etc.). Don't block Cycle 2 on icon fidelity.

### Active state behavior

For Cycle 2, the active item is hardcoded to "Assistant" (matching the static HTML reference). Clicking nav items to change the active state is a later cycle (probably Cycle 7, after workspace surfaces are wired). The active state is internal `ordo-uxi` state — a `NavItem` enum field on `UxiApp`. No bus message yet.

---

## Part 6: Workspace

**Status:** Not yet wired. Cycle 3+.

```
Region: workspace
  Anchor: fill-between (in 2D — fill the area not occupied by topbar and sidebar)
  Practical computation:
    x = sidebar.right
    y = topbar.bottom
    w = window_width - sidebar.width
    h = window_height - topbar.height

  Background: inherits window background gradient
  Padding: { left: 26, right: 26, top: 38, bottom: 20 }

  Children: vertical stack
    - hero (auto-height)
    - assistant-grid (fills remaining space above dock)
    - dock (auto-height, anchored to bottom)
```

### Hero region

```
Region: hero
  Anchor: edge:top of workspace-inner
  Height: auto (sized to content; ~140px when populated)
  Max-width: 1040 (centered within workspace if window is wider)

  Layout: two columns, headline-block | status-card
  Column gap: 24
  Right column (status-card) is auto-width
  Left column fills remaining space
```

**Headline block** (left column):
```
Vertical stack:
  Row 1: Breadcrumb
    Text: "Ordo • Planner"
    Style: LABEL_SMALL (10px), color: PARCHMENT at 35% alpha
    Margin bottom: 10

  Row 2: Headline
    Text: "The conversation is the control surface"
    Style: HEADING_HERO (42px), color: PARCHMENT
    Letter-spacing: 0
    Line-height: 0.96
    Max-width: 620

  Row 3: Lede
    Text: "Everything Ordo does, you can ask for here. Tabs are persistent, but this is the steering wheel."
    Style: LEDE (16px italic SERIF), color: MUTED
    Max-width: 570
    Margin top: 12
    Line-height: 1.5
```

**Status card** (right column):
```
Region: status-card
  Width: auto (~190px)
  Padding: { left: 14, right: 14, top: 12, bottom: 12 }
  Border: 1px solid LINE
  Background: PANEL at 74% alpha
  Radius: RADIUS_XL

  Layout: horizontal, dot + gap + label
  Gap: 10

  Dot:
    Size: 8 × 8 circle
    Color: JADE
    Halo: JADE_HALO at radius 14

  Label:
    Text: "Ordo runtime online"
    Style: LABEL_SMALL (11px MONO), color: MUTED
```

The status card's state should eventually subscribe to `topics::SYSTEM_STATE` and reflect health — green dot + "Ordo runtime online" when Healthy, gold dot + "Rescue mode active" when Rescue, danger dot + "Critical" when Critical. For Cycle 3, hardcode to "online" with a TODO; wire to bus in a later cycle.

### Assistant grid

```
Region: assistant-grid
  Anchor: fill-below(hero)
  Padding: { left: 0, right: 4, top: 28, bottom: 0 }

  Layout: two columns
    - conversation-panel (fills remaining space)
    - right-rail (280 wide)
  Column gap: 22
```

### Conversation panel

```
Region: conversation-panel
  Anchor: edge:left of assistant-grid-inner
  Width: fill-remaining (after right-rail)
  Min-height: 340

  Padding: { left: 22, right: 22, top: 22, bottom: 22 }
  Border-left: 1px solid white at 4.5% alpha
  Border-right: 1px solid white at 4.5% alpha

  Scroll: vertical when content exceeds height (later cycle)

  Children: vertical stack of message cards, gap 16
```

**Message card variants** — two types, assistant and operator:

```
Message card (both variants):
  Max-width: 420 (or 100% if narrower)
  Padding: { left: 22, right: 22, top: 23, bottom: 23 }
  Border: 1px solid LINE
  Radius: RADIUS_2XL
  Background: panel gradient (linear 145°, white-3% → white-1%) over PANEL
  Shadow: drop shadow, offset (0, 20), blur 60, alpha 18% — TODO: Vello can't blur,
          use a static dim band below the card or skip

  Internal layout:
    Row 1: Message meta
      Horizontal: "ROLE" + gap + "TIME"
      Style: META, color: PARCHMENT at 42% alpha
      Gap: 12
      Bottom margin: 14

    Row 2: Body
      Text: message content
      Style: BODY_LARGE (15px SERIF), color: PARCHMENT
      Line-height: 1.55

    Row 3: Tag
      Display: inline-flex, top margin 14
      Padding: { left: 7, right: 7, top: 5, bottom: 5 }
      Border: 1px solid LINE
      Radius: RADIUS_SM
      Background: white at 3.5% alpha
      Style: LABEL_TINY at 9px, color: DIM
```

Assistant variant: left-anchored, default background.
Operator variant: right-anchored (margin-left: auto), accent background:
```
Border: 1px solid GOLD at 22% alpha
Background: linear gradient (145°, GOLD at 8% → white at 1.5%) over PANEL
```

### Right rail

```
Region: right-rail
  Anchor: edge:right of assistant-grid-inner
  Width: 280

  Layout: vertical stack of mini-cards, gap 12
```

**Mini card** (3 instances stacked):
```
Region: mini-card
  Width: full right-rail width
  Padding: { left: 17, right: 17, top: 17, bottom: 17 }
  Border: 1px solid LINE
  Radius: RADIUS_XL
  Background: PANEL at 78% alpha

  Internal layout: vertical stack
    Row 1: Mini-label
      Style: LABEL_TINY at 9px, color: DIM
      Margin bottom: 8

    Row 2: Title (strong)
      Style: MINI_TITLE (20px SERIF), color: PARCHMENT, weight 600

    Row 3: Body
      Style: BODY (12px SERIF), color: MUTED
      Line-height: 1.5
      Margin top: 8
```

The three mini-cards' content for now:
1. label="BRIEF ROUTING", title="Planner", body="Assistant requests resolve through the active runtime lane."
2. label="PERSISTENT TABS", title="15 surfaces", body="Provider, review, memory, apps, plugins, MCP, and more."
3. label="PRIMARY ACTION", title="Lamp gold", body="Focused controls and active affordances use the Ordo lamp."

(Hardcoded in Cycle 3. Eventually these become dynamic — recent activity, runtime stats, contextual help — but that's a much later cycle.)

### Dock + composer

```
Region: dock
  Anchor: edge:bottom of workspace-inner
  Max-width: 760 (right-anchored if workspace is wider)
  Height: auto (~80)

  Children: vertical stack
    - dock-summary (auto height)
    - composer (auto height)
```

**Dock summary**:
```
Layout: horizontal, three columns: role | summary | collapse-button
Padding: { left: 6, right: 6, top: 0, bottom: 10 }
Gap: 14, vertically centered

Columns:
  Role:    Text: "ASSISTANT", style: LABEL_TINY, color: DIM
  Summary: Text: latest message preview, style: BODY italic, color: MUTED
           Ellipsis-truncated if it overflows
  Collapse-button: 26 × 22, no border, transparent background, "^" glyph in DIM
```

**Composer**:
```
Layout: horizontal grid, 4 icon buttons + input + send button
Heights: all 40
Border: 1px solid LINE_STRONG
Radius: RADIUS_LG
Backgrounds: PANEL at 95% alpha

Gap between elements: 9

Icon buttons (4, each 36 wide):
  Image, Attach, Folder, Mic
  Icon catalog same approach as sidebar (placeholder rounded rects for now)
  Color: PARCHMENT
  Hover: border GOLD at 38% alpha, background GOLD at 8% alpha

Input field:
  Fills remaining space
  Min-width: 160
  Padding: { left: 16, right: 16, top: 0, bottom: 0 }
  Style: BODY (14px SERIF), color: PARCHMENT
  Placeholder: "Tell Ordo the brief...", color: PARCHMENT at 48% alpha
  Focus: border GOLD at 45% alpha, focus-ring GOLD at 8% alpha 3px outside

Send button:
  Width: 48
  Border: transparent
  Background: GOLD
  Foreground (play triangle glyph): INK
  Shadow: drop shadow, offset (0, 12), blur 28, GOLD at 22% — Vello TODO
  Hover: slight brightness lift (later cycle)
```

Text input was non-trivial in Vello but has since landed as a reusable primitive in `ordo-uxi/src/input.rs`. The primitive covers what's needed for forms and the composer:

- Keyboard event capture from winit (`KeyEvent::text` for printable characters, `NamedKey` matching for editing keys)
- Per-field `TextInputState { buffer, cursor }` keyed by an opaque `FieldId`
- UTF-8-safe cursor moves and backspace
- Cursor positioning via skrifa glyph advances (reuses `text::layout_ascii`)
- Blink driven by winit's `ControlFlow::WaitUntil` (no extra tokio task)
- Submission via Enter, dispatched to a per-field handler on `UxiApp::on_field_submit`
- Single-focus model with click-to-focus and click-elsewhere-to-blur
- Cursor icon switches to text I-beam over input fields

Still deferred — will land in follow-up cycles as concrete surfaces demand them: text selection, paste, IME, multiline, undo/redo, ctrl+arrow word jump, ctrl-A / ctrl-Z.

The composer surface paints a static placeholder today and will adopt the primitive in a later cycle (along with the bus message it submits to). The first real consumer is the Cloud tab's Add Provider modal.

### Cloud tab

The Cloud tab is the second tab to render real bus-driven content (the Assistant tab being the first). It owns one workspace surface — the **LLM Providers panel** — with one header card, a list of configured-provider rows, and an Add/Edit modal that overlays the workspace when open.

```
Region: cloud-content
  Anchor: fill inside workspace-inner
  Max-card-width: 920 (center-anchored if workspace is wider)

  Children: vertical stack
    - header-card (auto height)
    - per-provider row × N (84 px each)
    - modal overlay (when cloud_modal.is_some())
```

**Header card** — title + subtitle + bottom row with Refresh (ghost button, left) and "+ Add provider" (primary gold pill, right). The default-provider selection happens on the row itself via the radio button, not via a separate dropdown.

**Per-provider row** (84 px tall, gold border when default, LINE otherwise):
```
Layout, left-to-right: radio | icon | name+badges+detail | Test/Edit/Delete buttons (right-anchored)

Radio:    18 px circle, hit target 30 × 28; sets default on click via
          CloudCredentialSetDefaultRequest
Icon:     36 × 36 gold-tinted glyph (placeholder; per-service icons later)
Name:     credential.label or .service (14 px SERIF, PARCHMENT)
Badges:   DEFAULT (gold) + test status (TESTING/OK/FAILED in JADE/DIM/DANGER)
Detail:   "<service>  <model>  <base_url>" (11 px MONO, MUTED)
Failed:   inline "! <message>" on a second line (10 px MONO, DANGER)
Buttons:  Test, Edit (64 × 32 ghost) and X (32 × 32 ghost, delete)
```

**Sort order** — default first, then case-insensitive alphabetical by service. Computed on every list/upsert/default-change event by `UxiApp::sort_cloud_credentials`. Operators reason about their providers by priority, not alphabetical order.

**Test result indicators** — the row carries a transient `TestState` (`Pending` / `Ok` / `Failed(message)`). Pending is set immediately when the operator clicks Test; the live state is replaced when `CloudCredentialTestResult` arrives. Indicators do not auto-decay — they stay until the next Test on the same service.

**Add/Edit modal** — opens via "+ Add provider" or row Edit. Dimmed backdrop (black at 55% alpha) spans the workspace; a centered 560 × 540 card carries the form.

Card contents, vertically stacked:
- Title (18 px SERIF, PARCHMENT) — "Add provider" or "Edit provider"
- Subtitle (11 px MONO, MUTED) — mode-specific helper text
- Seven labeled fields (label 10 px MONO 0.18em LS DIM, field uses `FieldStyle::composer`):
  - **Provider type** (`openai | anthropic | gemini | ollama | ...`) — required
  - **Label** — display name
  - **Base URL** — optional, falls back to provider default
  - **API key** — required on Add; blank on Edit preserves existing secret (Cycle-3 amendment to the bridge maps empty-string secret to `None`)
  - **Model** / **Context window** / **Temperature** — stored in the credential's `extras` map
- Save (primary gold pill) + Cancel (ghost) right-aligned at the card bottom

**Modal lifecycle:**
- `UxiApp::open_modal(mode)` allocates seven fresh `TextInputState` entries keyed by the `MODAL_*_FIELD` `FieldId` constants and focuses the first.
- Edit mode additionally calls `set_field_text` for each visible field, copying values from the `CloudCredentialView` (the API-key field is intentionally left blank).
- `UxiApp::close_modal` clears the modal state, frees the seven `text_fields` entries, and blurs focus if it sat on a modal field.
- Save → validation (provider type required; API key required on Add) → `BusRequest::CloudUpsert` → wait for `CloudCredentialUpserted` event → `close_modal`.
- Cancel, Escape, or any backdrop click closes without saving. **Modal absorbs clicks**: when `cloud_modal.is_some()`, `UxiApp::hit_test` runs the modal hit-test first and any miss inside the workspace is registered as `ModalHit::Backdrop` rather than falling through to the underlying tab.
- Enter on any modal field calls `submit_modal` (the Enter-to-submit affordance — operators expect "fill, hit Enter, done"). Tab cycles focus forward through the seven fields.

**Bus topology** — five request topics and five response/event topics (defined by `ordo_protocol::cloud_topics`). The UXI publishes requests through a `tokio::sync::mpsc::UnboundedSender<BusRequest>` channel held by `UxiApp`; the bus task on the async side drains the channel via `tokio::select!` alongside its subscription streams. Responses + events arrive through the existing `EventLoopProxy::send_event` path, translated by `translate_envelope`.

```
Request (UXI → bridge)              Response/event (bridge → UXI)
ordo.cloud.credentials.list.request → ordo.cloud.credentials.list.response
ordo.cloud.credential.upsert.request → ordo.cloud.credential.upserted
ordo.cloud.credential.remove.request → ordo.cloud.credential.removed
ordo.cloud.credential.test.request   → ordo.cloud.credential.test.result
ordo.cloud.default.set.request       → ordo.cloud.default.changed
```

**Initial fetch + refresh** — `run()` publishes `CloudList` once on startup (the runtime spawns the bridge before the UXI, so by the time we subscribe the bridge is listening). Any nav transition into the Cloud tab also republishes — cheap insurance against the startup race and a free "refresh" affordance from the user's perspective. The header-card Refresh button does the same thing explicitly.

---

## Part 7: State machine

The UXI's internal state is a small struct on `UxiApp`. It accumulates information from bus messages and feeds the renderer.

```rust
struct UxiState {
    // Window
    size: PhysicalSize<u32>,

    // From bus
    health: Option<HealthState>,
    activity: Option<ActivityState>,
    signals: Signals,           // see Cycle 1 spec
    last_heartbeats: HashMap<NodeId, Instant>,

    // Hardcoded for now
    profile_mode: String,       // "STANDARD"
    profile_port: u16,          // 4141

    // Local state (not from bus)
    active_nav: (usize, usize),
    active_tab: Tab,
    pointer: PointerState,      // cursor pos + hover/pressed Hit
    dock_hits: DockHitMap,      // composer hit rects, captured per-frame

    // Text input (Cycle: text-input primitive)
    text_fields: HashMap<FieldId, TextInputState>,
    input_hits: Vec<(FieldId, Rect)>,  // captured per-frame
    focused_field: Option<FieldId>,
    blink_on: bool,
    last_blink: Instant,

    // Cloud tab (Cycle: cloud-providers)
    cloud_credentials: Vec<CloudCredentialView>,    // sorted default-first
    cloud_default: Option<String>,
    cloud_test_results: HashMap<String, TestState>, // Pending/Ok/Failed
    cloud_modal: Option<CloudModalState>,           // open when Add/Edit
    cloud_hits: CloudHitMap,                        // captured per-frame
    bus_request_tx: Option<UnboundedSender<BusRequest>>,
}
```

Each cycle adds fields here as needed. The state is what the renderer consumes; the bus task updates it via `EventLoopProxy::send_event` translated into state mutations on the winit thread.

---

## Part 8: How a model uses this spec

When writing a cycle, the model:

1. Reads the section for the surface being wired (e.g. Part 5: Sidebar for Cycle 2).
2. Sees the anchor rules and translates them into layout computation. Helper functions in `ordo-uxi/src/layout.rs` (to be added in Cycle 2 if not earlier) implement the anchor primitives once and are reused.
3. Sees the token names and uses the constants already defined in `theme.rs` (or extends `theme.rs` with new ones if the surface introduces something new).
4. Sees the static content (hardcoded strings, fixed icon variants) and renders it.
5. Sees the bus wiring section and adds subscriptions / TODOs as specified.
6. Confirms visually against the static HTML reference (`index.html`/`styles.css`) — this spec describes the *intent*; the static HTML is the *visual truth*. If they disagree, the static HTML wins and this spec needs updating.

---

## Part 9: Open spec questions

These are deferred decisions that will need answers as surfaces are wired:

1. **Drop shadows.** Vello 0.6 has no blur. Several elements use CSS box-shadows for depth (message cards, send button, status card). Options: skip shadows entirely; approximate with a static dim band offset below the element; wait for Vello to ship a blur API. Recommend: skip in early cycles, revisit in a polish cycle.

2. **Backdrop blur.** Topbar's CSS uses `backdrop-filter: blur(24px)` for the frosted effect. Vello can't. Currently rendering as flat 88% alpha INK. Acceptable for now; will need composition pipeline changes if we ever want real blur.

3. **Text rendering for non-ASCII.** Cycle 1 uses skrifa's basic glyph layout, which works for ASCII labels. Message bodies, dynamic content from users, or any internationalization needs `parley` for proper text shaping. Defer until needed.

4. **Hover and focus states.** Vello has no concept of hover. We need to track mouse position in `UxiApp` and re-render when it moves over hoverable regions. Adds complexity. Defer specifying interaction states until Cycle 7 (interactive nav).

5. **Animations.** CSS uses transitions for hover lifts and active state changes. Vello requires manual frame-by-frame animation. Defer all animation to a dedicated polish cycle far in the future.

6. **Scrolling.** The conversation panel needs vertical scroll. Vello doesn't provide it — we implement it ourselves with a viewport offset, mouse-wheel handling, and clipping. Defer to its own cycle.

7. **Responsive breakpoints.** The CSS has a `@media (max-width: 900px)` block that reflows the layout. Anchor rules handle most resize cases gracefully, but below some threshold the topbar's signals strip won't fit. Decide later: clip overflowing signals, hide some, scale them down, or wrap to a second row.

---

This document is the working spec. Update it as surfaces are wired and decisions are made. The static HTML/CSS is the original; this is the Vello-shaped version of the same design.
