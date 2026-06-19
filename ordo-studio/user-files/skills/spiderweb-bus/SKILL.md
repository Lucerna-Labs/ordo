---
name: spiderweb-bus
description: "Jesse's layered bus architecture — the spiderweb/fabric model for data routing that sits at the core of all his platform work. Use this skill WHENEVER the task involves the bus, the fabric, the spiderweb, data routing between nodes, on-ramps/off-ramps, parallel execution layers, the highway metaphor, thread formation, intersections, vibration propagation, or any work on the platform bus in any project (Ordo, Nodus, cumfy-llm, Drag and Drop Bus Platform, or any future project). Also trigger when a model is designing data flow, adding a layer, wiring nodes, or making architectural decisions about how messages move between subsystems. This architecture is NOT in any model's training data — it was invented by Jesse and refined across many conversations. Do not substitute a more familiar pattern. Read this skill before writing bus code."
category:
  - architecture
  - bus
  - coding
available_to_modes:
  - coding
  - rust_vibe_coder
  - research
risk_level: medium
requires_tools: false
---

# The Spiderweb Bus Architecture

This is Jesse's original architecture for layered, organic data routing. It is the conceptual foundation under Ordo, Nodus, and every platform bus in Jesse's ecosystem. No model has this in training data because it was invented here, in conversation, and refined iteratively.

If you are about to write code that routes data between subsystems, adds a bus layer, or makes architectural decisions about message flow — read this first. Do not substitute a more familiar pattern (pub/sub, event sourcing, actor model, etc.) unless Jesse specifically asks for one. The spiderweb is the architecture. Match it.

## The Mental Model

Picture a mega highway system. The ground level goes in many directions like local streets, full of on-ramps and off-ramps that lead to and off of a super highway above.

### Ground Level — The Comfy Bus

Local streets. Messages moving between nodes at their own pace, stopping at intersections, taking turns. Normal node-to-node communication. Lots of on-ramps and off-ramps — any node can post to the bus, any node can listen. This is the base layer that everything else sits on top of.

### The Highway Above — The Parallel Layer

Fast, directional, and it doesn't stop. When something hits an on-ramp, it gets lifted out of the local traffic, blasted across multiple lanes simultaneously, and then the results drop back down via off-ramps to wherever they need to go on the ground.

### On-Ramps and Off-Ramps

The key interface between layers. On-ramps are where ground-level messages get **promoted** to parallel execution. Off-ramps are where parallel results get **demoted** back to normal bus traffic. These are the elevation changes — the mechanism for moving between speed tiers.

### The Purpose

The point of this architecture is to make data flow **faster and more efficient than direct connections**. Like why highways exist — not every car needs a direct road to every destination. The highway aggregates traffic, moves it fast, and distributes it at exits. The bus does the same with messages.

## The Spiderweb — Organic Structure

The architecture is not a rigid pipeline. It's a thick, layered spiderweb.

### Threads

Threads are not predefined. They **form as messages flow**, like desire paths. A message enters the fabric and creates or follows a thread as it moves. Threads are paths through multiple nodes — not just A→B, but a route through many nodes.

```rust
struct Thread {
    id: ThreadId,
    direction: Vec<NodeId>,  // path through many nodes
}
```

### Intersections

When two threads happen to pass through the same node, an intersection naturally exists. At intersections, messages on one thread can hop to another, governed by a transfer policy.

```rust
struct Intersection {
    threads: Vec<ThreadId>,       // which threads cross here
    transfer: TransferPolicy,     // can messages jump between threads?
}
```

### The Fabric

The fabric is the emergent result of messages flowing. It is not declared upfront — it forms organically.

```rust
struct Fabric {
    bus: ComfyBus,                // ground level
    threads: Vec<Thread>,         // the weave above
    intersections: Vec<Intersection>,
    ramps: Vec<Ramp>,             // on/off ramps between levels
}
```

### Vibrations

The real power of a spiderweb is that vibrations propagate. When something touches one part of the web, the rest of the web feels it. This maps to cross-layer communication:

- **Vertical signals** — events that propagate up through layers (backpressure from L0 → L1 → L2 → L3)
- **Horizontal signals** — peer-to-peer within a layer (one node telling its neighbors it's busy)
- **Cross-cutting concerns** — tracing, metrics, auth that span all layers but live in none

### Thickness

The web is not one strand — it's bundles. You can have **multiple instances of the same layer running in parallel**. Multiple transport backends (local channels + network). Multiple flow graphs (one per workspace or pipeline). All woven together by the orchestration layer. That's where the thickness comes from.

## The Four Layers

Each layer wraps the one below it. L3 holds L2, L2 holds L1, L1 holds L0. But any layer can emit signals that propagate both up and down.

### Layer 0 — Transport

The physical movement of bytes. Raw send/receive. This is the ground.

```rust
trait TransportLayer: Send + Sync {
    async fn send_raw(&self, target: NodeId, payload: Bytes) -> Result<()>;
    async fn recv_raw(&self) -> Result<(NodeId, Bytes)>;
}
```

### Layer 1 — Message

Typed messages on top of raw transport. Publish/subscribe semantics. Nodes subscribe to message types they care about.

```rust
trait MessageLayer: Send + Sync {
    async fn publish<M: BusMessage>(&self, msg: M) -> Result<()>;
    async fn subscribe<M: BusMessage>(&self) -> Receiver<M>;
}
```

### Layer 2 — Flow

The node graph. Connections between ports, graph topology, dataflow execution. This is where the ComfyUI-style wiring lives.

```rust
trait FlowLayer: Send + Sync {
    fn connect(&mut self, from: PortId, to: PortId) -> Result<()>;
    fn topology(&self) -> &Graph;
    async fn tick(&mut self) -> Result<()>; // advance the dataflow
}
```

### Layer 3 — Orchestration

The spider sitting in the center. Scheduling, cost awareness, retries, fallbacks. This layer watches the whole web vibrate and makes decisions. If a local model is overloaded, reroute to cloud. If a pipeline step fails, retry or degrade gracefully. It has a global view.

```rust
trait OrchestrationLayer: Send + Sync {
    async fn schedule(&self, task: PipelineTask) -> Result<ExecutionPlan>;
    async fn react(&self, signal: BusSignal) -> Result<Action>;
}
```

## Key Architectural Properties

### Preloading

The bus can preload the next node's data while the current one computes. This is the fabric doing work ahead of time — like a CPU instruction pipeline prefetching from cache. The bus knows the full graph topology, so it can predict what data will be needed next.

### Backpressure

Backpressure propagates through the web like vibration. When a node is slow (say, running a large model), the bus doesn't just queue — it signals upstream nodes to slow down or reroute. This is vertical signal propagation: L0 detects the slowdown, L1 reroutes, L2 adjusts the flow graph, L3 schedules around it.

### Cost-Aware Routing

The bus knows which nodes hit paid APIs versus local models. The orchestration layer can optimize routing based on budget constraints. This is the "3D fabric routing" concept — the bus routes tensor data to whichever GPU has capacity, with prefetch/batching optimizations.

### Graph as Serializable State

Borrowed from ComfyUI: the graph is serializable JSON, so workflows are shareable and reproducible. A "workflow" is just a saved configuration of which nodes are connected — the bus doesn't care what the nodes do, it just routes data between them.

## Relationship to Ordo

The spiderweb bus IS the Ordo bus. When Ordo documentation says "crates communicate only through typed messages on the bus," it's describing Layer 1 of this architecture. The higher layers (flow, orchestration) are where the spiderweb's power lives — they're the highway above the local streets.

The Ordo runtime skill describes the crate-level rules (single process, no IPC, protocol crate first). This skill describes the bus architecture those crates communicate through. Both are required for any Ordo/Nodus work.

## What Agents Must Not Do

1. **Do not substitute a familiar architecture.** Pub/sub, actor model, event sourcing, message queue — these are all simpler and all wrong for this use case. The spiderweb is layered, organic, and elevation-based. Match it.
2. **Do not declare the fabric upfront.** Threads form as messages flow. Intersections emerge where threads cross. The fabric is emergent, not configured.
3. **Do not ignore vibration propagation.** When one layer changes state, other layers must feel it. Backpressure, rerouting, cost-adjustment — these are not optional features, they're core properties of the web.
4. **Do not flatten the layers.** The elevation metaphor is structural. Ground level is slow and flexible. Highway level is fast and directional. They serve different purposes and messages move between them via ramps, not by being on the same plane.
5. **Do not skip preloading.** The bus knows the graph. It should prefetch data for the next node while the current node computes. This is one of the core performance advantages over direct connections.
