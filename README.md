# jj-hunk-tool

Hunk-level operations for [Jujutsu (jj)](https://github.com/jj-vcs/jj), built on [git-surgeon](https://github.com/raine/git-surgeon).

jj is great at rewriting history at the commit and file level (split, squash, rebase, etc.), but it doesn't have a way to address individual hunks or lines from the command line. You have to go through a diff editor. jj-hunk-tool assigns stable IDs to each hunk in a diff so you can refer to them, and to specific line ranges within them, in commands like `commit`, `discard`, `diffedit`, and `restore`.

## Install

```
cargo install --git https://github.com/mvzink/jj-hunk-tool.git
```

## Usage

```
# List hunks with stable IDs
jj-hunk-tool hunks [-r <rev>] [--full] [--file <path>]

# Show a specific hunk with line numbers
jj-hunk-tool show <hunk-id> [-r <rev>]

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

# Line ranges for sub-hunk precision (use `show` to see line numbers)
jj-hunk-tool commit abc1234:5        # line 5
jj-hunk-tool commit abc1234:1-10     # lines 1-10
jj-hunk-tool commit abc1234:1-3,7-9  # multiple ranges
```

## jj-surgeon skill

The repo includes a comprehensive Claude Code skill for working with jj, not just jj-hunk-tool but jj in general (revsets, rebasing, conflict resolution, etc.). Install it with:

```
jj-hunk-tool install-skill
```

## Disclaimer

This is a vibe-coded project. It will eat your dog's homework. I don't care, it's making me more productive.
