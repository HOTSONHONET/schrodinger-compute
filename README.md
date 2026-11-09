# schrodinger-compute

Connect homie nodes to one unified compute platform


## Basic design

```

┌──────────────────────────────────────────┐
│ Schrödinger Compute Desktop              │
│ (Tauri + React — later phase)            │
│                                          │
│ - Nodes view                             │
│ - Resources                              │
│ - Sessions                               │
│ - Terminal attach                        │
└───────────────────▲──────────────────────┘
                    │ HTTP
┌───────────────────┴──────────────────────┐
│ Hub (Coordinator) — Rust + Axum           │
│                                          │
│ - Poll agents                             │
│ - Node registry                          │
│ - Scheduler                              │
│ - Single API for UI & CLI                │
└───────────────────▲──────────────────────┘
                    │ HTTP
┌───────────────────┴──────────────────────┐
│ Agent (per machine) — Rust + Axum         │
│                                          │
│ - Resource reporting                     │
│ - Docker session management              │
│ - Terminal PTY bridge                    │
│                                          │
│ Status: Phase 1 (resource reporting) ✔️  │
└──────────────────────────────────────────┘

```


# How to setup (dev)

## Run the agent in homie-nodes

```bash

cargo run -p agent


```


## Run the hub (Should be first)


```bash


cargo run -p hub

```



## Run the UI

```bash

cd desktop && npm run tauri dev

```

## Distributed session direction

The original resource-unification goal and its implementation milestones are
specified in [the distributed-session RFC](docs/distributed-session-rfc.md).
A separate [two-worker prototype](experiments/distributed-session/README.md)
verifies shared-workspace visibility and partitions one CPU computation across
both workers. It does not yet provide distributed shared memory or a unified
Linux kernel. The existing Rust session APIs are unchanged.

The [unified-session experiment](experiments/unified-session/README.md) adds a
session CLI with striped files, byte-addressed RAM across workers, and compute
on shard owners. It runs independently of the Rust services and needs no shared
NFS mount.
