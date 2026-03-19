---
name: jj-surgeon
description: Comprehensive guide for working with Jujutsu (jj) version control. Use whenever managing, viewing, creating, editing, splitting, squashing, rebasing, reordering, or otherwise reorganizing jj commits and change history — including hunk-level operations, bookmarks, conflict resolution, revsets, and all standard jj workflows.
---

# Jujutsu (jj) Complete Agent Guide

## Key concepts

**Working copy is always a commit.** The working directory is automatically
snapshotted into commit `@` at the start of every jj command. There is no
staging area. All file changes are immediately part of `@`.

**"Clean" means an empty `@`.** When `@` has no diff vs its parent, the working
copy is clean. You do NOT need to `jj abandon` an empty `@` -- it is harmless
and jj creates a new empty `@` automatically after operations that consume it.

**Change IDs vs commit IDs.** Every commit has two identifiers:
- *Change ID* -- stable across rewrites (rebase, amend, squash). Shown as
  reversed-hex letters (k-z). Use this to refer to changes you plan to rewrite.
- *Commit ID* -- content hash (standard hex). Changes on every rewrite. Matches
  the Git SHA in colocated repos. Becomes permanent once immutable.

**No branches, only bookmarks.** Bookmarks are named pointers to commits. They
do NOT advance automatically on new commits (unlike Git branches). They DO
follow when a commit is rewritten. Bookmarks map to Git branches for push/fetch.

**Editing history is safe — but watch for conflicts.** jj rewrites commits
freely and automatically rebases descendants. If a rebase causes overlapping
changes, jj records a conflict in the descendant commit. Conflicts are data (not
blocking states) but must be resolved before the code compiles. Always check for
conflicts after rewriting ancestors. See
[references/conflict-resolution.md](references/conflict-resolution.md).

**Operation log.** Every command creates an operation entry. `jj undo` reverts
the last operation. `jj op restore <id>` jumps to any past state.

## Always pass these flags

```bash
# When viewing diffs:
jj diff --git --no-pager
jj show --git --no-pager @-
jj diff --git --no-pager -r <rev>
jj log --no-pager -p --git -r <revset>

# These prevent pager hangs and ensure machine-readable unified diff output.
```

## jj-hunk-tool commands

Use `jj-hunk-tool` for non-interactive hunk-level selection -- the one thing jj
cannot do without an interactive editor.

```bash
# List hunks with IDs, file paths, +/- counts, and numbered lines
jj-hunk-tool hunks
jj-hunk-tool hunks -r <revset>          # from a specific revision
jj-hunk-tool hunks --file src/main.rs   # filter to one file
jj-hunk-tool hunks --compact            # brief preview (no line numbers)

# Output unified diff patch for selected hunks
jj-hunk-tool patch <id1> <id2:1-10> ...
jj-hunk-tool patch <id>:5-30,40-50 -r <revset>
jj-hunk-tool patch --reverse <id>

# Commit selected hunks from @ (creates new parent, rest stays in @)
jj-hunk-tool commit <id1> <id2> -m "message"
jj-hunk-tool commit <id>:1-11 <id2> -m "partial commit"

# Split a non-@ revision (selected hunks split out, rest stays)
jj-hunk-tool commit <id1> -r <revset> -m "split out"

# Discard selected hunks from working copy
jj-hunk-tool discard <id1> <id2>

# Rewrite a revision in-place keeping only selected hunks
jj-hunk-tool diffedit <id1> <id2> -r <revset>

# Restore selected hunks from a revision (undo specific changes)
jj-hunk-tool restore <id> --from <revset>
```

### Hunk IDs

- 7-char hex strings derived from file path + hunk content
- Stable across runs as long as the diff hasn't changed
- Duplicates get `-2`, `-3` suffixes
- If not found, re-run `hunks` for fresh IDs
- Line ranges: `id:5-30` (1-based, from `hunks` output)
- Multiple ranges: `id:2-6,34-37`

## Native jj commands

### Creating and describing changes

```bash
jj new                                  # new empty change on top of @
jj new <rev>                            # new change on top of <rev>
jj new <rev1> <rev2>                    # new merge commit
jj new -A <rev>                         # insert after <rev>, rebasing descendants
jj new -B <rev>                         # insert before <rev>
jj commit -m "message"                  # set description on @, create new empty @
jj describe -m "message"                # set/update description (default: @)
jj describe <rev> -m "message"          # describe a specific revision
```

### Squashing and absorbing

```bash
jj squash -m "msg"                      # squash @ into its parent
jj squash -r <rev> -m "msg"            # squash <rev> into its parent
jj squash --from <src> --into <dst> -m "msg"  # move changes between revisions
jj absorb                               # auto-distribute @ changes into ancestors by blame
jj absorb --from <rev>                  # absorb from specific revision
```

**Always pass `-m` to `jj squash`.** Without it, jj opens `$EDITOR` to compose
a description from the two commits being merged.

`jj absorb` is extremely powerful: it looks at each changed line, finds the
mutable ancestor that last touched it, and moves the change there. Ambiguous
lines stay in the source. Always review with `jj op show -p` afterward.

**Warning:** `jj squash` rewrites the destination commit, causing all its
descendants to be rebased. If descendants modified the same lines, this creates
conflicts. Check with `jj log -r 'conflicts()'` after squashing and resolve
before continuing. See
[references/conflict-resolution.md](references/conflict-resolution.md).

### Splitting

```bash
jj split -m "first half"               # interactive split of @
jj split -r <rev> -m "first half"      # interactive split of any revision
jj split path/to/file -m "these files" # non-interactive: split by file
```

**Always pass `-m` to `jj split`.** Without it, jj opens `$EDITOR` for each
resulting commit. For non-interactive hunk-level splitting, use
`jj-hunk-tool commit -r`.

### Rebasing and reordering

```bash
jj rebase -r <rev> -o <dest>            # move single commit onto dest (alias: -d)
jj rebase -s <rev> -o <dest>            # move commit + descendants
jj rebase -b <rev> -o <dest>            # move whole branch
jj rebase -r <rev> -A <after>           # insert after (reorder)
jj rebase -r <rev> -B <before>          # insert before
jj rebase -s @ -o main                  # rebase current stack onto main
```

Rebasing can create conflicts in the moved commits if the new base has diverged.
Check `jj log -r 'conflicts()'` after rebasing. See
[references/conflict-resolution.md](references/conflict-resolution.md).

### Undoing and restoring

```bash
jj undo                                 # undo last operation
jj op log --no-pager                    # view operation history
jj op restore <op-id>                   # restore to any past state
jj restore <paths...>                   # restore files in @ from parent
jj restore --from <rev> <paths...>      # restore files from specific revision
jj restore -c <rev>                     # undo changes introduced by <rev>
jj revert -r <rev> -o @                 # create reverse-patch of <rev> on @
jj abandon <rev>                        # drop a revision, rebase descendants
```

### Viewing changes

```bash
jj status                               # working copy status
jj diff --git --no-pager                # working copy diff
jj diff --git --no-pager -r <rev>       # diff of specific revision
jj show --git --no-pager <rev>          # description + diff
jj log --no-pager                       # commit graph
jj log --no-pager -r '<revset>' -p --git  # graph with patches
jj file annotate <path>                 # blame (which change introduced each line)
```

### Bookmarks and pushing

```bash
jj bookmark create <name> -r <rev>      # create bookmark (default rev: @)
jj bookmark set <name> -r <rev>         # move bookmark
jj bookmark delete <name>               # delete bookmark
jj bookmark list --no-pager             # list bookmarks
jj git push -b <name>                   # push bookmark to remote
jj git push -c <rev>                    # create bookmark from change ID and push
jj git fetch                            # fetch from remote
```

### Conflicts

```bash
jj log -r 'conflicts()'                # find commits with conflicts
jj resolve --list                       # list conflicted files in @
jj resolve                              # launch 3-way merge tool for @ conflicts
jj resolve -r <rev>                     # resolve conflicts in specific revision
jj resolve --tool :ours                 # pick "ours" side
jj resolve --tool :theirs               # pick "theirs" side
# Or: just edit the conflict markers directly in the file (recommended for agents)
```

Conflicts are first-class data — a commit can contain conflicts and still be
rebased, squashed, or pushed. **For agents, the most reliable resolution method
is reading the conflicted file and editing out the conflict markers directly.**
jj's conflict markers differ from Git's — see
[references/conflict-resolution.md](references/conflict-resolution.md) for the
format, reading guide, and resolution strategies.

## Revset language

Revsets select sets of commits. Used with `-r` on most commands.

### Symbols

| Symbol | Meaning |
|---|---|
| `@` | Working copy commit |
| `@-` | Parent of `@` (shorthand for `@-1`) |
| `@--` | Grandparent of `@` |
| `root()` | Root commit |
| `trunk()` | Head of default remote's default branch |
| `<change_id>` | Commit by change ID (or unique prefix) |
| `<commit_id>` | Commit by commit ID (or unique prefix) |
| `<bookmark>` | Commit at bookmark |
| `<bookmark>@<remote>` | Remote bookmark |

### Operators

| Syntax | Meaning |
|---|---|
| `x-` | Parents of x |
| `x+` | Children of x |
| `::x` | Ancestors of x (inclusive) |
| `x::` | Descendants of x (inclusive) |
| `x::y` | DAG range: ancestors of y that are descendants of x |
| `x..y` | Set difference: ancestors of y minus ancestors of x |
| `x..` | Everything not an ancestor of x |
| `..x` | Ancestors of x (same as `::x` minus root) |
| `~x` | Complement |
| `x & y` | Intersection |
| `x \| y` | Union |
| `x ~ y` | Difference (x minus y) |

### Useful functions

```
ancestors(x, depth)    descendants(x, depth)   parents(x)    children(x)
heads(x)               roots(x)                connected(x)  fork_point(x)
bookmarks(pattern)     tags(pattern)            trunk()
mine()                 empty()                  merges()       conflicts()
description(pattern)   author(pattern)          files(expr)
mutable()              immutable()              present(x)
latest(x, count)
```

### Common patterns

```bash
trunk()..@             # changes on current stack not yet on trunk
@::                    # @ and all descendants
ancestors(@, 5)        # last 5 ancestors
mutable() & ancestors(@)  # mutable ancestors of @
description("fixup")   # commits with "fixup" in description
mine() & mutable()     # my mutable commits
```

## Workflows

### Hunk-level commit (selective commit from @)

```bash
jj-hunk-tool hunks                         # list hunks with line numbers
jj-hunk-tool commit <id1> <id2> -m "msg"   # commit selected hunks
# Remaining hunks stay in @
```

### Hunk-level split of historical revision

```bash
jj-hunk-tool hunks -r <rev>                # list with line numbers
jj-hunk-tool commit <id>:1-20 -r <rev> -m "first part"
# Rest stays in <rev>; descendants auto-rebased
jj log -r 'conflicts()'                    # check for conflicts!
```

**This operation rewrites `<rev>`, so all descendants are rebased.** If
descendants touch the same code, conflicts will appear. Resolve them before
continuing — see
[references/conflict-resolution.md](references/conflict-resolution.md).

### Blame-guided fixup

```bash
jj file annotate src/main.rs               # find which change touched each line
jj-hunk-tool hunks                         # list current hunks
jj-hunk-tool commit <id> -m "fix bug"      # commit the fix
jj squash --from @- --into <target>        # fold into the original change
```

### Auto-fixup with absorb

```bash
# Make fixes in working copy, then:
jj absorb                                  # auto-distributes to correct ancestors
jj op show -p --no-pager                   # review what happened
```

### Stacking changes

```bash
jj describe -m "feature part 1"
jj new                                     # start next change
# ... work ...
jj describe -m "feature part 2"
jj new                                     # and so on
jj log --no-pager -r 'trunk()..@'          # see the stack
```

### Reorder commits in a stack

```bash
jj rebase -r <rev> -A <after>             # move <rev> after <after>
jj rebase -r <rev> -B <before>            # move <rev> before <before>
```

### Push a stack

```bash
jj bookmark set feature -r @
jj git push -b feature
```

### Undo mistakes

```bash
jj undo                                    # undo last operation
jj op log --no-pager                       # find operation to restore
jj op restore <op-id>                      # restore to any point
```

## Common pitfalls for agents

- Do NOT `jj abandon @` to "clean up" an empty working copy. It's normal.
- Do NOT use `git` commands in a jj repo. Always use `jj`.
- Always pass `--git --no-pager` when viewing diffs.
- Always pass `--no-pager` to `jj log`, `jj op log`, `jj bookmark list`.
- **Always pass `-m "message"` to `jj commit`, `jj describe`, `jj squash`,
  `jj split`, and any other command that accepts it.** Omitting `-m` opens
  `$EDITOR`, which hangs in non-interactive contexts and disrupts the user.
  This applies even when using `--from`/`--into` with squash.
- `jj diff` with no `-r` shows `@` vs parent. Use `-r <rev>` for other revisions.
- After `jj commit -m "msg"`, the described change is `@-` (the parent). `@` is
  the new empty working copy.
- Immutable commits (on trunk, tags, remote bookmarks) cannot be rewritten.
  Use `mutable()` revset to find what you can edit.
- `jj squash` without args squashes `@` into `@-`. With `--from`/`--into` you
  can squash between any two mutable commits.
- After any history rewrite (`jj edit` + modify, `jj squash`, `jj rebase`,
  `jj-hunk-tool commit -r`), check `jj log -r 'conflicts()'` for conflicts.
  Resolve them immediately before editing further descendants — cascading
  conflicts are much harder to fix. See
  [references/conflict-resolution.md](references/conflict-resolution.md).
