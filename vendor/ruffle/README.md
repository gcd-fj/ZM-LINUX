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
