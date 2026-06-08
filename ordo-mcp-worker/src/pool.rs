//! Worker pool â€” pre-warmed Workers for low-latency extraction.
//!
//! Rotation policy: a Worker that has handled `max_uses` calls is
//! disposed (scratch zeroized) and replaced with a fresh one.
//! This keeps any accidental residue from a compromised extraction
//! from bleeding into future extractions.
//!
//! The pool is intentionally single-tenant per Worker â€” two
//! concurrent extractions get two different Workers. The pool
//! does NOT multiplex a single Worker across concurrent
//! extractions; that would re-introduce the cross-contamination
//! risk the rotation policy exists to prevent.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;

use crate::{Extractor, Worker, WorkerError, WorkerResult};
use ordo_bus::Bus;
use ordo_protocol::{McpExtractionError, McpExtractionResult, NodeId};

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub size: usize,
    pub max_uses_per_worker: u32,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            size: 3,
            max_uses_per_worker: 10,
        }
    }
}

pub struct WorkerPool {
    extractor: Arc<dyn Extractor>,
    config: PoolConfig,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
    idle: Mutex<VecDeque<Arc<Worker>>>,
    next_id: Mutex<u64>,
}

impl WorkerPool {
    pub fn new(extractor: Arc<dyn Extractor>) -> Self {
        Self::with_config(extractor, PoolConfig::default())
    }

    pub fn with_config(extractor: Arc<dyn Extractor>, config: PoolConfig) -> Self {
        let pool = Self {
            extractor,
            config,
            bus: None,
            node_id: NodeId::new(),
            idle: Mutex::new(VecDeque::new()),
            next_id: Mutex::new(0),
        };
        for _ in 0..pool.config.size {
            let w = pool.spawn_worker();
            pool.idle.lock().push_back(w);
        }
        pool
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_node_id(mut self, node_id: NodeId) -> Self {
        self.node_id = node_id;
        self
    }

    fn spawn_worker(&self) -> Arc<Worker> {
        let mut next_id = self.next_id.lock();
        let id = format!("worker-{}", *next_id);
        *next_id += 1;
        drop(next_id);
        let mut worker = Worker::new(id, self.extractor.clone()).with_node_id(self.node_id.clone());
        if let Some(bus) = &self.bus {
            worker = worker.with_bus(bus.clone());
        }
        Arc::new(worker)
    }

    /// Acquire a worker for one extraction. Returns a `WorkerHandle`
    /// that returns the worker to the pool on drop (unless the
    /// worker has hit its rotation cap, in which case it's
    /// disposed).
    pub fn acquire(&self) -> WorkerResult<WorkerHandle<'_>> {
        let worker = {
            let mut idle = self.idle.lock();
            idle.pop_front()
        };
        let worker = match worker {
            Some(w) => w,
            None => return Err(WorkerError::PoolExhausted),
        };
        Ok(WorkerHandle {
            worker: Some(worker),
            pool: self,
        })
    }

    /// Convenience: acquire + extract + release in one call.
    /// Used by the MCP client on every tool invocation.
    pub async fn extract(
        &self,
        invocation_id: &str,
        tool_id: &str,
        server_id: &str,
        raw_response: &Value,
        expected_schema: &Value,
    ) -> Result<McpExtractionResult, McpExtractionError> {
        let handle = match self.acquire() {
            Ok(h) => h,
            Err(_) => {
                return Err(McpExtractionError::WorkerFailure {
                    details: "pool exhausted".into(),
                });
            }
        };
        let worker = handle.worker();
        worker
            .extract(
                invocation_id,
                tool_id,
                server_id,
                raw_response,
                expected_schema,
            )
            .await
    }

    pub fn len(&self) -> usize {
        self.idle.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.idle.lock().is_empty()
    }
}

pub struct WorkerHandle<'a> {
    worker: Option<Arc<Worker>>,
    pool: &'a WorkerPool,
}

impl<'a> std::fmt::Debug for WorkerHandle<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerHandle")
            .field(
                "worker_id",
                &self.worker.as_ref().map(|w| w.id().to_string()),
            )
            .finish()
    }
}

impl<'a> WorkerHandle<'a> {
    pub fn worker(&self) -> Arc<Worker> {
        self.worker.as_ref().expect("handle worker missing").clone()
    }
}

impl<'a> Drop for WorkerHandle<'a> {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        // Rotation: if the worker has hit its use cap, dispose
        // and replace with a fresh one.
        if worker.uses_since_spawn() >= self.pool.config.max_uses_per_worker {
            worker.dispose();
            let fresh = self.pool.spawn_worker();
            self.pool.idle.lock().push_back(fresh);
        } else {
            self.pool.idle.lock().push_back(worker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeterministicExtractor;
    use serde_json::json;

    #[tokio::test]
    async fn pool_pre_warms_requested_workers() {
        let extractor = Arc::new(DeterministicExtractor::default());
        let pool = WorkerPool::with_config(
            extractor,
            PoolConfig {
                size: 5,
                max_uses_per_worker: 10,
            },
        );
        assert_eq!(pool.len(), 5);
    }

    #[tokio::test]
    async fn acquire_and_drop_returns_worker_to_pool() {
        let extractor = Arc::new(DeterministicExtractor::default());
        let pool = WorkerPool::with_config(
            extractor,
            PoolConfig {
                size: 1,
                max_uses_per_worker: 10,
            },
        );
        {
            let _h = pool.acquire().unwrap();
            assert_eq!(pool.len(), 0);
        }
        assert_eq!(pool.len(), 1);
    }

    #[tokio::test]
    async fn rotation_disposes_worker_at_use_cap() {
        let extractor = Arc::new(DeterministicExtractor::default());
        let pool = WorkerPool::with_config(
            extractor,
            PoolConfig {
                size: 1,
                max_uses_per_worker: 2,
            },
        );
        let schema = json!({ "type": "object", "properties": { "r": { "type": "string" } }, "required": ["r"] });
        let raw = json!({ "r": "x" });
        // Record the pre-rotation worker id.
        let pre_id = {
            let h = pool.acquire().unwrap();
            h.worker().id().to_string()
        };
        // Run up to the cap.
        for _ in 0..2 {
            pool.extract("inv", "tool", "server", &raw, &schema)
                .await
                .unwrap();
        }
        // Post-rotation: acquire should return a different worker.
        let post_id = {
            let h = pool.acquire().unwrap();
            h.worker().id().to_string()
        };
        assert_ne!(pre_id, post_id, "rotation must issue a fresh worker id");
    }

    #[tokio::test]
    async fn pool_exhaustion_returns_error() {
        let extractor = Arc::new(DeterministicExtractor::default());
        let pool = WorkerPool::with_config(
            extractor,
            PoolConfig {
                size: 1,
                max_uses_per_worker: 10,
            },
        );
        let _h1 = pool.acquire().unwrap();
        let err = pool.acquire().unwrap_err();
        assert!(matches!(err, WorkerError::PoolExhausted));
    }
}
