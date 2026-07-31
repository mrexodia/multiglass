# multiglass

Transparently handle multiple [shellglass](https://github.com/iksteen/shellglass) sessions on a single machine.

## Installation

```bash
cargo install --git https://github.com/mrexodia/multiglass
```

Linux, macOS, and Windows are supported. Windows terminal streaming uses
ConPTY and therefore requires Windows 10 version 1809 or newer.

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

`stream` starts `$SHELL` by default on Unix and `%COMSPEC%` (`cmd.exe`) on
Windows. To use PowerShell, pass it explicitly:

```powershell
multiglass stream -- pwsh.exe -NoLogo
# Or, for Windows PowerShell:
multiglass stream -- powershell.exe -NoLogo
```

Configuration, the relay PID, and logs are stored in
`~/.config/multiglass` on Unix and `%APPDATA%\multiglass` on Windows.
