# multiglass

Transparently handle multiple [shellglass](https://github.com/iksteen/shellglass) sessions on a single machine.

## Installation

```bash
cargo install --git https://github.com/mrexodia/multiglass
```

## Usage

```bash
# Configure upstream hub
multiglass config https://upstream.hub/ <shellglass-key> --port 47890

# Start local relay on port 47890 and stream to https://upstream.hub
multiglass start

# Wrap this shell's tab and push it to the local relay — goes live immediately
multiglass stream

# Attach a tab without going live yet (switch to it later)
multiglass stream --no-switch

# Switch the upstream hub to this tab (run from inside it, or pass a slug)
multiglass switch

# Show where we're streaming to and what's locally attached
multiglass status

# Stop streaming
multiglass stop
```
