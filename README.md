# schrodinger-compute

Connect homie nodes to one unified compute platform

## Interactive codebase flowchart

Open [docs/codebase-flow.html](docs/codebase-flow.html) in a browser to explore the
system overview, resource reporting, session scheduling, terminal attach, and
source/state map. Select components for source links and implementation notes.
The file works offline and includes search, zoom, and print-to-PDF controls.


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
