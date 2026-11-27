# Coverage expansion + reachability markers — review report

Branch: `test/property-coverage-markers` (local only; nothing pushed, no PR, no
issue comments).
Started from `bcd02e2` — the tip of `test/non-utf8-proptests` ("test: keep
undecodable bytes in generated diffs and set Hegel case counts"), which is the
existing six-property Hegel work plus `hegel.toml` (development 32 / ci 200 /
soak 2000) and `hegeltest =0.54.2`.

```
0ff132d docs: document the testing layers and the coverage-marker contract
6b6292d test: make the shape property's coverage requirement absolute
1c6f3ce test: assert the coverage-marker contract in one deterministic run
ee3c73a test: harness support for the protocol, reverse payloads and repo config
c6f8eb9 test: property tests for selection, payloads and every apply mode
450d740 test: apply payloads with `git apply`, the strict applier
1dab207 test: property tests for diff shapes, hunk spans, ids and unusual paths
08cda2f test: add oracles for sections, hunk spans and hunk ids to the harness
58e09cf fix: take a `--file` path verbatim even when it begins with a hyphen
6b569e8 fix: recognise an absent diff side as `dev/null`, not `/dev/null`
10ed90c test: extract the property harness into tests/harness/mod.rs
d33ca5b feat: add reachability ("coverage") markers to the paths that matter
```

`git diff --stat bcd02e2..HEAD`: 13 files, +4371 −731. Two of those commits are
bug fixes that the new coverage found (`6b569e8`, `58e09cf`), three are test
infrastructure, three are property suites, one is the marker facility, one the
marker contract, one documentation. Each commit is a valid state: `cargo fmt
--check`, `cargo clippy --all-targets -D warnings` and the full suite were run
on every one before committing.

---

## 1. Code map and priority ordering

Public surface, per `src/*.rs` (the crate is now a library plus the binary, so
that tests can drive identification in-process — see §3):

| file | contents | entry points |
|---|---|---|
| `src/main.rs` (≈600 lines) | clap surface, command dispatch, `parse_id_range`, `resolve_hunk_specs` | `hunks`, `patch`, `split`, `squash`, `diffedit`, `restore`, `absorb`, hidden `_jj-tool`; global `--debug` |
| `src/diff.rs` (≈240 lines) | jj subprocess wrappers + the section-wise UTF-8 decoder | `get_jj_diff`, `get_jj_diff_from_to`, `decode_diff` (private), `require_supported`, re-exports `parse_diff` / `assign_ids` / `check_supported` from git-surgeon |
| `src/tool.rs` (≈1190 lines) | payload building, jj `--tool` invocation, absorb routing, `_jj-tool` protocol, hunk display | `build_combined_patch`, `split_hunks`, `squash_hunks`, `diffedit_hunks`, `restore_hunks`, `absorb_hunks`, `jj_tool_apply`, `HunkFingerprint`, `parse_hdr_ranges`/`parse_old_range` (private) |
| `src/markers.rs` (≈430 lines, new) | marker registry, hit set, emission, `REQUIRED`, the two observation helpers | `Marker`, `ALL`, `hit`/`hit!`, `is_hit`, `reset`, `hit_names`, `render`, `parse_emitted`, `emit_if_requested`, `missing`, `assert_required_hit`, `observe_hunks`, `observe_selection`, `REQUIRED` |
| `src/lib.rs` (new) | `pub mod diff; pub mod markers; pub mod tool;` | — |

CLI surface (behaviour that can be observed from outside the process):

- `hunks [-r REV] [--compact] [--file PATH]` — listing, `--file` filter, two
  rendering modes.
- `patch <ids>... [-r REV] [--reverse]` — payload only, no mutation.
- `split <ids>... [-r REV] [-m MSG] [-p/--parallel] [-o/-A/-B REV]...`
- `squash <ids>... [-r REV] [--from REV]... [--into REV] [-m MSG] [-u] [-k]`
- `diffedit <ids>... [-r REV] [--from REV] [--to REV] [--restore-descendants]`
- `restore <ids>... [--from REV] [--into REV] [-c REV] [--restore-descendants]`
- `absorb [<ids>...] [-r REV] [--dry-run] [-i/--interactive]`
- `_jj-tool LEFT RIGHT` — the jj `--tool` protocol, configured by
  `JJ_HUNK_TOOL_PATCH` / `JJ_HUNK_TOOL_REVERSE`.

Priority ordering used to decide what got a property, highest first:

1. **Diff decoding and hunk identification** — everything downstream is
   unreachable if a file/hunk shape is dropped, merged or mis-addressed
   (issue #1 was exactly this class). Covered by shapes, spans, ids.
2. **Hunk selection and payload construction** — the payload is what the user
   inspects and what gets committed; a divergence between them is silent
   corruption. Covered by selection, ranges, payload-vs-split.
3. **Every apply mode** — `split`/`squash`/`diffedit`/`restore`/`absorb` each
   rewrite history; asserting "exit 0" is worthless here, so each asserts a
   content model. Covered by four modes + absorb.
4. **Absorb routing** — blame-based routing can put a change in the wrong
   commit; also the only place with five distinct outcomes.
5. **Unusual paths** — legal on disk, easy to lose in a textual pipeline.
6. **Error paths** — must fail loudly and name the problem, never print a
   partial payload.
7. **`--compact` rendering, `--debug` output, interactive absorb UI** — lowest:
   cosmetic, or covered by the pre-existing integration suite.

---

## 2. Part 1 — what each property proves, and which bug class it kills

Existing six (unchanged, `tests/properties.rs`; harness moved out): non-UTF-8
aborts, `--file` isolation, byte-exactness of unselected files, hunk accounting,
selection round-trip, body-less rename.

New, `tests/properties_shapes.rs`:

| property | proves | kills |
|---|---|---|
| `listing_matches_jj_for_every_file_shape` | for all twelve shapes (modified, no-trailing-newline, CRLF, very long line, added, added-empty, deleted, binary, mode-only, mode+content, identical rename, rename+content) the listing has exactly jj's `@@` blocks — count, body lines, and the `(+A −D)` header counts; hunkless shapes stay unlisted and hence unselectable; ids unique; listing deterministic; a selected hunk yields jj's block byte for byte or a refusal that names the metadata and prints nothing; a reverse payload applied backwards to the head reproduces the base | a hunk lost/merged so it cannot be addressed; a metadata-only or hunkless file becoming selectable; partial output before a refusal; a dropped or invented shape (requirement is absolute, see below) |
| `hunk_headers_describe_the_lines_they_cover` | every `@@ -a,b +c,d @@` agrees with its body *and* with the file contents: old side == `base[a−1 .. a−1+b]`, new side == `head[c−1 .. c−1+d]`; regions at file start/end/middle and whole-file; every changed line appears in exactly one hunk | off-by-one and uneven-context offsets (the `fix/uneven-context` class), headers drifting from bodies, `-N,0`/`-0,0` conventions mangled |
| `hunk_ids_are_content_addressed_and_stable` | ids equal sha1(path + body lines)[..7] with `-2`, `-3` suffixes, recomputed with an independent hand-written SHA-1; stable across runs, invariant to unrelated files changing, and a one-character change moves exactly one id | ids that drift (scripting breaks), undetected collisions (wrong hunk selected), ids no longer a function of content |
| `unusual_paths_are_listed_and_selected_faithfully` | paths with spaces, quotes, non-ASCII, leading/trailing spaces and names that look like diff syntax (`diff --git a/…`, `--- …`, `@@ -1 +1 @@ …`) list and select exactly like any other path | path handling that loses a file whose name looks like a patch header, quoting that makes hunks unaddressable |

New, `tests/properties_apply.rs`:

| property | proves | kills |
|---|---|---|
| `payload_is_exactly_the_selected_hunks` | for six selection shapes (all, first, last, middle, contiguous run, interleaved) the payload is jj's blocks verbatim in the given id order; empty selection is a no-op, exit 0 | dropped/re-ordered/duplicated/re-rendered hunks (trailing whitespace, CR), an empty selection treated as an error, a selection branch that stops being taken |
| `inline_ranges_slice_hunks_faithfully` | `id:start-end` keeps the hunk's context, contains exactly the named change lines, never turns a context line into a change, recomputes header counts, and applies to base with exactly the selected lines changed | off-by-one on the 1-based body index, dropped context (inapplicable patch), stale header arithmetic |
| `split_moves_exactly_the_selected_hunks` | `@-` == base + selected hunks; `@` still holds head; files with no selected hunk are byte-identical | over-application (rewriting unrelated files), no-ops, losing the remainder |
| `squash_moves_exactly_the_selected_hunks` | destination == base + selected; working copy keeps head; `jj diff @` still shows exactly the unselected changes | wrong hunks moved into the destination, unselected work dropped |
| `diffedit_keeps_exactly_the_selected_hunks` | the revision holds base + selected only | keeps everything/nothing, wrong subset |
| `restore_undoes_exactly_the_selected_hunks` | `@` == head − selected == base + unselected (reverse apply path) | reverting the wrong hunks, reverting everything, half-applied files |
| `absorb_routes_hunks_to_their_introducing_ancestor` | two commits introduce a file each; each change lands in the commit its blame names; a brand-new file stays in `@`; `--dry-run` routes identically without writing; `@`'s bytes are unchanged | routing to the wrong ancestor (rewriting history with someone else's change), absorbing a new file into an impossible ancestor, a dry run that mutates |
| `printed_payload_and_split_agree` | content from applying the *printed* payload with `git apply` equals what `split` committed, for the same ids | "the patch looks right but the commit does not" |

`tests/coverage_contract.rs` (1 test) is the marker contract, described in §3.

Hegel idiom: no `tc.assume` anywhere; constraints are expressed by construction
(distinct tags per file so a line can only match one place; 8-line spacing so
hunks cannot merge; `uniquify` for paths); dependent inputs use
`#[hegel::composite]` (`shape_case`, `text_case`, `selection`, `spicy_path`);
`tc.note` records the case, the selection shape, the hunk header and the
coverage reached for every failure.

---

## 3. Part 2 — the marker facility

Design constraints from the brief: minimal, dependency-light, weighted toward
free, compiled in release, usable from Hegel properties *and* ordinary unit
tests, no service, no network — and it must **fail loudly**.

`src/markers.rs` (always compiled, zero dependencies beyond the already-present
`git_surgeon` type used by one helper):

- **Registry**: one `marker_set! { Variant => "area/name", … }` table generates
  `pub enum Marker`, `Marker::ALL`, `Marker::name()` and
  `static HITS: [AtomicBool; N]`. Adding a marker is a one-line table edit plus,
  if it is contractual, one line in `REQUIRED`.
- **`hit(marker)` / `hit!(Variant)`**: one relaxed atomic store. No allocation,
  no lock, no branch, and no `cfg` gate — release builds mark too.
- **Observation helpers** (the only two places that mark without branching, both
  on mandatory single paths): `observe_hunks(&[(String, &DiffHunk)])`, called by
  the one `identify()` helper every command uses, and
  `observe_selection(selected, all, line_ranges)`, called from
  `resolve_hunk_specs` for every apply mode and from `absorb`.
- **Contract**: `REQUIRED: &[Marker]`, `missing()`, `assert_required_hit()`
  (panics naming every missing marker, `#[track_caller]`).
- **Emission**: `emit_if_requested()` prints `coverage-marker: <name>` lines, in
  `Marker::ALL` order, to stderr at exit — from `main`'s single exit path, so a
  failing command still reports the paths it reached. Enabled by
  `JJ_HUNK_TOOL_MARKERS` (any non-empty value); unset, the cost is one `getenv`.
  The variable is inherited by `jj`, so the internal `_jj-tool` child reports to
  the parent's stderr and `apply/jj_tool` is observable from outside.
- **Test-side**: `tests/harness/mod.rs` sets the variable on *every* tool run and
  carries the markers in `ToolRun`, so a property cannot forget to ask whether it
  exercised anything; `Markers` unions markers across runs and
  `Markers::require(&[…], what)` fails naming what is missing.

### Where markers are placed

Full table with rationale in `README.md` ("Coverage markers"); summary of the
required set (52 markers):

- decoding — `diff/section_decoded`, `diff/section_undecodable_skipped`,
  `diff/sections_multiple`;
- identification — `parse/no_hunks`, `parse/hunk_modified`,
  `parse/hunk_additions_only`, `parse/hunk_deletions_only`,
  `parse/hunk_new_file`, `parse/hunk_deleted_file`, `parse/hunk_path_change`,
  `parse/hunk_no_newline_marker`, `parse/duplicate_id_suffix`,
  `parse/unsupported_rename`, `parse/unsupported_copy`,
  `parse/unsupported_mode_change`, `parse/unsupported_similarity`;
- selection — `select/no_hunks`, `select/all_hunks`, `select/single_hunk`,
  `select/subset`, `select/interleaved`, `select/first_hunk`,
  `select/last_hunk`, `select/line_ranges`;
- payload — `patch/whole_hunk`, `patch/reverse`, `patch/supported_hunk`;
- apply modes — `apply/patch_only`, `apply/split`, `apply/split_parallel`,
  `apply/squash`, `apply/diffedit`, `apply/restore`, `apply/absorb_dry_run`,
  `apply/absorb_execute`, `apply/jj_tool`, `apply/jj_tool_reverse`;
- absorb routing — `absorb/new_file`, `absorb/matched_change`,
  `absorb/matched_file`, `absorb/ambiguous`, `absorb/no_overlap`;
- errors — `error/hunk_not_found`, `error/unsupported_metadata`,
  `error/no_hunks_selected`, `error/invalid_range_start`,
  `error/invalid_range_end`, `error/invalid_line_number`, `error/range_bounds`,
  `error/jj_command_failed`, `error/patch_apply_failed`,
  `error/missing_patch_env`.

Declared but **not** required (documented in `REQUIRED`, in the README table and
in the source): `offset/header_ranges_fallback` and `offset/old_range_unparsed`,
both defensive fallbacks for a malformed `@@` header, which no command can ever
consume — no deterministic test can reach them without synthesising input the
tool would never see. The reachable offset/context behaviour is asserted directly
by the hunk-span and header-arithmetic properties instead.

### The assertion that fails loudly

`tests/coverage_contract.rs::required_markers_are_all_exercised` runs, in one
process, ~20 deterministic scenarios in real repositories (all twelve shapes at
once, four hunks for every selection branch, each apply mode, every absorb
routing outcome including the immutable-ancestor branch via a repo-scoped
`immutable_heads()`, the `_jj-tool` protocol forward/inverse/inapplicable/unset,
and every user-reachable error), plus two in-process synthetic diffs for the two
shapes `jj` cannot emit, then asserts `REQUIRED ⊆ hit`.

---

## 4. Deliberate-failure demonstrations

**(a) Contract level — remove one `hit!` call.** Commenting out
`crate::hit!(AbsorbNewFile)` in `tool.rs`:

```
$ cargo nextest run --test coverage_contract
 FAIL [ 1.467s] jj-hunk-tool::coverage_contract required_markers_are_all_exercised
 thread 'required_markers_are_all_exercised' panicked at tests/coverage_contract.rs:563:13:
 the coverage contract (tests/coverage_contract.rs) did not exercise 1 coverage
 marker(s): absorb/new_file
 Summary [ 1.468s] 1 test run: 0 passed, 1 failed, 0 skipped
```

After restoring the call:

```
 PASS [ 1.345s] jj-hunk-tool::coverage_contract required_markers_are_all_exercised
 Summary [ 1.345s] 1 test run: 1 passed, 0 skipped
```

**(b) Generator level — a shape stops being produced.** Filtering
`RenamedAndModified` and `ModeAndModified` out of the generated catalogue:

```
$ cargo nextest run --test properties_shapes -E 'test(listing_matches)'
 FAIL [ 23.720s] jj-hunk-tool::properties_shapes listing_matches_jj_for_every_file_shape
 thread 'listing_matches_jj_for_every_file_shape' panicked at tests/properties_shapes.rs:...:
 identifying every generated shape did not exercise 4 coverage marker(s):
 parse/hunk_path_change, parse/unsupported_rename, parse/unsupported_mode_change,
 error/unsupported_metadata
 Summary [ 23.721s] 1 test run: 0 passed, 1 failed, 3 skipped
```

After restoring the catalogue:

```
 PASS [ 14.145s] jj-hunk-tool::properties_shapes listing_matches_jj_for_every_file_shape
 Summary [ 14.145s] 1 test run: 1 passed, 0 skipped
```

Note what demo (b) exposed during development: with the first version of the
property (1–4 drawn shapes per case, asserting only the drawn shapes' markers),
removing two shapes from the catalogue **passed**. That circular requirement is
why the property now carries the whole catalogue in every case and requires a
fixed `ALL_SHAPE_MARKERS` list (commit `6b6292d`).

A third, real instance of the mechanism working: `absorb/new_file` was
unreachable because `absorb` compared `old_file == "/dev/null"` while the parser
stores `dev/null` (fixed in `6b569e8`). The marker found a branch that had never
run.

---

## 5. Gates and timings

Reference box: caduceus, 8 cores, jj 0.44.0, rustc 1.97.1, GNU patch 2.8.

```
cargo fmt --check                                        -> clean
cargo clippy --all-targets -- -D warnings                -> clean
cargo nextest run                                        -> 152 tests, all pass
cargo test                                               -> 17 lib + 10 bin unit + 1 contract
                                                            + 106 integration + 6 + 8 + 4 properties,
                                                            all pass
```

Wall clock (whole suite, nextest, 32 cases per property — the `development`
profile): **30.4 s** reported by nextest, **30.6 s** process wall. Per property
binary: `properties` 10.4 s (slowest test), `properties_shapes` 17.1 s,
`properties_apply` 18.3 s, `coverage_contract` 2.8 s; binaries run in parallel.

At the `ci` case count (200 per property, `HEGEL_TEST_CASES=200 cargo nextest
run`): **175.9 s** reported, **2 min 58 s** wall, 152 tests, all pass (3 tests
flagged slow by nextest's 60 s threshold).

Baseline for comparison, same box, `bcd02e2`: 139 tests, 6 properties, 10.0 s
reported / 14.0 s wall at 32 cases.

Case counts were not lowered: `hegel.toml` is untouched (32 / 200 / 2000), and
`HEGEL_TEST_CASES` still overrides. Each case keeps its repository small (one
`jj git init`, one or two commits, up to a dozen small files).

---

## 6. What I could not express well, and why

1. **CRLF bodies are not compared byte for byte.** `git-surgeon`'s parser splits
   with `str::lines()`, which strips `\r`, so CRLF hunks are listed and re-emitted
   without carriage returns. Pre-existing bug, unrelated to this work and out of
   scope; CRLF cases therefore assert hunk counts, listing identity and markers
   only. Documented in the file header. Fixing it is a separate change in the
   dependency (or in how the tool decodes), and should come with the `patch`
   round-trip this suite cannot currently make.
2. **Paths containing spaces cannot be applied by `patch(1)`.** `patch -p1`
   cannot open `a b.txt` at all, so the mutation properties use ASCII, no-space
   paths; the space cases assert listing and payload byte-exactness (the payload
   is correct) but not application. This is an applier limitation, not a tool
   limitation — but it does mean `split`/`restore` are unusable on such paths,
   which the tool does not currently warn about.
3. **`split <id>:start-end` could not be asserted end to end on this machine.**
   Slicing turns unselected deletions into context, which grows context past
   three lines; this build of GNU patch 2.8 rejects such a unified diff (reduced
   reproduction below), while `git apply` accepts it. The property therefore
   validates the payload with `git apply` and a splice model, and the *tool-level*
   range apply path is exercised only by the pre-existing `patch <id>:range`
   integration tests. On a working `patch` the mode is fine; the fail-path in
   `tool::jj_tool_apply` is correct.
4. **GNU patch 2.8 on this box is not usable as a strict oracle**: `-F 0` reports
   "succeeded … with fuzz 1" for an exact match with leading *and* trailing
   context, and default fuzz rejects hunks with ≥6 leading context lines plus
   additions. Both reproduce with hand-written diffs against hand-written files
   (`/tmp` scratch, no tool involved). Hence `450d740`: the harness applies
   payloads with `git apply`, which never fuzzes. The tool's own applier is
   unchanged.
5. **`jj` never emits `copy from`/`copy to` or `similarity index`.** Those two
   shape markers are only reachable from synthetic diff text, so they are
   exercised in-process (parse + observe) instead of through a repository. The
   suite is honest about it in the contract test and the README.
6. **Two defensive fallbacks are unreachable** (`offset/*`, see §3). Requiring
   them would mean synthesising a malformed `@@` header that no command can
   consume. A future second diff backend could reach them; the marker stays
   placed for that, but it is not part of the contract.
7. **Rename detection is jj's decision, not the generator's.** `jj` folds a
   delete/add pair with similar content into a body-less rename, so an added file
   next to a similar deleted file silently stops testing additions-only hunks.
   The generator avoids the trap by construction (short added content, long
   deleted content, distinct tags) and the marker requirement now fails loudly if
   it ever happens anyway — but the property cannot state "this file must be an
   addition" as an invariant of the input, only as a requirement on the output.
8. **Bookmark-based immutability is load-bearing for `absorb/no_overlap`.**
   Reaching "nothing mutable overlaps" needs an immutable ancestor, which needs a
   repo-scoped `revset-aliases."immutable_heads()"` plus a bookmark. That is
   deterministic and documented, but it is a configuration dependency in a test —
   if jj changes how repo config is layered, that scenario breaks first.
9. **The environment needed a linker shim.** This box's rustup `ld.lld` wrapper
   points at a garbage-collected `/nix/store` path, so every build failed; I ran
   gates with `RUSTFLAGS="-C linker=$HOME/.cache/linkfix/cc-nolld"` (a shim that
   drops `-fuse-ld=lld` and execs the system gcc). Nothing in the repo depends on
   it.
10. **The soak profile was not run at 2000 cases here** (≈30 min for the suite at
    the new property count). The profile resolves and runs correctly at a small
    override (`HEGEL_DEFAULT_PROFILE=soak HEGEL_TEST_CASES=80`), and the base
    branch's measurements are unchanged.

Not done, deliberately: nothing is pushed; the pre-existing integration suite is
untouched apart from the two behaviour fixes; `hegel.toml` counts are unchanged;
no new runtime dependencies (the SHA-1 oracle and the marker facility are
hand-written); no refactor beyond what the markers needed (one `identify()`
helper, one `apply_patch()` helper, one `require_supported()` wrapper in
`diff.rs`).

---

## Appendix A — reduced reproductions of the local `patch` 2.8 defects

Both involve no tool code: a hand-written file and a hand-written unified diff.

**(A1) `-F 0` reports phantom fuzz for an exact match.**

```sh
$ printf 'l000\nl001\nl002\n...\nl011\n' > f.txt          # 12 one-per-line entries
$ printf -- '--- a/f.txt\n+++ b/f.txt\n@@ -3,5 +3,5 @@\n l002\n l003\n-l004\n+XXX\n l005\n l006\n' > p.diff
$ patch -p1 -F 0 --batch --silent -i p.diff ; echo $?
1 out of 1 hunk FAILED
$ patch -p1 --batch --silent --dry-run --verbose -i p.diff | tail -1
Hunk #1 succeeded at 3 with fuzz 1.
$ git apply --check p.diff ; echo $?
0
```

A hunk that has both leading and trailing context always needs "fuzz 1" from this
build, so `-F 0` can never be used to demand exact context. Hunks with no leading
context (a change on the first line) are accepted at `-F 0`, which is why the
pre-existing round-trip property (whose cases change the first and last lines)
never noticed.

**(A2) valid diffs with ≥6 leading context lines are rejected.**

```sh
# f: 12 lines; body: 6 context lines, then two additions, then 3 context lines
$ printf -- '--- a/f\n+++ b/f\n@@ -1,9 +1,11 @@\n l000\n l001\n l002\n l003\n l004\n l005\n+A1\n+A2\n l006\n l007\n l008\n' > p.diff
$ patch -p1 --batch --silent -i p.diff ; echo $?
1 out of 1 hunk FAILED
$ git apply --check p.diff ; echo $?
0
```

Five leading context lines are accepted, six are not. Slicing a hunk (`id:a-b`)
turns unselected deletions into context, so a sliced payload routinely has six or
more leading context lines — which is why `split <id>:range` fails on this box
while the payload is valid.

---

## Appendix B — the marker facility in full (`src/markers.rs`)

```rust
//! Reachability ("coverage") markers for `jj-hunk-tool`.
//!
//! A *marker* is a named point on a code path that must be reached for the
//! tool to have actually done the thing a test claims it did. Markers are the
//! same idea as the Antithesis SDK's `hit!` assertions or Precept's coverage
//! markers: the test does not merely observe that a command exited 0, it also
//! asserts that the executed code *travelled the path under test*. A suite that
//! silently stops generating rename-shaped diffs, or a branch that becomes
//! unreachable, then fails instead of passing vacuously.
//!
//! The facility is deliberately small:
//!
//! * [`Marker`] is a closed set of named paths, generated by [`marker_set!`].
//! * [`hit`] (or the [`hit!`](crate::hit!) macro) marks one as reached. It is a
//!   single relaxed atomic store — a few cycles, no allocation, no lock — and it
//!   is compiled unconditionally, so release builds mark too.
//! * [`REQUIRED`] is the contract: the set of markers the test suite promises to
//!   exercise. [`assert_required_hit`] fails loudly, naming every missing
//!   marker, when a run does not reach the whole set.
//! * [`emit_if_requested`] prints the hit set to stderr on exit when
//!   `JJ_HUNK_TOOL_MARKERS` is set, which is how the integration tests observe
//!   the markers of a subprocess. With the variable unset the cost is one
//!   `getenv` at exit.
//!
//! In-process tests use [`reset`] + [`hit`] + [`is_hit`] directly; see
//! `tests/coverage_contract.rs` for the run that asserts the contract against
//! real `jj` output, and the "Coverage markers" section of the README for the
//! documented set.

use std::sync::atomic::{AtomicBool, Ordering};

use git_surgeon::diff::DiffHunk;

/// Mark `$marker` as reached, i.e. `markers::hit(Marker::$marker)`.
///
/// Exported at the crate root so library code can write `crate::hit!(…)` and
/// the binary `jj_hunk_tool::hit!(…)`.
#[macro_export]
macro_rules! hit {
    ($marker:ident) => {{
        $crate::markers::hit($crate::markers::Marker::$marker);
    }};
}

/// Set this in the environment to make the binary print its hit markers to
/// stderr when it exits. Any non-empty value enables it.
pub const MARKERS_ENV_VAR: &str = "JJ_HUNK_TOOL_MARKERS";

/// Prefix of every emitted marker line.
pub const LINE_PREFIX: &str = "coverage-marker:";

macro_rules! marker_set {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// A named point on a code path that the test suite can require.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Marker {
            $($variant),*
        }

        impl Marker {
            /// Every marker, in declaration order. Emission order follows this,
            /// so an emitted marker list is byte-stable across runs.
            pub const ALL: &'static [Marker] = &[$(Marker::$variant),*];

            /// The stable name used in emitted output, test assertions and the
            /// README's marker table.
            pub fn name(self) -> &'static str {
                match self {
                    $(Marker::$variant => $name),*
                }
            }
        }

        /// One flag per marker, indexed by the enum's discriminant.
        static HITS: [AtomicBool; marker_set!(@count $($variant),*)] =
            [const { AtomicBool::new(false) }; marker_set!(@count $($variant),*)];
    };
    (@count $($variant:ident),*) => {
        <[()]>::len(&[$(marker_set!(@unit $variant)),*])
    };
    (@unit $variant:ident) => { () };
}

marker_set! {
    // ── Diff decoding (`diff::decode_diff`) ──────────────────────────────────
    DiffSectionDecoded => "diff/section_decoded",
    DiffSectionUndecodableSkipped => "diff/section_undecodable_skipped",
    DiffSectionsMultiple => "diff/sections_multiple",

    // ── Hunk identification (`markers::observe_hunks`) ───────────────────────
    ParseNoHunks => "parse/no_hunks",
    ParseHunkNewFile => "parse/hunk_new_file",
    ParseHunkDeletedFile => "parse/hunk_deleted_file",
    ParseHunkPathChange => "parse/hunk_path_change",
    ParseHunkAdditionsOnly => "parse/hunk_additions_only",
    ParseHunkDeletionsOnly => "parse/hunk_deletions_only",
    ParseHunkModified => "parse/hunk_modified",
    ParseHunkNoNewlineMarker => "parse/hunk_no_newline_marker",
    ParseDuplicateIdSuffix => "parse/duplicate_id_suffix",
    ParseUnsupportedRename => "parse/unsupported_rename",
    ParseUnsupportedCopy => "parse/unsupported_copy",
    ParseUnsupportedModeChange => "parse/unsupported_mode_change",
    ParseUnsupportedSimilarity => "parse/unsupported_similarity",

    // ── Selection semantics (`markers::observe_selection`) ───────────────────
    SelectNoHunks => "select/no_hunks",
    SelectAllHunks => "select/all_hunks",
    SelectSingleHunk => "select/single_hunk",
    SelectSubset => "select/subset",
    SelectInterleaved => "select/interleaved",
    SelectFirstHunk => "select/first_hunk",
    SelectLastHunk => "select/last_hunk",
    SelectLineRanges => "select/line_ranges",

    // ── Patch payload construction (`tool::build_combined_patch`) ────────────
    PatchWholeHunk => "patch/whole_hunk",
    PatchReverse => "patch/reverse",
    PatchSupportedHunk => "patch/supported_hunk",

    // ── Apply modes ──────────────────────────────────────────────────────────
    ApplyPatchOnly => "apply/patch_only",
    ApplySplit => "apply/split",
    ApplySplitParallel => "apply/split_parallel",
    ApplySquash => "apply/squash",
    ApplyDiffedit => "apply/diffedit",
    ApplyRestore => "apply/restore",
    ApplyAbsorbDryRun => "apply/absorb_dry_run",
    ApplyAbsorbExecute => "apply/absorb_execute",
    ApplyJjTool => "apply/jj_tool",
    ApplyJjToolReverse => "apply/jj_tool_reverse",

    // ── Absorb routing (`tool::absorb_hunks`) ────────────────────────────────
    AbsorbNewFile => "absorb/new_file",
    AbsorbMatchedChange => "absorb/matched_change",
    AbsorbMatchedFile => "absorb/matched_file",
    AbsorbAmbiguous => "absorb/ambiguous",
    AbsorbNoOverlap => "absorb/no_overlap",

    // ── Offset / context edge cases ──────────────────────────────────────────
    //
    // Both are defensive fallbacks for a malformed `@@` header, which `jj` never
    // emits: `parse_header_ranges` falls back to (1, 1) when the header does not
    // start with `@@ -`, and `absorb` treats a header it cannot parse as "no old
    // range". They are placed so a future caller that *can* feed them a
    // malformed header (a hand-edited patch, a different diff backend) reports
    // it, but they are not required: no deterministic test can reach them
    // without synthesising a header that no command would ever consume.
    OffsetHeaderRangesFallback => "offset/header_ranges_fallback",
    OffsetOldRangeUnparsed => "offset/old_range_unparsed",

    // ── Error paths ─────────────────────────────────────────────────────────
    ErrorHunkNotFound => "error/hunk_not_found",
    ErrorUnsupportedMetadata => "error/unsupported_metadata",
    ErrorNoHunksSelected => "error/no_hunks_selected",
    ErrorInvalidRangeStart => "error/invalid_range_start",
    ErrorInvalidRangeEnd => "error/invalid_range_end",
    ErrorInvalidLineNumber => "error/invalid_line_number",
    ErrorRangeBounds => "error/range_bounds",
    ErrorJjCommandFailed => "error/jj_command_failed",
    ErrorPatchApplyFailed => "error/patch_apply_failed",
    ErrorMissingPatchEnv => "error/missing_patch_env",
}

/// The marker contract: every marker in this list must be reached by the test
/// suite. `tests/coverage_contract.rs` asserts it, so removing or losing a
/// path fails the suite instead of quietly shrinking coverage.
///
/// Adding a marker to a code path without adding it here is allowed (markers
/// may be placed for later work, for shapes `jj` itself cannot produce, or for
/// defensive fallbacks nothing can reach), but adding it here is a promise that
/// a deterministic test reaches it.
pub const REQUIRED: &[Marker] = &[
    // Diff decoding.
    Marker::DiffSectionDecoded,
    Marker::DiffSectionUndecodableSkipped,
    Marker::DiffSectionsMultiple,
    // Identification: one marker per hunk/file shape.
    Marker::ParseNoHunks,
    Marker::ParseHunkNewFile,
    Marker::ParseHunkDeletedFile,
    Marker::ParseHunkPathChange,
    Marker::ParseHunkAdditionsOnly,
    Marker::ParseHunkDeletionsOnly,
    Marker::ParseHunkModified,
    Marker::ParseHunkNoNewlineMarker,
    Marker::ParseDuplicateIdSuffix,
    Marker::ParseUnsupportedRename,
    Marker::ParseUnsupportedCopy,
    Marker::ParseUnsupportedModeChange,
    Marker::ParseUnsupportedSimilarity,
    // Selection branches.
    Marker::SelectNoHunks,
    Marker::SelectAllHunks,
    Marker::SelectSingleHunk,
    Marker::SelectSubset,
    Marker::SelectInterleaved,
    Marker::SelectFirstHunk,
    Marker::SelectLastHunk,
    Marker::SelectLineRanges,
    // Payload construction.
    Marker::PatchWholeHunk,
    Marker::PatchReverse,
    Marker::PatchSupportedHunk,
    // Apply modes.
    Marker::ApplyPatchOnly,
    Marker::ApplySplit,
    Marker::ApplySplitParallel,
    Marker::ApplySquash,
    Marker::ApplyDiffedit,
    Marker::ApplyRestore,
    Marker::ApplyAbsorbDryRun,
    Marker::ApplyAbsorbExecute,
    Marker::ApplyJjTool,
    Marker::ApplyJjToolReverse,
    // Absorb routing.
    Marker::AbsorbNewFile,
    Marker::AbsorbMatchedChange,
    Marker::AbsorbMatchedFile,
    Marker::AbsorbAmbiguous,
    Marker::AbsorbNoOverlap,
    // Offset / context edges are asserted by the properties that check hunk
    // spans and header arithmetic (tests/properties_shapes.rs,
    // tests/properties_apply.rs); the two `offset/*` markers are defensive
    // fallbacks for malformed headers that `jj` never emits, so they are
    // deliberately not part of the contract.
    // Error paths.
    Marker::ErrorHunkNotFound,
    Marker::ErrorUnsupportedMetadata,
    Marker::ErrorNoHunksSelected,
    Marker::ErrorInvalidRangeStart,
    Marker::ErrorInvalidRangeEnd,
    Marker::ErrorInvalidLineNumber,
    Marker::ErrorRangeBounds,
    Marker::ErrorJjCommandFailed,
    Marker::ErrorPatchApplyFailed,
    Marker::ErrorMissingPatchEnv,
];

/// Mark `marker` as reached. A relaxed store: no allocation, no lock, no
/// ordering requirements (the set is only read at exit or by tests).
#[inline]
pub fn hit(marker: Marker) {
    HITS[marker as usize].store(true, Ordering::Relaxed);
}

/// Whether `marker` has been reached in this process.
#[inline]
pub fn is_hit(marker: Marker) -> bool {
    HITS[marker as usize].load(Ordering::Relaxed)
}

/// Clear every marker. Only tests need this; the binary never resets.
pub fn reset() {
    for flag in &HITS {
        flag.store(false, Ordering::Relaxed);
    }
}

/// Every marker reached so far, in [`Marker::ALL`] order.
pub fn hit_markers() -> Vec<Marker> {
    Marker::ALL.iter().copied().filter(|m| is_hit(*m)).collect()
}

/// Names of every marker reached so far, in [`Marker::ALL`] order.
pub fn hit_names() -> Vec<&'static str> {
    hit_markers().into_iter().map(Marker::name).collect()
}

/// Render the hit set as `coverage-marker: <name>` lines.
///
/// Deterministic: [`Marker::ALL`] order, one line per hit marker.
pub fn render() -> String {
    let mut out = String::new();
    for name in hit_names() {
        out.push_str(LINE_PREFIX);
        out.push(' ');
        out.push_str(name);
        out.push('\n');
    }
    out
}

/// Markers from `required` that have not been reached, by name.
pub fn missing(required: &[Marker]) -> Vec<&'static str> {
    required
        .iter()
        .filter(|m| !is_hit(**m))
        .map(|m| m.name())
        .collect()
}

/// Panic, naming every missing marker, unless `required` is a subset of the
/// hit set. `what` describes the run that was supposed to reach them.
#[track_caller]
pub fn assert_required_hit(required: &[Marker], what: &str) {
    let missing = missing(required);
    assert!(
        missing.is_empty(),
        "{what} did not reach {} required coverage marker(s):\n  {}\n\
         Reached markers: {}\n\
         A marker that is never reached means the behaviour it guards is no longer \
         exercised (a lost generator shape, an unreachable branch, a deleted call site), \
         not that the assertion is stale.",
        missing.len(),
        missing.join("\n  "),
        if hit_names().is_empty() {
            "(none)".to_string()
        } else {
            hit_names().join(", ")
        }
    );
}

/// Parse `JJ_HUNK_TOOL_MARKERS`-style output back into marker names.
///
/// Ignores every line that is not a marker line, so it can be applied to a
/// command's whole stderr.
pub fn parse_emitted(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| line.strip_prefix(LINE_PREFIX))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect()
}

/// Record the file and hunk shapes present in an identified diff.
///
/// Every command converts `jj diff` output into identified hunks through this
/// one function, so a marker here fires if and only if the tool really received
/// and identified a hunk of that shape. That is what makes "the generator
/// stopped producing rename-shaped diffs" a test failure rather than a silent
/// loss of coverage: the shape markers are the observation point that all the
/// per-shape code paths sit behind.
pub fn observe_hunks(identified: &[(String, &DiffHunk)]) {
    if identified.is_empty() {
        crate::hit!(ParseNoHunks);
        return;
    }
    for (id, hunk) in identified {
        if id.contains('-') {
            // `assign_ids` only suffixes when two hunks hash to the same
            // prefix, so this marks the collision path, not the ordinary ids.
            crate::hit!(ParseDuplicateIdSuffix);
        }
        if crate::diff::is_absent_side(&hunk.old_file) {
            crate::hit!(ParseHunkNewFile);
        } else if crate::diff::is_absent_side(&hunk.new_file) {
            crate::hit!(ParseHunkDeletedFile);
        } else if hunk.old_file != hunk.new_file {
            crate::hit!(ParseHunkPathChange);
        }

        let adds = hunk.lines.iter().any(|line| line.starts_with('+'));
        let deletes = hunk.lines.iter().any(|line| line.starts_with('-'));
        match (adds, deletes) {
            (true, false) => crate::hit!(ParseHunkAdditionsOnly),
            (false, true) => crate::hit!(ParseHunkDeletionsOnly),
            (true, true) => crate::hit!(ParseHunkModified),
            // A hunk of pure context cannot come from `jj diff`, but a marker
            // without a shape is worse than no marker, so it is not required.
            (false, false) => {}
        }

        if hunk.lines.iter().any(|line| line.starts_with('\\')) {
            crate::hit!(ParseHunkNoNewlineMarker);
        }

        match hunk.unsupported_metadata.as_deref() {
            Some("rename from" | "rename to") => crate::hit!(ParseUnsupportedRename),
            Some("copy from" | "copy to") => crate::hit!(ParseUnsupportedCopy),
            Some("old mode" | "new mode") => crate::hit!(ParseUnsupportedModeChange),
            Some("similarity index" | "dissimilarity index") => {
                crate::hit!(ParseUnsupportedSimilarity);
            }
            _ => {}
        }
    }
}

/// Record the shape of a hunk selection: which hunks of `all` were picked, and
/// whether inline line ranges narrowed any of them.
///
/// Called once per command, from the point where the caller's hunk-id specs
/// have been resolved against the identified diff, so the markers describe the
/// selection the tool actually built its payload from.
pub fn observe_selection(selected: &[&str], all: &[&str], line_ranges: bool) {
    if selected.is_empty() {
        crate::hit!(SelectNoHunks);
        return;
    }
    if line_ranges {
        crate::hit!(SelectLineRanges);
    }
    if selected.len() == 1 {
        crate::hit!(SelectSingleHunk);
    }
    if selected.len() == all.len() {
        crate::hit!(SelectAllHunks);
    } else {
        crate::hit!(SelectSubset);
    }

    let indices: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, id)| selected.contains(id))
        .map(|(index, _)| index)
        .collect();
    if indices.first() == Some(&0) {
        crate::hit!(SelectFirstHunk);
    }
    if !all.is_empty() && indices.last() == Some(&(all.len() - 1)) {
        crate::hit!(SelectLastHunk);
    }
    if indices.windows(2).any(|pair| pair[1] != pair[0] + 1) {
        crate::hit!(SelectInterleaved);
    }
}

/// Print the hit set to stderr when [`MARKERS_ENV_VAR`] is set to a non-empty
/// value. Called once, at exit, by the binary.
pub fn emit_if_requested() {
    if std::env::var_os(MARKERS_ENV_VAR).is_some_and(|value| !value.is_empty()) {
        eprint!("{}", render());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Unit tests in one binary share this process's marker state, so tests
    /// that assert on the set take this lock and reset before asserting.
    static LOCK: Mutex<()> = Mutex::new(());

    fn locked() -> std::sync::MutexGuard<'static, ()> {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        guard
    }

    #[test]
    fn marker_names_are_unique_and_namespaced() {
        let mut names: Vec<&str> = Marker::ALL.iter().map(|m| m.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate marker name in the set");
        for name in names {
            assert!(
                name.contains('/'),
                "marker {name:?} is not namespaced like `<area>/<path>`"
            );
        }
    }

    #[test]
    fn required_markers_are_all_declared_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for marker in REQUIRED {
            assert!(
                seen.insert(marker.name()),
                "{} is listed twice in REQUIRED",
                marker.name()
            );
            assert!(
                Marker::ALL.contains(marker),
                "{} is required but not declared",
                marker.name()
            );
        }
    }

    #[test]
    fn hit_and_reset_round_trip() {
        let _guard = locked();
        assert!(!is_hit(Marker::ApplySplit));
        hit(Marker::ApplySplit);
        assert!(is_hit(Marker::ApplySplit));
        assert!(!is_hit(Marker::ApplySquash));
        assert_eq!(hit_names(), vec!["apply/split"]);
        reset();
        assert!(!is_hit(Marker::ApplySplit));
        assert!(hit_names().is_empty());
    }

    #[test]
    fn render_is_ordered_and_parseable() {
        let _guard = locked();
        hit(Marker::ApplySquash);
        hit(Marker::ApplySplit);
        let rendered = render();
        let parsed = parse_emitted(&rendered);
        // Declaration order, not call order.
        assert_eq!(parsed, vec!["apply/split", "apply/squash"]);
        // Non-marker lines are ignored, so whole stderr can be scanned.
        assert_eq!(
            parse_emitted("warning: skipping non-UTF-8 file\ncoverage-marker: apply/split\n"),
            vec!["apply/split"]
        );
    }

    #[test]
    #[should_panic(expected = "did not reach 1 required coverage marker")]
    fn assert_required_hit_names_the_missing_marker() {
        let _guard = locked();
        hit(Marker::ApplySplit);
        assert_required_hit(&[Marker::ApplySplit, Marker::ApplySquash], "a test run");
    }

    #[test]
    fn assert_required_hit_passes_on_a_subset() {
        let _guard = locked();
        hit(Marker::ApplySplit);
        assert_required_hit(&[Marker::ApplySplit], "a test run");
    }
}
```
