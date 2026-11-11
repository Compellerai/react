#!/bin/bash
# Shared build/run environment for start-react.sh and build.sh.
# Source this file; do not execute it.

# ${BASH_SOURCE[0]} under bash; $0 is the sourced file under zsh.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)"

# cargo/rustup live in the invoking user's home, which is not $HOME under sudo.
_user_home="$HOME"
if [ -n "${SUDO_USER:-}" ]; then
    _user_home="$(eval echo "~$SUDO_USER")"
fi
if [ -d "$_user_home/.cargo/bin" ]; then
    export PATH="$_user_home/.cargo/bin:$PATH"
    export CARGO_HOME="${CARGO_HOME:-$_user_home/.cargo}"
    export RUSTUP_HOME="${RUSTUP_HOME:-$_user_home/.rustup}"
fi

# Homebrew tools (pkg-config, cmake, ffmpeg) when launched outside a login shell
export PATH="/opt/homebrew/bin:$PATH"
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/ffmpeg/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"

# Private settings: license key, dev API host. Untracked — see .env.example.
if [ -f "$REPO_ROOT/.env.local" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$REPO_ROOT/.env.local"
    set +a
fi

# The NDI SDK is proprietary and not part of this repository. Point NDI_SDK_DIR
# at your copy, or place it in ./ndi-sdk (gitignored).
export NDI_SDK_DIR="${NDI_SDK_DIR:-$REPO_ROOT/ndi-sdk}"

# Keep local filesystem paths (home directory, checkout location) out of the
# compiled binary. CARGO_ENCODED_RUSTFLAGS is \x1f-separated, so paths with
# spaces are safe. Both scripts share these flags, so switching between them
# never invalidates the build cache.
_sep=$'\x1f'
export CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix${_sep}${CARGO_HOME:-$_user_home/.cargo}/registry/src/=${_sep}--remap-path-prefix${_sep}${REPO_ROOT}/="
unset _sep _user_home
