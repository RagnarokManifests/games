# Ragnarok Launcher

Tauri app (Rust + React/TypeScript) — game launcher with Steam Tools integration.

## Build commands
- `cd src-tauri && cargo check` — Rust typecheck
- `npx tsc --noEmit` — TypeScript typecheck
- `npm run tauri build` — full build

## Key architecture
- **Backend**: Rust in `src-tauri/src/` — main logic in `main.rs`, managers in `src-tauri/src/managers/`
- **Frontend**: `src/App.tsx` — single large file with all UI + Tauri invoke calls
- **IPC**: Tauri commands registered in `main.rs`, invoked via `@tauri-apps/api` `invoke()` from `App.tsx`

## Recent changes
- **Catalog fallback chain** (`src-tauri/src/managers/game.rs`): Ryuu API → GitHub raw catalog → SteamTools static list → FALLBACK_GAME_IDS (hardcoded). Cache v5, min 500 games.
- **Plugin download** (`main.rs` ~line 695): 4 GitHub mirror URLs × 3 retries each (30s→60s→120s timeout) to bypass Windows firewall blocks (os error 10013).
- **Local save backup** (`cloud_saves.rs` + `main.rs`): Backs up saves to `%APPDATA%/ragnarok/local_save_backups/{appId}/backup_{timestamp}/`, max 10 per game. Settings toggle in `LocalBackupSettingsCard` component. Auto-sync on game-close and launcher focus.
- **CloudRedirect** (`main.rs` ~line 1777): Downloads DLL from `Selectively11/CloudRedirect` GitHub releases. Works but has a hardcoded config path.
- **Bypass Mediafire folders** (`main.rs` ~line 8470): Supports multiple Mediafire folders (`1ukdsqzvsdokn` and `vhonhj3luhxo3`) with automatic pagination (chunks), deduplication, and cache.
