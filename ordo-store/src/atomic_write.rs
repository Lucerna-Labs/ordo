//! Atomic file write helper.
//!
//! Replaces `std::fs::write(path, bytes)` for files where a
//! crash mid-write would leave a half-written file that breaks
//! reads. The OpenClaw saga we lived through (gateway service
//! crashing into a JSON-schema validator on a config left
//! partially-written by a `--force` rebuild) was exactly this
//! class of bug — visible from outside as "the runtime won't
//! start anymore."
//!
//! ## Pattern
//!
//! 1. Write the new content to a sibling temp file
//!    (`path.atomicwrite-<pid>-<nanos>.tmp`).
//! 2. `fsync` the temp file so the bytes are durable.
//! 3. Rename the temp over the destination — atomic on every
//!    desktop OS Ordo runs on (POSIX `rename(2)`, Windows
//!    `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` via Rust's
//!    `std::fs::rename`).
//!
//! On a crash *before* the rename, the destination is unchanged
//! and the temp file is left behind; on a crash *after* the
//! rename, the destination has the full new content.
//!
//! ## What this does NOT do
//!
//! - **fsync the parent directory.** POSIX requires syncing the
//!   directory inode for the rename to be durable, otherwise a
//!   power loss can lose the rename even if the temp's bytes
//!   were synced. Stdlib doesn't expose `fsync(dir_fd)`. We rely
//!   on most filesystems' default-ordered behaviour and the
//!   journal being honest about the rename. For ordo-runtime's
//!   threat model (operator-initiated writes, rare power loss
//!   during config edit) this is acceptable. A future revision
//!   can call out to `nix::fcntl::fsync` on Unix if the call
//!   sites prove this matters.
//! - **Hash-verify the read-back.** After rename, we don't
//!   re-read + checksum. Add that at the call site if the
//!   downstream consumer needs strong "what was written is what
//!   I'll read back" guarantees.
//! - **Cap-style permissions / ownership preservation.** The
//!   rename adopts the temp file's mode (default 644 / 600
//!   per umask). On Unix, if you're overwriting a file with
//!   custom permissions, you'd need to read+restore them. Not
//!   needed for the current call sites (plugin manifests +
//!   extension files use the default mode).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Monotonic counter so multiple atomic_writes within the same
/// nanosecond from the same process still get unique temp
/// filenames. Wraps at u64 max — practically infinite.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomically write `bytes` to `path`. Returns `Ok(())` on
/// success. On any error (temp open, write, fsync, rename) the
/// original file at `path`, if any, is left untouched.
///
/// The temp file is created in the same directory as `path` so
/// the rename stays on the same filesystem (cross-fs renames
/// can fall back to copy-then-delete, which loses atomicity).
pub fn atomic_write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic_write target has no parent directory",
        )
    })?;
    // Build a unique sibling temp filename.
    let temp_path = make_temp_path(path);
    // Ensure parent exists. fs::write doesn't auto-create; we
    // mirror that behaviour — caller is expected to have
    // ensured the parent. We do NOT silently `create_dir_all`
    // because a missing parent often signals a logic bug
    // (typo'd path, wrong working dir).
    let _ = parent;

    // Write + fsync the temp file. Use `create_new` so we
    // never accidentally clobber an existing temp from a
    // different in-flight write (collisions should be
    // impossible given the unique suffix, but the assertion is
    // free).
    let res: io::Result<()> = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(bytes.as_ref())?;
        file.sync_all()?;
        // Drop closes the file before we rename — Windows
        // requires the source to be closed for MoveFileEx to
        // succeed.
        drop(file);
        fs::rename(&temp_path, path)?;
        Ok(())
    })();

    // If we failed before rename, clean up the temp file. After
    // a successful rename, there's no temp to clean (it became
    // the destination).
    if res.is_err() && temp_path.exists() {
        let _ = fs::remove_file(&temp_path);
    }
    res
}

fn make_temp_path(target: &Path) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "atomicwrite".into());
    let temp_name = format!(".{stem}.atomicwrite-{pid}-{nanos}-{counter}.tmp");
    target.with_file_name(temp_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering as Ord2};

    // Each test uses its own subdir to avoid sharing temp files.
    fn fresh_test_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ord2::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("ordo-store-atomic-{label}-{pid}-{n}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir test dir");
        dir
    }

    #[test]
    fn writes_new_file() {
        let dir = fresh_test_dir("new");
        let p = dir.join("settings.json");
        atomic_write(&p, b"{\"hello\":1}").expect("write");
        let read_back = fs::read_to_string(&p).expect("read back");
        assert_eq!(read_back, "{\"hello\":1}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwrites_existing_file_atomically() {
        let dir = fresh_test_dir("overwrite");
        let p = dir.join("settings.json");
        fs::write(&p, b"OLD").expect("seed");
        atomic_write(&p, b"NEW").expect("write");
        let read_back = fs::read_to_string(&p).expect("read back");
        assert_eq!(read_back, "NEW");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn errors_when_parent_missing() {
        let dir = std::env::temp_dir().join("definitely-not-there-xyz");
        let _ = fs::remove_dir_all(&dir);
        let p = dir.join("settings.json");
        let err = atomic_write(&p, b"x").expect_err("expected error");
        // open(temp) fails because parent doesn't exist.
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ),
            "got {:?}",
            err.kind()
        );
    }

    #[test]
    fn keeps_original_when_write_fails_at_temp_open() {
        // We can't easily simulate a fsync-fail in a portable
        // test, but we CAN verify that a parent-missing error
        // (the most common failure path before rename) leaves
        // a pre-existing dest alone — except dest can't exist
        // either if parent doesn't, so use a permission-denied
        // path. Skip on Windows where chmod-readonly semantics
        // differ.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = fresh_test_dir("denied");
            let p = dir.join("settings.json");
            fs::write(&p, b"OLD").expect("seed");
            // Make dir read-only so the temp file can't be
            // created.
            let mut perms = fs::metadata(&dir).unwrap().permissions();
            perms.set_mode(0o555);
            fs::set_permissions(&dir, perms).expect("chmod");
            let res = atomic_write(&p, b"NEW");
            assert!(res.is_err());
            // Restore so cleanup can run.
            let mut perms = fs::metadata(&dir).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&dir, perms).expect("chmod restore");
            // Original content untouched.
            let read_back = fs::read_to_string(&p).expect("read back");
            assert_eq!(read_back, "OLD");
            let _ = fs::remove_dir_all(&dir);
        }
        // Windows path: just assert that two consecutive atomic
        // writes both succeed — i.e. the temp-naming + rename
        // dance doesn't deadlock or leak.
        #[cfg(windows)]
        {
            let dir = fresh_test_dir("denied");
            let p = dir.join("settings.json");
            atomic_write(&p, b"first").expect("write 1");
            atomic_write(&p, b"second").expect("write 2");
            assert_eq!(fs::read_to_string(&p).unwrap(), "second");
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn temp_files_are_cleaned_up_on_success() {
        let dir = fresh_test_dir("cleanup");
        let p = dir.join("settings.json");
        atomic_write(&p, b"x").expect("write");
        // Only the destination should remain. No `.atomicwrite-*.tmp`.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".atomicwrite-"))
            .collect();
        assert!(leftovers.is_empty(), "expected no temp files");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn temp_path_is_in_same_directory_as_target() {
        let target = Path::new("/some/dir/file.json");
        let temp = make_temp_path(target);
        assert_eq!(temp.parent(), target.parent());
        let temp_name = temp.file_name().unwrap().to_string_lossy().to_string();
        assert!(temp_name.contains("file.json"));
        assert!(temp_name.contains(".atomicwrite-"));
        assert!(temp_name.ends_with(".tmp"));
    }

    #[test]
    fn many_concurrent_writes_to_same_file_dont_collide() {
        // Same-process concurrency — multiple threads. The
        // counter + nanos suffix should give each its own
        // temp filename.
        use std::sync::Arc;
        use std::thread;
        let dir = Arc::new(fresh_test_dir("concurrent"));
        let p = Arc::new(dir.join("settings.json"));
        let mut handles = vec![];
        for i in 0..16 {
            let p = Arc::clone(&p);
            handles.push(thread::spawn(move || {
                atomic_write(&*p, format!("write-{i}").as_bytes())
                    .expect("write should succeed despite contention");
            }));
        }
        for h in handles {
            h.join().expect("thread");
        }
        // Final content is whichever thread won the last rename.
        // The file is readable + non-empty.
        let content = fs::read_to_string(&*p).expect("read final");
        assert!(content.starts_with("write-"));
        let _ = fs::remove_dir_all(&*dir);
    }
}
