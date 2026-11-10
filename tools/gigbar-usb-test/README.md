# gigbar-usb-test

Standalone **Open DMX USB** bench test for the **Chauvet DJ GigBAR Move**. Fires
every section of the bar — 2 pars, 2 derbies, laser, white/UV strobe, 2 moving
heads — so you can verify the DMX wiring and addressing against the real fixture.

It uses the **same `open_dmx` crate REACT uses** (`src/dmx/usb.rs`) for
FTDI "Open DMX USB" (FT232RL bit-bang) cables, but is fully standalone: **no
SaaS, no login, no app restart.** It does NOT go through the music
autopilot — in the live app the Laser and Strobe lanes are gated dark at idle
(they only fire on a drop / big groove), so a passive test of the app looks like
those sections "aren't firing." This tool drives deterministic values from the
**GigBAR MOVE User Manual Rev. 4** DMX chart so every channel is exercised.

## Run

```bash
cd react/tools/gigbar-usb-test

# Default: cycle each section in turn, then ALL together (autodetects the cable).
cargo run

# Be explicit (recommended — match your bar's menu):
cargo run -- --port /dev/cu.usbserial-AB0MWWLR --personality 35 --base 1 --mode cycle

# Light everything at once (movers sweep):
cargo run -- --mode all

# One section only (verify a single address block):
cargo run -- --mode laser        # par1 par2 derby1 derby2 laser strobe mh1 mh2 (35ch)

# Turn the bar off:
cargo run -- --mode blackout
```

## Set the bar first

On the GigBAR Move menu, set the DMX **personality** and **start address** to
match `--personality` / `--base`:

- `--personality 35` → bar menu in **35-channel** mode (full per-section control).
- `--personality 17` → bar menu in **17-channel** mode (pars+derby merged, one
  control for both heads).
- `--base N` → bar's DMX start address (its `d NNN` menu value).

## Modes

| `--mode`    | what it does |
|-------------|--------------|
| `cycle`     | lights each section for `--hold` secs (default 2.5), then ALL, looping. Best for verifying **per-section addressing** — the active section prints to the console. |
| `all`       | every section lit at once; both heads sweep counter-phase. |
| `blackout`  | sends zeros (turns the bar off). |
| `<section>` | one section: `par1 par2 derby1 derby2 laser strobe mh1 mh2` (35ch) / `pars laser strobe heads` (17ch). |

## ⚠️ Safety

This is a **laser** fixture. Aim it at a wall, never at eyes, and keep the strobe
section in mind for photosensitivity. **Ctrl-C** blacks the bar out cleanly before
exit.

## Why standalone (and what's next)

REACT renders each fixture through a **flat ChannelMap** (one `red`/`pan`/
`laser_macro` slot), so today a 35-channel GigBAR collapses last-wins into a
single section. The GigBAR stays **one fixture** (`chauvet-dj-gigbar-move.json`,
never split). Making the **live app** drive all sections of that single combined
fixture is the follow-up: expand the combined profile into per-section render
intents internally (one file, one stage fixture), honour the profile's
per-channel static values (e.g. the par dim/strobe channel), and let the
autopilot drive every section. This tool de-risks the hardware/addressing first
and is independent of that work — it drives the raw 35-channel layout directly.
