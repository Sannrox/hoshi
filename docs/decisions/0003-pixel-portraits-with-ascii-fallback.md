# ADR 0003: Pixel portraits with ASCII fallback

- Status: Accepted
- Date: 2026-10-12
- Supersedes: —

## Context

Issue #27 asks for faces and places in the TUI: a commander, fleet admirals,
pirate captains, and planet views. Terminals differ in whether they can draw
inline images. The layout must stay usable at 80x24 with no stray control
sequences on a plain terminal. Art has to ship in-tree under Apache-2.0 with a
size budget so the binary stays small.

## Decision

- **Style.** 8×8 pixel art, white and gray on true black (`#000`). No painted
  plates, no extra chrome. Each slot also has a 4×8 ASCII drawing used for
  tests and when images are off.
- **One RGB grid per portrait.** Half-block cells (`▀`) reuse that grid. An
  inline graphics protocol, when the terminal advertises it, sends the same
  raw RGB after the frame is drawn.
- **Detection.** `portrait_protocol = auto|off|graphics` in user config, plus
  `HOSHI_PORTRAIT_PROTOCOL` and `--no-portraits`. Auto looks at `TERM` /
  `TERM_PROGRAM` for terminals that speak the protocol. Tests force protocol
  off so TestBackend never sees control sequences.
- **Moods.** Config maps event-log substrings to pleased / alarmed / neutral.
  A missing mood file uses the neutral drawing for that slot.
- **Assets.** Original work in `crates/hoshi-tui/assets/portraits/`, listed in
  `crates/hoshi-tui/ASSETS.md`. CI enforces a per-file and pack size cap.

## Consequences

- No extra image crate. Core still owns rules; the TUI only renders.
- Live protocol drawing is a thin stdout write after ratatui; snapshot tests
  cover ASCII and the absence of escapes.
- Admirals reuse the fleet-screen slot until a later Issue gives them names.

## Rejected alternatives

- A dedicated image-protocol crate: heavier lockfile for one 8×8 blit.
- Painted or high-resolution plates: they blow the size budget and fight the
  dense terminal layout.
- Blocking the UI on a portrait download or generator: assets are bundled.
