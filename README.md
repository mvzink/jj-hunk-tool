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

## Testing

Unit tests are plain Rust; everything below that builds real repositories needs
`jj` on `PATH`:

```sh
cargo nextest run                                   # everything, 32 Hegel cases per property
cargo nextest run --test coverage_contract          # the coverage-marker contract
HEGEL_TEST_CASES=200 cargo nextest run --test properties \
    --test properties_shapes --test properties_apply  # the CI profile
HEGEL_DEFAULT_PROFILE=soak cargo nextest run --test properties  # 2000-case soak
```

`hegel.toml` sets the case counts per profile (`development` 32, `ci` 200,
`soak` 2000). Measured on the 8-core reference box, wall clock for the whole
suite: **30 s at 32 cases per property** and **2 min 58 s at 200** (nextest runs
the test binaries in parallel; per case a property costs one `jj git init`, one
or two commits and a handful of tool invocations, so the dominant cost is
subprocesses, not generation).

The properties live in `tests/properties.rs` (non-UTF-8 diff handling, issue #1),
`tests/properties_shapes.rs` (file/hunk shapes, line spans, ids, unusual paths)
and `tests/properties_apply.rs` (selection, payload construction, every apply
mode). Their oracles are independent of the implementation: `jj diff --git` bytes
are re-parsed in the test, `hunks` output is re-parsed as text, and expected file
contents come from applying a hand-built patch with `git apply`.

## Coverage markers

The suite asserts *reachability*, not just outcomes: a test that claims to cover
rename-shaped diffs fails if no rename ever reached the parser, and an apply mode
that stops calling a helper fails instead of passing quietly. The facility lives
in [`src/markers.rs`](src/markers.rs) and is always compiled — no `cfg(test)`, no
feature flag, no dependency.

- `Marker` is a closed set of namespaced names (`parse/hunk_path_change`,
  `apply/restore`, …), declared in one `marker_set!` table.
- `hit!(Marker)` records one reached path. It is a single relaxed atomic store:
  no allocation, no lock, and near-free on hot paths.
- `markers::REQUIRED` is the contract: the markers the suite promises to reach.
- `markers::assert_required_hit` / the harness's `Markers::require` fail loudly,
  naming every missing marker.
- `JJ_HUNK_TOOL_MARKERS=1` makes the binary print its hit set to stderr on exit
  (success and failure alike), which is how integration tests observe a
  subprocess. Unset, the cost is one `getenv` at exit.

Adding a marker is a two-place edit in `src/markers.rs`: append
`Variant => "area/name",` to the `marker_set!` table (the enum, `ALL`, `name()`
and the hit array are generated), and — only if a deterministic test reaches it —
add `Marker::Variant` to `REQUIRED`. Declaring without requiring is allowed and
useful; requiring is a promise, and `tests/coverage_contract.rs` holds the suite
to it.

The contract itself is `tests/coverage_contract.rs`: one deterministic run that
drives every required marker (including the absorb branches that need an
immutable ancestor, the `_jj-tool` protocol driven directly, and every error
path) and asserts `REQUIRED ⊆ hit`. Two markers describe diff shapes `jj` cannot
emit (`copy from`/`copy to`, `similarity index`); they are reached in-process
through the same library entry point the binary uses.

One further marker is placed but not required: `absorb/new_file` sits on a branch
that cannot be taken while `absorb` compares the absent side against `"/dev/null"`
instead of the `dev/null` the parser stores. It is on the same footing as the two
`offset/*` fallbacks above — a marker kept for the code it guards, deliberately
outside the contract until the code can reach it.

| marker | where it is placed | what it proves |
|---|---|---|
| `diff/section_decoded` | `diff::decode_diff`, accepted section | a decodable file section reached the parser |
| `diff/section_undecodable_skipped` | `diff::decode_diff`, rejected section | a non-UTF-8 section was skipped, not decoded |
| `diff/sections_multiple` | `diff::decode_diff` | a diff with more than one file section |
| `parse/no_hunks` | `markers::observe_hunks`, empty diff | the no-change path |
| `parse/hunk_modified` | `observe_hunks` | a hunk with both `+` and `-` lines |
| `parse/hunk_additions_only` | `observe_hunks` | a pure-insertion hunk |
| `parse/hunk_deletions_only` | `observe_hunks` | a pure-deletion hunk |
| `parse/hunk_new_file` | `observe_hunks` | a `--- /dev/null` hunk |
| `parse/hunk_deleted_file` | `observe_hunks` | a `+++ /dev/null` hunk |
| `parse/hunk_path_change` | `observe_hunks` | a hunk whose old and new paths differ |
| `parse/hunk_no_newline_marker` | `observe_hunks` | a hunk carrying `\ No newline at end of file` |
| `parse/duplicate_id_suffix` | `observe_hunks` | two hunks hashed to the same id prefix |
| `parse/unsupported_rename` / `…_copy` / `…_mode_change` / `…_similarity` | `observe_hunks` | metadata a hunk-level operation cannot reproduce |
| `select/*` (8 markers) | `markers::observe_selection` | every selection shape: none, all, single, subset, interleaved, first, last, line ranges |
| `patch/whole_hunk`, `patch/reverse` | `tool::build_combined_patch` | the two payload branches |
| `patch/supported_hunk` | `diff::require_supported`, success | a hunk that can be moved |
| `apply/patch_only` | `main`, `patch` command | the read-only payload path |
| `apply/split`, `apply/split_parallel`, `apply/squash`, `apply/diffedit`, `apply/restore` | `main`, one per command | each apply mode was invoked |
| `apply/absorb_dry_run`, `apply/absorb_execute` | `tool::absorb_hunks` | absorb planned, then wrote |
| `apply/jj_tool`, `apply/jj_tool_reverse` | `tool::jj_tool_apply` | the `--tool` protocol applied a payload, forward and reverse |
| `absorb/matched_change`, `absorb/matched_file`, `absorb/ambiguous`, `absorb/no_overlap` | `tool::absorb_hunks` routing | each reachable routing decision |
| `absorb/new_file` | `tool::absorb_hunks`, new-file branch | **not required yet**: that branch compares `old_file` against `"/dev/null"` while the parser stores `dev/null`, so it never runs; it joins `REQUIRED` once that comparison is fixed |
| `error/hunk_not_found`, `error/unsupported_metadata`, `error/no_hunks_selected`, `error/invalid_range_start`/`…_end`/`…_line_number`, `error/range_bounds`, `error/jj_command_failed`, `error/patch_apply_failed`, `error/missing_patch_env` | every bail/callback site | each user-reachable failure path |
| `offset/header_ranges_fallback`, `offset/old_range_unparsed` | `tool::parse_header_ranges`, `tool::absorb_hunks` | **not required**: defensive fallbacks for a malformed `@@` header, which `jj` never emits |

## Disclaimer

This is a vibe-coded project. It will eat your dog's homework.
