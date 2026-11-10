# Contributing to REACT

Thanks for helping make live visuals and lighting more accessible to musicians
and venues.

## Getting set up

Follow [Building](README.md#building) in the README, then:

```bash
./start-react.sh                                            # build and run
source scripts/env.sh && cargo test --features camera       # run the tests
```

No private settings are needed for either. `.env.example` describes the
optional ones.

## Before you open a pull request

- The test suite passes.
- The change is focused, and the description says what you changed and how you
  tested it. For hardware features, name what you tested against (DMX fixtures,
  Hue, WLED, CDJs, NDI receivers).
- Nothing private is committed: `.env.local`, signing identities, keys, the NDI
  SDK, model weights. The `.gitignore` covers the usual locations.
- New third-party code or assets are listed, with their license, in
  [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

## Reporting bugs

Open an issue with your macOS version and Mac model, what you did, what
happened, and the relevant log output:

```bash
RUST_LOG=info ./start-react.sh
```

## Licensing of contributions

REACT is licensed under the [Apache License 2.0](LICENSE). Contributions you
submit are accepted under the same license (Section 5).
