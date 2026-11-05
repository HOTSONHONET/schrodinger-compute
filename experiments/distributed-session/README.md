# Distributed session experiment

One coordinator dispatches a single reduction across two workers and checks a
shared workspace. See [the architecture RFC](../../docs/distributed-session-rfc.md)
for the route toward a unified Ubuntu environment, remote memory and GPU execution.
This experiment is separate from the existing Rust hub/agent API.

## Local protocol tests (no Docker or extra Python packages)

From the repository root, with Python 3.10+ on Linux:

```bash
python3 -B -m unittest discover -s experiments/distributed-session -v
```

The tests start two HTTP worker services on loopback with a shared temporary
directory. They cover odd/even partitions, concurrent dispatch, identity/result
validation, disconnected workspaces, authentication, invalid ranges, symlinks,
busy workers, worker failure and an actual response timeout. This is not a
physical multi-host or Docker test.

## Beast + Legion: two Ubuntu containers

Prerequisites on each Linux/WSL host: Docker Engine with Compose v2, this checkout,
and network connectivity. Docker must have access to an **existing directory on
the same NFS export** on both hosts. Choose the filesystem/server/export and mount
it before this experiment; no host mounts, exports or firewall changes are automated.
Give UID 1000 read/write access to that directory on both machines. Do not use two
independent local directories: the probe must reject that configuration.

From `experiments/distributed-session` on Beast:

```bash
# Generate once, then transfer through your existing secure channel to Legion.
# Keep this variable out of committed .env files.
export SC_TOKEN="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
export SC_NODE_ID=beast
export SC_WORKSPACE_HOST=/mnt/schrodinger/workspace
export SC_BIND_IP=127.0.0.1
# Pull/build before timing the experiment.
docker compose build
docker compose up -d
```

On Legion, export the same token and use `SC_NODE_ID=legion`. Set
`SC_WORKSPACE_HOST` to Legion's mount of that same export, then build/start there.
Use the checked-in same version of the worker on both hosts.

Keep port 7351 on loopback and forward Legion's worker through SSH from Beast:

```bash
# Use an existing SSH alias that reaches the Linux host running Legion's Docker.
# -N keeps this connection open; run in a separate terminal.
ssh -N -L 7352:127.0.0.1:7351 legion
```

Now run the coordinator on Beast (with the same `SC_TOKEN` still exported):

```bash
python3 -B session.py \
  --workers http://127.0.0.1:7351 http://127.0.0.1:7352 \
  --count 1000000 --timeout 10
```

Expected: `verified: true`, `workspace_verified: true`, sum
`333332833333500000`, and two attributed partial results for `beast` and `legion`.
The elapsed time includes filesystem probes and computation; it is not a compute
speedup benchmark. Both workers iterate their assigned ranges rather than using
the verification formula.

For a larger enough run to inspect CPU, use the supported maximum count
`2000000`, and watch `docker stats` on each host. This remains a short protocol
demonstration and does not establish useful sustained throughput.

Alternatively, bind `SC_BIND_IP` to a trusted LAN address and supply those worker
URLs directly. Bearer authentication does not encrypt HTTP. Use an encrypted
transport such as the SSH forwarding above for traffic outside a trusted lab.

## Terminal and editor boundary

To inspect either Ubuntu container locally on its host:

```bash
docker compose exec worker bash
ls /workspace
```

The shell above executes on that host. The coordinator distributes the explicitly
supported reduction; ordinary shell commands do not automatically use both CPUs.
A stable SSH/VS Code session gateway is an upcoming milestone, not implemented by
this experiment. Continue using existing host Remote SSH access for inspection.

## Failure acceptance and cleanup

Stop Legion's container, rerun the coordinator and verify a nonzero exit with
`session failed (no successful aggregate)`. Restart it, deliberately use an
unshared directory, and verify the workspace preflight fails. Never publish a
successful aggregate when only one worker replied.

On **each** host:

```bash
docker compose down
unset SC_TOKEN
```

Stopping these containers preserves the shared workspace. Normal runs delete
probe files. A failed cleanup can leave `.schrodinger-probe-<session>` files;
remove only probes belonging to completed/abandoned runs, not an active session.
The bound directory must be operator-controlled. Both workers run as UID 1000;
this is a single-owner lab experiment, not multi-tenant isolation.

Timeouts stop waiting; they do not cancel a remote CPU calculation. Work is bounded,
not retried, and may finish after the coordinator fails. The workers have no
persistent queue or replay recovery. Unique advertised node IDs do not attest
physical separation; record actual machine/container evidence for the two-host test.

## Validation status

- Automated protocol suite: runnable without Docker (see PR for observed result).
- Ubuntu image build / Compose deployment: pending in an environment with Docker.
- Beast + Legion + NFS: pending on the user's hardware.
- Remote shared memory, thread migration, GPU pooling: not implemented here.
