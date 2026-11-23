# CLAUDE.md

Project guidance for Claude Code. Shared agent rules live in `.github/` so Claude and GitHub Copilot read the
same source; they are imported here.

@.github/copilot-instructions.md
@.github/instructions/graphify.instructions.md

Setup and maintenance of the knowledge graph is described in [GRAPHIFY.md](GRAPHIFY.md). The project-scoped
MCP server `graphify` (serving `graphify-out/graph.json`) is declared in `.mcp.json`; it needs `graphify-mcp` on PATH
(`pipx install "graphifyy[mcp]"`).

## Rules for agents working on Ravenglass

- **Read first:** `START_HERE.md`, `docs/PLAN.md`, and Rune's `docs/reference/rune-AGENTS.md` (its rules apply here too).
- **All Rust. Cross-platform.** No C/C++/Objective-C engines or system webviews. Platform code lives only in `app/src/platform/{macos,windows,linux}.rs`, and each feature there needs a portable default.
- **Rune is pinned** to `dev` @ `0ca075619d116f8f6e0e3bc8ff815c72c636cac8`. Bump every git line in `Cargo.toml` together, and re-copy Rune's `[patch.crates-io]` table when you do.
- **Check your work headlessly** (`--snap`, `--tree`, `--script`; see `docs/reference/rune-core-README.md`, "Agent tooling") and compare it against `brand/preview/`.
- **No hard-coded colours** in views. Ask `Theme::role("…")`; add new roles to both built-in themes.
- **The grid:** sizes are whole cells, monospace text, IBM Plex Mono by default.
- **State machines** for anything with modes, phases or timers (`mail-core::machine` style: an enum state plus a `step` that returns effects).
- **Off the UI thread:** network, disk, parsing, Blitz layout and raster. Start them from `on_ready`.
- Keep files under 800 lines; tests go in sibling `tests.rs` files.
- **Never log** message bodies, addresses or tokens.
- **The reference IDE (unstablebuild/rune) is GPL-3.** Match its look; never copy its code or assets.
- **When unsure about scope or a design decision,** check `docs/PLAN.md` §8 and ask Jason rather than guessing on anything hard to reverse.
