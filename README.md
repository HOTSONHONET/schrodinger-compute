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