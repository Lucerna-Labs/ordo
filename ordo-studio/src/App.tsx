// Ordo studio entry point.
//
// The shell rendered here is `OrdoShell` — the 15-tab UXI mapping onto the
// ordo-* crate set. The previous wired shell (atmospheric aura, niche
// composer, mechanic chat, etc.) is preserved in the git history of this
// file and in the unchanged components/ directory; if any of those pieces
// are needed again they can be lifted from there without re-deriving.

import OrdoShell from "./OrdoShell";

export default function App() {
  return <OrdoShell />;
}
