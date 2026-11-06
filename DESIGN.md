<!-- SEED: re-run /impeccable document once there's code to capture the actual tokens and components. -->
---
name: dissonant
description: A native macOS DAW that shows you which notes sound good — dark, mono-forward, raw by design.
---

# Design System: dissonant

## 1. Overview

**Creative North Star: "The 4-Track Terminal"**

A dark, monospace instrument with tape-grit soul. Four worlds fuse here: the tactile,
playful-but-serious precision of a Teenage Engineering OP-1; the dense, numbers-everywhere
legibility of music trackers (Renoise, FastTracker); the lo-fi analog warmth of cassette
4-track and zine culture; and the unembellished honesty of terminal and brutalist software.
The result is a tool that respects the user enough to show real musical structure plainly,
then makes it readable — never a toy, never a dashboard.

The surface is committed dark, the way a DAW used in a dim room should be, with a single
characterful brand color carrying identity and one purpose-built palette for the note-tier
system layered on top. Density is welcome; it's made legible through guidance and labeling,
not stripped out. Grit is a feature — subtle texture, honest monospace, tape-deck restraint —
not decoration bolted on. Every pixel should feel like it was placed by someone who loves
this music.

This system explicitly rejects three things, carried verbatim from the product's
anti-references: the **childish toy** (candy buttons, cartoon mascots, confetti), the
**generic SaaS dashboard** (cards-on-grey, corporate blue, Inter-everything), and the
**sterile/clinical** (cold all-white, over-minimal Apple-stock neutrality). Pro-tool *depth*
is not rejected — depth made legible is the whole point.

**Key Characteristics:**
- Committed dark surface; one characterful brand color, not corporate blue.
- Mono-forward typography — the tool reads like an instrument, not a web app.
- The three-tier note system is the signature element and must read by shape + label, not color alone.
- Responsive motion that conveys state (playhead, recoloring tiers), never choreography.
- Honest density: tracker-grade information, made legible by guidance.

## 2. Colors

Committed-dark strategy: a near-black architectural base, one saturated and slightly-off
brand color doing real identity work, and a dedicated, accessibility-first palette for the
three note-tiers. The mood lives in the brand color and the grit, not in a tinted-grey wash.

### Primary
- **Brand Signal** (`oklch` value `[to be resolved during implementation]`): the single
  characterful brand hue — leaning toward a saturated, lightly-aged tone with CRT/tape-deck-LED
  character (amber-phosphor or acid-tinged territory), deliberately *not* corporate blue. Carries
  logo, primary actions, current selection, the locked-key indicator. Chroma capped so text stays
  readable; white/near-white text on any filled instance.

### Neutral
- **Tape Black** (`[to resolve]`, OKLCH L ~0.10–0.14, near-zero chroma or a hair toward the brand hue):
  the main working surface — the dim room the music happens in.
- **Deck Panel** (`[to resolve]`, base pulled ~10–15% toward ink): a second neutral layer for
  toolbars, the chord-track lane, side panels — slightly distinct from the content surface.
- **Print Ink** (`[to resolve]`, ≥7:1 vs surface): primary text/note glyphs, carrying a trace of
  the brand hue at low chroma.
- **Faded Label** (`[to resolve]`, ≥3.5:1 vs surface): secondary text, inactive labels, gridlines.

### The Note-Tier Palette (signature; accessibility-first)
The three tiers are the product. They get distinct, color-blind-safe hues AND a non-color cue each:
- **Solid (chord tone):** the "rock-solid" tier — a confident, filled treatment. `[hue to resolve]`.
- **Tension (spicy-but-good):** the "use-with-intent" tier — a warmer/amber signal, distinct shape
  or partial fill. `[hue to resolve]`.
- **Dissonance (flagged):** the "going-wrong-on-purpose" tier — a hot warning hue PLUS a
  hatch/stripe/flag mark so it never relies on color. Labeled, never disabled. `[hue to resolve]`.

### Named Rules
**The Three-Tier Doctrine.** The note-tier system NEVER communicates by color alone. Every tier
carries a second channel — shape, fill pattern, border, or label — so red-green color blindness
(~8% of men) and small sizes never break the core mechanic. This rule is non-negotiable; it is the
product, not a polish item.

**The One-Signal Rule.** The brand color marks identity, the current selection, and the locked key —
nothing decorative. Its restraint is what makes the locked-key moment land.

## 3. Typography

**Display/UI Font:** Monospace `[specific family to be chosen at implementation]` — a characterful
mono with real personality (think the lineage of typewriter/terminal faces, not a sterile coding font).
**Body Font:** the same mono family carries the UI; prose-heavy help text may use a single quiet
companion sans `[to resolve]` if mono fatigues at length.
**Numeric/Data:** mono, always — note names, chord symbols, timecodes, tempo, beat positions.

**Character:** Mono-forward and proud of it. The fixed grid of a monospace face *is* the tracker
heritage and the terminal honesty in one move; it makes numbers and note-data line up perfectly and
gives the whole tool the feel of an instrument rather than a SaaS product.

### Hierarchy
- **Display** (mono, heavy weight, fixed rem — not fluid): the wordmark, big chord names, the
  key-lock moment. Used sparingly for character.
- **Headline** (mono, medium-bold): panel/section titles.
- **Title** (mono, medium): track names, dialog titles.
- **Body** (mono or companion sans, regular, ~14–15px, 65–75ch for prose): help text, descriptions.
- **Label/Data** (mono, smaller, slightly tightened): note names, chord symbols, numeric readouts,
  the "notes you could play now" list.

### Named Rules
**The Fixed-Scale Rule.** Type uses a fixed rem scale (ratio ~1.125–1.2), never fluid `clamp()`.
This is a tool viewed at consistent DPI; a heading that shrinks inside a side panel looks broken,
not responsive.

## 4. Elevation

Mostly flat, with tonal layering — depth comes from the near-black surface stepping to the slightly
lighter panel layer, not from drop shadows. Responsive motion energy means shadows, when they appear,
are a *response to state* (a dragged note lifting, a focused field), never ambient decoration. The
aesthetic is tape-deck and terminal: physical-feeling but flat, with grit and contrast doing the work
shadows would do elsewhere.

### Named Rules
**The Flat-Deck Rule.** Surfaces are flat at rest. Any shadow or lift is a state response (hover,
drag, focus) and disappears when the state ends.

## 5. Components

Component specs land on the first scan-mode rerun (once SwiftUI exists). One signature pattern is
seeded now because it's the core mechanic:

### The Note Tier (signature, pre-implementation doctrine)
A note in the piano roll renders its tier through **two channels at once**: a tier color AND a tier
shape/fill/mark. Solid = full confident fill; Tension = partial fill or distinct outline; Dissonance =
hot color plus a hatch/stripe and a small flag affordance. The note stays fully draggable and playable
in every tier — tiering is information, never a gate. The same tier vocabulary appears in the "notes you
could play now" readout so the two surfaces always agree.

A note whose pitch bends (it carries a pitch envelope) keeps **one tier, read at its start pitch** —
bending never recolours it — and adds a **moving mark**: a dark folded top-right corner, distinct from
the tension dot, the dissonance hatch and the keep outline, sized off the row so it survives the
smallest rows, and static (nothing animates, so reduced motion needs no special case). The inspector's
explanation says it plainly: it bends, judged where it starts.

## 6. Do's and Don'ts

### Do:
- **Do** commit to the dark surface and let one characterful brand color carry identity.
- **Do** give every note-tier a non-color cue (shape, pattern, border, or label) — always.
- **Do** keep numbers, note names, and chord symbols in monospace so data lines up.
- **Do** make density legible through guidance and labeling; pro-tool depth is welcome.
- **Do** use motion to convey state (playhead, recoloring tiers, flag reactions) at 150–250ms.

### Don't:
- **Don't** build the **childish toy** — no candy buttons, cartoon mascots, rounded-everything, confetti.
- **Don't** build the **generic SaaS dashboard** — no cards-on-grey, no corporate blue, no Inter-everywhere.
- **Don't** go **sterile/clinical** — no cold all-white, over-minimal Apple-stock neutrality.
- **Don't** ever signal a note-tier by color alone (the Three-Tier Doctrine).
- **Don't** disable, block, or auto-correct a "wrong" note — label it and let it be chosen.
- **Don't** use fluid `clamp()` type or orchestrated load animations; this is a tool, not a landing page.
