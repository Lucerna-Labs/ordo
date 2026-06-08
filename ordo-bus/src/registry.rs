//! In-memory provider registry for runtime service discovery.
//!
//! Providers announce themselves at startup with the paths they
//! serve; router queries the registry to find live providers for a
//! given path. Heartbeat-based expiry keeps dead providers from
//! being routed to.
//!
//! Not coupled to the `Bus` — callers typically publish register /
//! deregister envelopes on the bus and a listener pipes them into
//! this registry, but the registry is usable standalone.
//!
//! Thread-safe: a `parking_lot::RwLock` around the inner map. Cheap
//! to clone (`Arc`-wrapped at the caller side); this struct itself
//! is intended to be held once per runtime.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

/// A single provider registration.
#[derive(Debug, Clone)]
pub struct ProviderRegistryEntry {
    pub provider_id: String,
    pub serves_paths: Vec<String>,
    /// Free-form metadata (retrieval semantics, cost hint, provenance
    /// guarantee, etc.) serialized as JSON. The registry doesn't
    /// interpret this — only callers do.
    pub metadata: serde_json::Value,
    /// Last heartbeat wall-clock time. Used for expiry.
    pub last_heartbeat: Instant,
    pub heartbeat_interval: Duration,
}

impl ProviderRegistryEntry {
    pub fn new(
        provider_id: impl Into<String>,
        serves_paths: Vec<String>,
        metadata: serde_json::Value,
        heartbeat_interval: Duration,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            serves_paths,
            metadata,
            last_heartbeat: Instant::now(),
            heartbeat_interval,
        }
    }

    pub fn is_alive(&self, now: Instant, grace: Duration) -> bool {
        now.duration_since(self.last_heartbeat) <= self.heartbeat_interval + grace
    }
}

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Default)]
struct Inner {
    by_id: HashMap<String, ProviderRegistryEntry>,
    /// Derived index: path → provider_ids serving that path. Rebuilt
    /// on every mutation — the registry is expected to be small
    /// (dozens, not thousands) so invalidation is cheap.
    by_path: HashMap<String, Vec<String>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, entry: ProviderRegistryEntry) {
        let mut guard = self.inner.write();
        // If re-registering, drop the old paths first.
        if let Some(old) = guard.by_id.remove(&entry.provider_id) {
            remove_from_path_index(&mut guard.by_path, &old.provider_id, &old.serves_paths);
        }
        for path in &entry.serves_paths {
            guard
                .by_path
                .entry(path.clone())
                .or_default()
                .push(entry.provider_id.clone());
        }
        guard.by_id.insert(entry.provider_id.clone(), entry);
    }

    pub fn deregister(&self, provider_id: &str) -> Option<ProviderRegistryEntry> {
        let mut guard = self.inner.write();
        let removed = guard.by_id.remove(provider_id)?;
        remove_from_path_index(
            &mut guard.by_path,
            &removed.provider_id,
            &removed.serves_paths,
        );
        Some(removed)
    }

    pub fn heartbeat(&self, provider_id: &str) -> bool {
        let mut guard = self.inner.write();
        if let Some(entry) = guard.by_id.get_mut(provider_id) {
            entry.last_heartbeat = Instant::now();
            true
        } else {
            false
        }
    }

    /// Return provider entries that serve `path`, filtered to those
    /// still alive given the current wall clock.
    pub fn for_path(&self, path: &str) -> Vec<ProviderRegistryEntry> {
        let guard = self.inner.read();
        let ids = match guard.by_path.get(path) {
            Some(v) => v.clone(),
            None => return Vec::new(),
        };
        let now = Instant::now();
        let grace = Duration::from_secs(2);
        ids.into_iter()
            .filter_map(|id| guard.by_id.get(&id).cloned())
            .filter(|entry| entry.is_alive(now, grace))
            .collect()
    }

    pub fn get(&self, provider_id: &str) -> Option<ProviderRegistryEntry> {
        self.inner.read().by_id.get(provider_id).cloned()
    }

    pub fn all(&self) -> Vec<ProviderRegistryEntry> {
        self.inner.read().by_id.values().cloned().collect()
    }

    /// Remove entries whose heartbeat deadline has passed. Returns
    /// the list of expired provider_ids so the caller can emit
    /// `memory.provider.deregister` events for each.
    pub fn sweep_expired(&self, grace: Duration) -> Vec<ProviderRegistryEntry> {
        let now = Instant::now();
        let mut guard = self.inner.write();
        let expired_ids: Vec<String> = guard
            .by_id
            .values()
            .filter(|e| !e.is_alive(now, grace))
            .map(|e| e.provider_id.clone())
            .collect();
        let mut expired = Vec::with_capacity(expired_ids.len());
        for id in expired_ids {
            if let Some(entry) = guard.by_id.remove(&id) {
                remove_from_path_index(&mut guard.by_path, &entry.provider_id, &entry.serves_paths);
                expired.push(entry);
            }
        }
        expired
    }

    pub fn len(&self) -> usize {
        self.inner.read().by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().by_id.is_empty()
    }
}

fn remove_from_path_index(
    by_path: &mut HashMap<String, Vec<String>>,
    provider_id: &str,
    paths: &[String],
) {
    for path in paths {
        if let Some(bucket) = by_path.get_mut(path) {
            bucket.retain(|id| id != provider_id);
            if bucket.is_empty() {
                by_path.remove(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(id: &str, paths: &[&str]) -> ProviderRegistryEntry {
        ProviderRegistryEntry::new(
            id,
            paths.iter().map(|s| (*s).to_string()).collect(),
            json!({"cost": "cheap"}),
            Duration::from_secs(30),
        )
    }

    #[test]
    fn register_and_lookup_by_path() {
        let reg = ProviderRegistry::new();
        reg.register(entry("a", &["lucerna/voice", "lucerna/brand"]));
        reg.register(entry("b", &["lucerna/voice"]));
        let voice = reg.for_path("lucerna/voice");
        assert_eq!(voice.len(), 2);
        let brand = reg.for_path("lucerna/brand");
        assert_eq!(brand.len(), 1);
        assert_eq!(brand[0].provider_id, "a");
    }

    #[test]
    fn re_register_replaces_paths() {
        let reg = ProviderRegistry::new();
        reg.register(entry("a", &["p1"]));
        reg.register(entry("a", &["p2"]));
        assert!(reg.for_path("p1").is_empty());
        assert_eq!(reg.for_path("p2").len(), 1);
    }

    #[test]
    fn deregister_removes_from_both_indexes() {
        let reg = ProviderRegistry::new();
        reg.register(entry("a", &["p"]));
        let removed = reg.deregister("a").expect("present");
        assert_eq!(removed.provider_id, "a");
        assert!(reg.for_path("p").is_empty());
        assert!(reg.get("a").is_none());
    }

    #[test]
    fn expired_providers_hidden_from_for_path() {
        let reg = ProviderRegistry::new();
        let mut e = entry("a", &["p"]);
        // Backdate last_heartbeat so the provider is already dead.
        e.last_heartbeat = Instant::now() - Duration::from_secs(60);
        e.heartbeat_interval = Duration::from_secs(1);
        reg.register(e);
        let live = reg.for_path("p");
        assert!(live.is_empty());
        // But it's still in the registry until sweep_expired runs.
        assert!(reg.get("a").is_some());
    }

    #[test]
    fn sweep_expired_removes_and_returns_them() {
        let reg = ProviderRegistry::new();
        let mut stale = entry("stale", &["p"]);
        stale.last_heartbeat = Instant::now() - Duration::from_secs(60);
        stale.heartbeat_interval = Duration::from_secs(1);
        reg.register(stale);
        reg.register(entry("fresh", &["p"]));
        let swept = reg.sweep_expired(Duration::from_millis(100));
        assert_eq!(swept.len(), 1);
        assert_eq!(swept[0].provider_id, "stale");
        assert!(reg.get("stale").is_none());
        assert!(reg.get("fresh").is_some());
    }

    #[test]
    fn heartbeat_refreshes_liveness() {
        let reg = ProviderRegistry::new();
        let mut e = entry("a", &["p"]);
        e.last_heartbeat = Instant::now() - Duration::from_secs(60);
        e.heartbeat_interval = Duration::from_secs(1);
        reg.register(e);
        assert!(reg.for_path("p").is_empty());
        assert!(reg.heartbeat("a"));
        assert_eq!(reg.for_path("p").len(), 1);
    }
}
