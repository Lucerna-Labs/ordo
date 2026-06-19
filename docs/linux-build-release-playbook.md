# Linux Build & Release Playbook (for future coders)

How to build, fix, verify, and ship Ordo's Linux desktop package without
repeating the 3-hour debugging session that produced it. Read
[linux-popos-install-findings.md](linux-popos-install-findings.md) first for the
*why*; this is the *how*.

## The one thing to internalize

Ordo's desktop shell (`ordo-servo-shell`, feature `servo-engine`) depends on the
full `servo` crate, which **compiles Servo + SpiderMonkey (`mozjs_sys`) +
WebRender + Stylo from source** — ~850 crates, ~30–60 min, ~8 GB RAM, ~15 GB
disk. So:

- A source install is heavy by nature. The **prebuilt `.deb`** is the path users
  should take; the source build is the fallback.
- The build needs a big toolchain (C/C++, CMake, Clang/libclang, LLVM, Python 3,
  the font stack) **plus** `libtss2-dev` for the TPM sealer. CI runners and dev
  WSL boxes already have most of this, which is why broken dependency lists pass
  there and fail on a clean machine. **Never trust "it built on CI / my box" as
  proof the install works.** Only a clean container proves it.

## Golden rule: verify in a clean container, not on your box

`ordo-servo-shell` is a **separate cargo workspace** (it's in `exclude` in the
root `Cargo.toml`), so `cargo build --workspace` in CI never builds it. The
Servo shell build is only exercised by the release job and by this playbook.

### Reproduce the full build on a clean Pop!_OS base (Ubuntu 22.04)

Build on **22.04**, the oldest supported base — a `.deb` linked there installs
on both 22.04 and 24.04. Building on 24.04 produces a package that won't install
on 22.04 (newer glibc/`t64` libs).

From the repo root (Docker required):

```bash
# On Windows / Git Bash, prefix docker volume mounts with MSYS_NO_PATHCONV=1 and
# use a Windows-style path, or Git Bash mangles /c/... into the wrong mount.
docker run --rm -v "$(pwd)":/src:ro \
  -v ordo_cargo:/root/.cargo -v ordo_rustup:/root/.rustup -v ordo_cache:/cache \
  ubuntu:22.04 bash -c '
    set -e
    export DEBIAN_FRONTEND=noninteractive HOME=/root
    apt-get update && apt-get install -y ca-certificates curl git
    git clone --branch main /src /root/ordo        # clone => LF endings + committed state
    cd /root/ordo
    # one source of truth for build deps (installs apt libs + rust + node):
    bash scripts/install-linux-build-deps.sh
    . "$HOME/.cargo/env"
    ./Build-Ordo-Linux-Deb.sh
  '
```

The named volumes (`ordo_cargo`, `ordo_rustup`, `ordo_cache`) cache the toolchain
and ~850 compiled crates so re-runs are fast. To test *uncommitted* working-tree
changes, `cp` the changed files over the clone before building instead of relying
on `git clone` (which only sees committed state).

### Install-test the `.deb` on clean 22.04 AND 24.04

A clean base has no desktop libs, so this is stricter than a real Pop!_OS box and
catches missing `Depends:`:

```bash
for img in ubuntu:22.04 ubuntu:24.04; do
  docker run --rm -v "$(pwd)/dist":/dist:ro "$img" bash -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    dpkg -i /dist/ordo_*_amd64.deb || apt-get install -f -y    # the real 2-step install
    dpkg -l ordo | tail -1                                     # must end up "ii"
    for b in ordo ordo-servo-shell; do
      ldd /opt/ordo/bin/$b | grep -i "not found" && echo "MISSING LIB in $b" || echo "$b OK"
    done
    /opt/ordo/bin/ordo --help >/dev/null && echo "runtime runs"
  '
done
```

`ii ordo` + no "not found" on either image = the package is correct. (The
`dpkg -i` first pass printing dependency errors is expected; `apt-get install -f`
resolves them. That two-step is exactly what `scripts/install-linux-prebuilt.sh`
does.)

## Diagnosing a NEW dependency failure

When a `*-sys` crate fails, the error names the missing system library. Map it to
the Ubuntu `-dev` package and add it to **`scripts/install-linux-build-deps.sh`**
(the single source of truth — the release CI runs this same script, so a green
release proves the user dep list).

| Build error mentions | Add this apt package |
|---|---|
| `tss2-sys` / `tss-esapi-sys` | `libtss2-dev` |
| `could not find system library 'fontconfig'` | `libfontconfig-dev` |
| `freetype` / `harfbuzz` `-sys` | `libfreetype-dev` / `libharfbuzz-dev` |
| `mozjs_sys` bindgen / `libclang` | `clang libclang-dev llvm-dev` |
| `aws-lc-sys` / vendored C build | `cmake` (+ a C compiler) |
| SpiderMonkey build invoking python | `python3 python-is-python3` |

To see what the Servo tree will pull (and therefore what tools it needs) without
a full build:

```bash
docker run --rm -v "$(pwd)":/src:ro rust:1.93-bookworm bash -c '
  cp -r /src/ordo-servo-shell /tmp/ss && cd /tmp/ss
  cargo tree --features servo-engine -e normal | grep -iE "mozjs|aws-lc|harfbuzz|freetype|fontconfig|sys$"'
```

## Computing the `.deb` `Depends:` correctly

Don't hand-wave the runtime deps. Derive them from the built binaries:

1. **Hard-linked libs** — `readelf -d BIN | grep NEEDED`, then map each `.so` to
   its package with `dpkg -S` (pick the `x86_64-linux-gnu` match, **not** the
   i386 one). Both `ordo` and `ordo-servo-shell` matter. Today that's `libc6,
   libgcc-s1, libstdc++6, zlib1g, libfontconfig1` (+ font transitive deps via
   `libfontconfig1`) and `libtss2-esys-3.0.2-0 + libtss2-tctildr0` for the TPM
   sealer. `libssl3` is **not** needed — aws-lc is statically linked.
2. **dlopen'd libs** — `winit`/`surfman` open GL/X11/Wayland at runtime, so they
   never appear in `NEEDED`. Declare them anyway: `libx11-6, libxcursor1, libxi6,
   libxrandr2, libxcb1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0,
   libwayland-cursor0, libwayland-egl1, libegl1, libgles2`.
3. **Cross-release names** — Ubuntu 24.04's `t64` transition renamed some
   packages (e.g. `libtss2-esys-3.0.2-0` → `...-0t64`). Use `pkg | pkg-t64`
   alternatives in `Depends:` and verify the whole list resolves on **both**
   22.04 and 24.04 with `apt-get install --dry-run`.

## Cross-platform gotchas

- **The lockfile is shared with Windows.** `ordo-studio/package-lock.json` is
  consumed by `npm ci` on Linux (build scripts) *and* Windows
  (`Launch-Ordo-Servo.ps1`, which uses strict `npm ci` with no fallback). If you
  touch it, regenerate with `npm install --package-lock-only` and confirm
  `npm ci` passes on **both** OSes. A lock generated on one platform can miss the
  other's optional/native deps (this is what broke Linux: missing `@emnapi/*`
  wasm variants from Vite/Rolldown).
- **Line endings.** `.gitattributes` pins `*.sh` to `eol=lf`. Always test via
  `git clone` (not a raw copy of a Windows working tree) so scripts get LF.
- **Don't add a fallback to hide lock drift.** CI's `npm ci || npm install`
  masked the broken lockfile for ages. Fix the lock, don't paper over it.

## Cutting a prebuilt release

1. Land the change on `main` (CI + the smoke workflow run automatically).
2. Tag a release: `git tag v0.1.x && git push origin v0.1.x`.
3. `.github/workflows/release.yml`'s `linux-desktop` job builds the `.deb` on
   `ubuntu-22.04` (running `install-linux-build-deps.sh`) and publishes it.
4. `scripts/install-linux-prebuilt.sh` then finds it; the public one-liner works.

> The release `.deb` build is a full Servo compile (~30–60 min on a 4-core
> runner). It runs only on tags / manual dispatch, not every push.

## Pre-ship checklist for any Linux change

- [ ] `bash -n` clean on every changed `.sh`.
- [ ] `scripts/install-linux-build-deps.sh` runs to completion on a clean
      `ubuntu:22.04` container (ends "prerequisites are ready").
- [ ] `.deb` builds on clean 22.04 (0 errors, incl. `mozjs_sys`).
- [ ] `.deb` installs (`ii`) with no "not found" on clean 22.04 **and** 24.04.
- [ ] If `package-lock.json` changed: `npm ci` passes on Linux **and** Windows.
- [ ] Docs updated ([INSTALL.md](../INSTALL.md), README, LINUX_BUILD.md).
- [ ] Real-desktop smoke test (the Servo window actually opening) — the one thing
      containers can't verify.
