# CLAUDE.md

Guidance for AI coding agents working in this repository.

## Dependencies

Always use the latest stable versions of crates/dependencies and their current
APIs. Do not use deprecated APIs or outdated patterns. Check crates.io for
current versions and documentation for up-to-date usage examples.

## UI stability rule

When building egui menu panels with dynamic/conditional content, ALWAYS render
all UI rows with placeholder text when inactive, rather than conditionally
showing/hiding rows. This prevents the menu from jumping and moving buttons out
from under the user's cursor. Use dimmed colors (e.g.,
`Color32::from_rgb(50, 50, 60)`) for placeholder content. The structure should
be constant — only the text/colors change.

## Never commit private material

This repository is public. Signing identity, notarization credentials and the
development API host live only in the untracked `.env.local`, loaded by
`scripts/env.sh`. Do not add them, internal hostnames, or the NDI SDK to any
tracked file.

License blobs are verified with Ed25519 public keys committed in
`src/auth/license.rs`. Public keys belong in the source; the matching private
key exists only on the license server and must never appear here — not in
code, tests, fixtures or docs. Tests generate throwaway keys at run time.

Release packaging is private too. `build.sh` stops at an ad-hoc signed `.app`;
official releases are signed, notarized and turned into an installer by an
untracked `release.sh`. Do not add Developer ID signing, notarization or
installer steps to `build.sh` or any other tracked file.
