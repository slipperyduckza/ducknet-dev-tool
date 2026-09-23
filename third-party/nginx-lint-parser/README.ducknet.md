# Vendored: nginx-lint-parser

Upstream: https://github.com/walf443/nginx-lint (MIT — see LICENSE.upstream-MIT)

Why vendored instead of a path dep into the `NGINX/nginx-lint` checkout or a
crates.io version: so the reference clone can be deleted without affecting
the build, and the AppImage build stays hermetic (no network fetch, no 30+
WASM/plugin workspace members).

Only this one crate is used — the rowan-based nginx config parser with error
recovery (`parse_string_with_errors`, `parse_string_rowan`, `LineIndex`).
Our own checks live in `src/nginxlint.rs`, including a missing-semicolon CST
walk ported from upstream's `src/rules/syntax/missing_semicolon.rs`.

Do not edit the vendored sources; upgrade by re-copying from upstream.
