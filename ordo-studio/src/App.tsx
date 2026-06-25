// Ordo studio entry point.
//
// The shell rendered here is `OrdoShell` — the 41-tab UXI mapping onto the
// ordo-* crate set. The previous wired shell (atmospheric aura, niche
// composer, mechanic chat, etc.) was removed in the UXI reconcile; it remains
// recoverable from git history (the `Baseline before UXI reconcile` commit) if
// any of those pieces are ever needed again.

import OrdoShell from "./OrdoShell";
import UpdateBanner from "./UpdateBanner";

export default function App() {
  return (
    <>
      <OrdoShell />
      <UpdateBanner />
    </>
  );
}
