# Virtual Stage Smoke Path

This is the small COM-407/COM-408 smoke path for Friday/Saturday stream prep.
It is not the final Design Mode renderer.

## Enable

From the repository root:

```bash
REACT_VIRTUAL_STAGE=1 PATH=/opt/homebrew/bin:$PATH cargo run
```

Optional physical DMX backends can still be enabled independently:

```bash
REACT_VIRTUAL_STAGE=1 REACT_DMX_BACKEND=ola REACT_DMX_OLA_HOST=127.0.0.1 REACT_DMX_OLA_PORT=9010 cargo run
REACT_VIRTUAL_STAGE=1 REACT_DMX_BACKEND=artnet REACT_DMX_ARTNET_TARGET=192.168.1.50:6454 cargo run
```

## What should move

The overlay is operator/debug UI only. It is fake visual geometry, but every value
comes from the same lighting intent -> DMX frame -> virtual fixture evaluator path
used by physical DMX output.

- `react-demo-wash`: RGBW wash responds to energy and color.
- `react-demo-moving-head`: pan, tilt, dimmer, strobe, color wheel, and gobo-ish state respond to the cue snapshot.
- `react-demo-laser`: macro and intensity respond to energy/tension.
- Blackout hard-zeros the generated DMX frame and the evaluated virtual fixture state.

## Gate

```bash
PATH=/opt/homebrew/bin:$PATH cargo test dmx -- --nocapture
PATH=/opt/homebrew/bin:$PATH cargo check
PATH=/opt/homebrew/bin:$PATH cargo check --locked
```
