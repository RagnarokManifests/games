---
name: ragnarok-ui-design
description: Design system reference for Ragnarok Launcher's frontend (src/App.tsx + src/components/). Use this whenever styling, restyling, or building any UI in this specific repo — new views, modals, cards, buttons, badges, or anything touched with Tailwind classes or framer-motion. Keeps every screen visually consistent with the "Futurist Minimal" system already established across Home, the sidebar, catalog, achievements, settings, and game detail. Trigger on requests like "make this look nicer", "restyle this view", "add a new card/modal", "this looks transparent/washed out", or "match the rest of the app".
---

# Ragnarok Launcher UI Design System — "Futurist Minimal"

This is a Tauri desktop app (Rust backend + one large React/TypeScript frontend
file, `src/App.tsx`, plus a few standalone components in `src/components/`).
The whole UI floats over a full-bleed background image/video
(`DynamicBackground`), so **every card needs enough opacity to read clearly
against whatever's behind it** — this is the single most common mistake to
avoid (see "Solid, not glass" below).

## Why this exists

This app went through a period of heavy parallel-agent work restyling
different views independently. Without a shared reference, each pass tends to
invent its own color story, its own card treatment, its own animation timing
— and the app ends up looking like a patchwork instead of one product. This
skill is that shared reference. Read it before touching any styling in this
repo, and match what's here rather than improvising a new look for just the
screen you're on.

## Solid, not glass

Cards use a **solid, nearly-opaque dark background**, not a translucent glass
effect. An earlier pass used `bg-white/[0.02] backdrop-blur-xl`, which looked
washed-out and transparent over the background art — the user explicitly
disliked this and asked for something more solid and minimalist. Use:

```
bg-[#0d0e12] border border-white/[0.08]
```

Reserve actual blur/translucency for true overlays — full-screen modal
backdrops (`bg-black/60 backdrop-blur-md`) — not for cards sitting in the
normal page flow.

## Color

- **Base app background**: near-black, neutral, no color tint (`#050507`–`#0a0a0f` range).
- **Cards/surfaces**: `bg-[#0d0e12]` with `border-white/[0.08]` (see above — solid, not glass).
- **Accent**: this app has a single user-customizable accent color wired through a CSS variable, `var(--accent-color-hex)`, exposed via the `text-accent` / `bg-accent` / `border-accent` Tailwind classes. Use it sparingly — primary buttons, active nav/tab state, one ambient glow, progress bars/percentages. Don't reach for a different color per component (an earlier pass had violet here, cyan there, coral elsewhere — collapse all of that down to this one accent + neutral grays).
- **Text**: `text-white/90` for primary content, `text-gray-500`/`text-gray-600` for secondary/tertiary.
- **Semantic color stays semantic**: Denuvo/DRM warnings, pass/fail spec-match indicators, and similar meaningful status colors (red/amber/green) should stay clearly colored — don't neutralize those, they're conveying real information, not decoration.
- One exception: color-picker controls (like the accent-color swatches in Settings) must show each option in its true color — that's the one place color choice IS the content.

## Shape & spacing

- Generous whitespace — let content breathe, don't cram.
- `rounded-2xl` for cards/panels/modals.
- `rounded-xl` for buttons, inputs, list rows.
- `rounded-full` for pills, badges, avatars, icon-circles.
- Borders are always hairline: `border-white/[0.06]` to `border-white/[0.08]`, never thick or brightly colored (except semantic warnings, see above).
- At most **one** soft ambient background glow per screen region. Scattered per-card decorative blur blobs read as noisy, not futuristic.

## Typography

- Headings: `font-black`, tight tracking, not oversized/shouty.
- Section labels, stat labels, and badges: `uppercase tracking-widest text-[9px]` to `text-[10px] font-black`. This is the app's signature "futuristic" text treatment — keep using it for anything label-like.
- Body/description text: `font-medium` or `font-semibold`, `text-sm`/`text-[13px]`, `text-gray-400`.

## Motion (framer-motion — already a dependency; `motion`/`AnimatePresence` already imported in App.tsx)

- **Entrance**: fade + 8–12px y-translate, `duration: 0.3–0.4`, `ease: 'easeOut'`. Apply this to every card/section as it mounts.
- **Stagger**: when a list or grid of similar items mounts (stat boxes, tool buttons, game cards, achievement rows), wrap them in a parent with a `staggerChildren` variant (0.05–0.08s between children) rather than animating them all at once.
- **Hover**: subtle only — `scale: 1.01–1.03`, or a border/background brightness shift, or a 1–2px translate. Never a large jump.
- **Active-state transitions** (selected nav item, selected tab): prefer a shared-element `layoutId` transition over an instant snap, so the highlight visibly slides/morphs between states.
- **Modals**: backdrop fades in (`opacity 0→1`), the modal panel itself scales in slightly (`scale: 0.96–0.98 → 1`) plus the same fade+y-translate as everything else.
- **Loading states**: prefer a gentle opacity pulse (framer-motion, looping) over a spinning icon where it fits the content (e.g. a status dot, a progress bar) — spinners are fine for literal "in progress" icons (refresh, download).

## Shared component tokens

Reuse these rather than inventing new variants per screen:

| Element | Classes |
|---|---|
| Card | `rounded-2xl bg-[#0d0e12] border border-white/[0.08]` |
| Primary button | accent-filled, `rounded-xl`, `hover:brightness-110 hover:-translate-y-0.5` |
| Secondary button | `bg-white/[0.06] hover:bg-white/[0.1] border border-white/10 rounded-xl` |
| Badge / pill | `rounded-full bg-white/[0.05] border border-white/10 text-[9px] uppercase tracking-widest` |
| Input | `bg-black/40 border border-white/10 rounded-xl focus:border-accent/50 focus:ring-1 focus:ring-accent/25` |
| Icon circle | `rounded-full bg-white/[0.05] border border-white/10` (or `bg-white/[0.04]` for smaller inline icons) |

## i18n — don't skip this

Every screen in this app is bilingual (Spanish/English) via `useLanguage()`
(returns `{ lang, t }`, `t` being the structured `TRANSLATIONS` object) and
`useTranslateInline()` (returns a `ti(esText, enText)` helper for one-off
strings not worth adding to the structured translations). Both are exported
from `src/App.tsx`. When adding or restyling UI text, use one of these —
never hardcode a single-language string, even for something that feels like
a throwaway label.

## Before you touch a component

1. Skim 1–2 already-restyled views for a live reference (Home, the sidebar, or Achievements are good examples as of this writing) rather than starting from a blank slate.
2. Preserve every prop, `invoke()` call, state variable, and business-logic branch exactly as-is — this is a visual/motion pass, not a refactor. If a parallel agent or a past session already broke this rule, don't compound it.
3. After edits, run `npx tsc --noEmit -p tsconfig.json` from the project root and confirm it exits clean before considering the work done.
