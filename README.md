# jj-hunk-tool

For when you wish `jj split --interactive` wasn't interactive.

Hunk-level operations for [Jujutsu (jj)](https://github.com/jj-vcs/jj), built on [git-surgeon](https://github.com/raine/git-surgeon).

jj is great at rewriting history at the commit and file level (split, squash, rebase, etc.), but it doesn't have a way to address individual hunks or lines from the command line. You have to go through a diff editor, e.g. the built-in one you get with `jj split -i`. jj-hunk-tool assigns stable IDs to each hunk in a diff so you can refer to them, and to specific line ranges within them, in commands like `split`, `squash`, `diffedit`, and `restore`.

Each command mirrors its jj counterpart — just pass hunk IDs instead of `-i`.

## Install

```
cargo install --git https://github.com/mvzink/jj-hunk-tool.git
```

## Usage

```shell
# List hunks with stable IDs (includes line numbers for range selection)
jj-hunk-tool hunks [-r <rev>] [--compact] [--file <path>]

# Output a patch for selected hunks
jj-hunk-tool patch <hunk-id>... [-r <rev>] [--reverse]

# Split selected hunks out of a revision (like jj split)
jj-hunk-tool split <hunk-id>... [-r <rev>] [-m <msg>] [-p] [-o <rev>] [-A <rev>] [-B <rev>]

# Move selected hunks into another revision (like jj squash)
jj-hunk-tool squash <hunk-id>... [-r <rev>] [--from <rev>] [--into <rev>] [-m <msg>] [-u] [-k]

# Keep only selected hunks in a revision (like jj diffedit)
jj-hunk-tool diffedit <hunk-id>... [-r <rev>] [--from <rev>] [--to <rev>]

# Undo selected hunks from a revision (like jj restore)
jj-hunk-tool restore <hunk-id>... [-c <rev>] [--from <rev>] [--into <rev>]

# Auto-absorb hunks into ancestor commits
jj-hunk-tool absorb [<hunk-id>...] [--dry-run] [-i]

# Line ranges for sub-hunk precision (use `hunks` to see line numbers)
jj-hunk-tool split abc1234:5        # line 5
jj-hunk-tool split abc1234:1-10     # lines 1-10
jj-hunk-tool split abc1234:1-3,7-9  # multiple ranges
```

## Command mapping

| jj command | jj-hunk-tool equivalent | what hunk IDs mean |
|---|---|---|
| `jj split -i` | `jj-hunk-tool split <hunks>` | hunks for the first commit |
| `jj squash -i` | `jj-hunk-tool squash <hunks>` | hunks to move to destination |
| `jj diffedit` | `jj-hunk-tool diffedit <hunks>` | hunks to keep in the revision |
| `jj restore -i` | `jj-hunk-tool restore <hunks>` | hunks to undo |
| `jj absorb` | `jj-hunk-tool absorb` | hunks auto-routed to ancestors |

## Absorb

`jj-hunk-tool absorb` automatically moves hunks from `@` into the mutable ancestor commits that introduced the overlapping code. It's similar to `jj absorb`, but operates at hunk granularity rather than per-line.

### How it works

1. **Annotate**: For each file changed in `@`, runs `jj file annotate` on the parent to find which commit introduced each line.
2. **Route**: For each hunk, checks the deleted/modified lines (`-` lines). If they all blame to a single mutable ancestor, the hunk is routed there. If they blame to multiple ancestors, the hunk is ambiguous and stays in `@`.
3. **Execute**: For each target ancestor, runs `jj-hunk-tool squash --from @ --into <target>` with the tool protocol, moving only the matched hunks.

Key differences from `jj absorb`:
- **Atomic hunks**: Each hunk goes to one target or stays. `jj absorb` can split a single hunk across targets at the line level.
- **File fallback**: When a hunk has no overlapping blamed lines (e.g. pure insertions), falls back to the most recent mutable ancestor that touched the same file. New files stay in `@`.
- **Selective**: Pass specific hunk IDs to absorb only those hunks.

```sh
# Preview what would happen
jj-hunk-tool absorb --dry-run

# Absorb all matched hunks
jj-hunk-tool absorb

# Absorb only specific hunks
jj-hunk-tool absorb abc1234 def5678
```

### Interactive mode

`absorb -i` presents each hunk with its content and proposed target, then prompts for an action:

```
abc1234 src/main.rs (+5 -3)
  1: fn example() {
  2:-    old_code();
  3:+    new_code();
  4: }

Target: kkzuqymt (add feature A)
[a]bsorb / [s]kip / [t]arget / [q]uit:
```

- **a** — Accept the proposed routing
- **s** — Skip this hunk (leave in `@`)
- **t** — Override the target: shows a numbered list of all mutable ancestors to choose from
- **q** — Quit; skip all remaining hunks

This is useful for reviewing what absorb will do, handling ambiguous hunks manually, or redirecting a hunk to a different ancestor than what blame suggests.

## Alias

If you use jj-hunk-tool by hand, consider adding a custom command alias in your jj `config.toml`:

```toml
[aliases]
hunk = ["util", "exec", "--", "jj-hunk-tool"]
```

Then you can use `jj hunk hunks`, `jj hunk split abc1234`, etc.

## jj-surgeon skill

The repo includes a comprehensive agent skill for working with jj in general as well as jj-hunk-tool (revsets, rebasing, conflict resolution, etc.). Install it with [skills](https://github.com/vercel-labs/skills):

```
npx skills add mvzink/jj-hunk-tool
```

## Disclaimer

This is a vibe-coded project. It will eat your dog's homework.
