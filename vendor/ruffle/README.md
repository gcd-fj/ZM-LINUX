# Pinned Ruffle core compatibility fix

Source: https://github.com/ruffle-rs/ruffle/tree/a4f5b5256e245693bc9077ef6c6b6abc95490e7f

Only `core` is vendored. Its sibling crates still use that exact Git revision. The root manifest retains upstream workspace dependency/lint settings, with membership reduced to core; core path dependencies point to the pinned Git source. The application workspace applies a Cargo patch so frontend-utils uses the same core instance. License: see LICENSE.md.

Local runtime change: `src/avm2/globals/json.rs` preserves JSON numbers as f64 and only promotes losslessly representable i32 values. Upstream narrowed all integral doubles to wrapping i32; e.g. 1788615600000 became 1909204864 (January 1970). This corrupts server dates and daily reward lookup keys.

Regression: `cargo test -p zm-player --test json_numbers` runs a small SWF through AVM2 JSON.parse, including nested timestamps, signed/unsigned 32-bit boundaries, safe large integers, fractional numbers, and UTC weekday checks. Fixture source is under crates/zm-player/tests/fixtures.

Remove the patch and vendor copy together only after a reviewed upstream revision passes the same regression.

Additional compatibility fixes:
- `date.rs`: accepts one- or two-digit month/day fields and clock fields, including HH:MM. Official Activity.xml uses dates such as 2026/8/27-00:00:01; the game replaces the hyphen with a space before Date parsing. Two-digit-only parsing returns NaN and incorrectly hides activities. The AVM2 fixture covers game-style dates.
- `display_object.rs`: bitmap caches include the rasterization origin relative to the object's transform in their validity key. Bounds can move without changing texture size, particularly with hidden animated children; reusing the old pixels at a new origin causes jitter. Whole-object translation still reuses the cache. Regression: `cargo test -p ruffle_core --lib bitmap_cache_regression`.

Real-account activity visibility and chest rendering need verification after restarting the rebuilt application.

Timeline overlay fix: `movie_clip.rs` preserves surviving sibling anchors when a rewind replaces timeline graphics. Previously a re-created button background could be appended above a script-added label. `timeline_rewind_preserves_script_overlay_order` reproduces forward/rewind hover-state transitions with a synthetic SWF (source and generator in the repo); it fails at frame 1 before the fix. Diagnostic patch marker: timeline-overlay-v1.

Visible render bounds follow-up: the origin cache key alone did not eliminate chest jitter in real use. Rendering bounds now skip invisible children (including SimpleButton state content), so hidden animation cannot change filter rasterization dimensions or subpixel origins. Mask drawing explicitly preserves invisible descendants. ActionScript-visible getBounds semantics remain untouched. The regression moves a hidden child through fractional-pixel positions and asserts stable render bounds, then verifies mask mode and visible content still include it. Diagnostic marker: visible-render-bounds-v1.

Timeline overlay v2: only move a recreated background when it is above its surviving anchor. Depth insertion may already put it below an authored caption; reinserting at the old anchor index reverses that correct order. The six-frame TimelineLabels fixture exercises both label pages and repeated hover/press/rewind transitions. User confirmed visible-render-bounds-v1 fixes activity chest jitter.

Array.sortOn primitive entries: AVMplus `ArraySort::toFieldObject` / `FieldCompare`
orders non-object entries together, after objects in ascending sorts and before
objects in descending sorts, without reading the requested property on primitives.
The previous implementation read `id` on String entries and threw #1069 while
the holiday welfare view initialized, leaving reward rows empty. This patch keeps
real object property/getter errors intact; it does not suppress arbitrary script
exceptions. The existing shared sort handling of undefined is unchanged.
Reference: https://github.com/adobe/avmplus/blob/master/core/ArrayClass.cpp#L856-L908
Regression: `cargo test -p zm-player --test sort_on --locked`; source and SWF are
in `crates/zm-player/tests/fixtures/ArraySortOn.*`. Rebuild with the same ASC inputs
as the other fixtures and `python3 tools/build-json-fixture.py ArraySortOn`.
Diagnostic marker: sort-on-primitives-v1. The synthetic fixture reproduces the
observed exception; the real-account reward page still requires verification.

Opt-in slow-phase tracing: when the `zm_perf` INFO target is enabled, player
phases taking at least 25ms log a static phase name and elapsed wall time. This
separates preload, AVM frames, timers/network callbacks, mouse handling and GC
without changing their ordering, budgets or outcomes. Phases may nest, so their
durations must not be summed. Disabled tracing does not read the clock. No script
values, credentials or account fields are logged. Marker: slow-phase-trace-v1.
