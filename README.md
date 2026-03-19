# jj-hunk-tool

Hunk-level operations for [Jujutsu (jj)](https://github.com/jj-vcs/jj), built on [git-surgeon](https://github.com/raine/git-surgeon).

jj is great at rewriting history at the commit and file level (split, squash, rebase, etc.), but it doesn't have a way to address individual hunks or lines from the command line. You have to go through a diff editor, e.g. the built-in one you get with `jj split -i`. jj-hunk-tool assigns stable IDs to each hunk in a diff so you can refer to them, and to specific line ranges within them, in commands like `commit`, `discard`, `diffedit`, and `restore`.

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

# Commit selected hunks into a new change
jj-hunk-tool commit <hunk-id>... [-r <rev>] [-m <message>]

# Discard selected hunks
jj-hunk-tool discard <hunk-id>... [-r <rev>]

# Keep only selected hunks in a revision (remove the rest)
jj-hunk-tool diffedit <hunk-id>... [-r <rev>]

# Restore selected hunks from one revision into another
jj-hunk-tool restore <hunk-id>... --from <rev> [--to <rev>]

# Line ranges for sub-hunk precision (use `hunks` to see line numbers)
jj-hunk-tool commit abc1234:5        # line 5
jj-hunk-tool commit abc1234:1-10     # lines 1-10
jj-hunk-tool commit abc1234:1-3,7-9  # multiple ranges
```

## Alias

If you use jj-hunk-tool by hand, consider adding a custom command alias in your jj `config.toml`:

```toml
[aliases]
hunk = ["util", "exec", "--", "jj-hunk-tool"]
```

Then you can use `jj hunk hunks`, `jj hunk commit abc1234`, etc.

## jj-surgeon skill

The repo includes a comprehensive agent skill for working with jj in general as well as jj-hunk-tool (revsets, rebasing, conflict resolution, etc.). Install it with (defaults to Claude Code location):

```
jj-hunk-tool install-skill
```

## Disclaimer

This is a vibe-coded project. It will eat your dog's homework.
