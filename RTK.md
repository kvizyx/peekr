<!-- rtk-instructions v2 -->
# Token-optimized command usage

Prefix all commands listed in [commands by workflow](#commands-by-workflow) with `rtk`. If RTK has a dedicated filter, it uses it. If not, it passes through unchanged. This means RTK is always safe to use.

Even in command chains with `&&`, use `rtk`. Example:

```bash
# Wrong
git add . && git commit -m "message" && git push

# Correct
rtk git add . && rtk git commit -m "message" && rtk git push
```

Re-run a command as `rtk proxy <command>` only when its result is unusable: empty when output was clearly expected, contradicting its exit code, or garbled.

## Commands by workflow

### Build and compile
```bash
rtk cargo build         # Cargo build output
rtk cargo check         # Cargo check output
rtk cargo clippy        # Clippy warnings grouped by file
```

### Test
```bash
rtk cargo test          # Cargo test
rtk test <cmd>          # Generic test wrapper
```

### Git

Note that Git passthrough works for all subcommands, even those not explicitly listed.

```bash
rtk git status          # Compact status
rtk git log             # Compact log (works with all git flags)
rtk git diff            # Compact diff
rtk git show            # Compact show
rtk git add             # Ultra-compact confirmations
rtk git commit          # Ultra-compact confirmations
rtk git push            # Ultra-compact confirmations
rtk git pull            # Ultra-compact confirmations
rtk git branch          # Compact branch list
rtk git fetch           # Compact fetch
rtk git stash           # Compact stash
rtk git worktree        # Compact worktree
```

### GitHub
```bash
rtk gh pr view <num>    # Compact PR view
rtk gh pr checks        # Compact PR checks
rtk gh run list         # Compact workflow runs
rtk gh issue list       # Compact issue list
rtk gh api              # Compact API responses
```

### Files and search
```bash
rtk ls <path>           # Tree format, compact
rtk read <file>         # Code reading with filtering
rtk grep <pattern>      # Search grouped by file (75%). Format flags (-c, -l, -L, -o, -Z) run raw.
rtk find <pattern>      # Find grouped by directory
```

### Analysis and debug
```bash
rtk err <cmd>           # Filter errors only from any command
rtk log <file>          # Deduplicated logs with counts
rtk json <file>         # JSON structure without values
rtk deps                # Dependency overview
rtk env                 # Environment variables compact
rtk summary <cmd>       # Smart summary of command output
rtk diff                # Ultra-compact diffs
```

### Infrastructure
```bash
rtk docker ps           # Compact container list
rtk docker images       # Compact image list
rtk docker logs <c>     # Deduplicated logs
rtk kubectl get         # Compact resource list
rtk kubectl logs        # Deduplicated pod logs
```

### Network
```bash
rtk curl <url>          # Compact HTTP responses
rtk wget <url>          # Compact download output
```

### Meta commands
```bash
rtk gain                # View token savings statistics
rtk gain --history      # View command history with savings
rtk proxy <cmd>         # Run command without filtering (for debugging)
rtk init                # Add RTK instructions to RTK.md
```
<!-- /rtk-instructions -->
