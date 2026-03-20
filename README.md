# jj-hunk-tool

Hunk-level operations for [Jujutsu (jj)](https://github.com/jj-vcs/jj), built on [git-surgeon](https://github.com/raine/git-surgeon).

jj is great at rewriting history at the commit and file level (split, squash, rebase, etc.), but it doesn't have a way to address individual hunks or lines from the command line. You have to go through a diff editor, e.g. the built-in one you get with `jj split -i`. jj-hunk-tool assigns stable IDs to each hunk in a diff so you can refer to them, and to specific line ranges within them, in commands like `split`, `squash`, `diffedit`, and `restore`.

Each command mirrors its jj counterpart — just pass hunk IDs instead of `-i`.

## Install

```
cargo install --git https://github.com/mvzink/jj-hunk-tool.git
```

## Usage

```
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

## Common patterns

```sh
# Discard specific hunks from working copy (like git checkout -p)
jj-hunk-tool restore <hunk-id>...

# Split working copy into two commits
jj-hunk-tool split <hunk-id>... -m "first part"

# Move specific hunks from @ into parent
jj-hunk-tool squash <hunk-id>... -m "squashed"

# Split a historical revision
jj-hunk-tool split <hunk-id>... -r <rev> -m "extracted"
```

## Alias

If you use jj-hunk-tool by hand, consider adding a custom command alias in your jj `config.toml`:

```toml
[aliases]
hunk = ["util", "exec", "--", "jj-hunk-tool"]
```

Then you can use `jj hunk hunks`, `jj hunk split abc1234`, etc.

## jj-surgeon skill

The repo includes a comprehensive agent skill for working with jj in general as well as jj-hunk-tool (revsets, rebasing, conflict resolution, etc.). Install it with (defaults to Claude Code location):

```
jj-hunk-tool install-skill
```

## Disclaimer

This is a vibe-coded project. It will eat your dog's homework.
