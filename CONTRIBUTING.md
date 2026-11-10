# Contributing

Thanks for helping make Sethu better. This project is intentionally small and
boring to build; please keep it that way.

## Ground rules

- Be respectful. See the [Contributor Covenant](https://www.contributor-covenant.org/).
- Security issues: follow [SECURITY.md](SECURITY.md), never a public issue.
- By contributing you agree your work is licensed under the MIT license.

## Development setup

```bash
git clone https://github.com/your-org/sethu.git
cd sethu
cargo test --workspace
SETHU_WEB_DIR=./web RUST_LOG=sethu_server=debug cargo run
```

No Node, npm or bundler is required for the frontend. It is plain ES modules.
If you want noVNC offline: `./scripts/vendor-novnc.sh`.

For device testing you need at least one of:

- a Mac with Screen Sharing and the legacy VNC password enabled,
- a Mac with Xcode + WebDriverAgent for an iPhone.

## Before opening a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Add tests for server behaviour. Integration tests live in
  `crates/sethu-server/tests/api.rs` and drive the real router.
- Keep frontend code framework-free and dependency-free.
- Do not interpolate user data with `innerHTML`. Use `textContent` or DOM APIs.
- Update `README.md` / `ARCHITECTURE.md` when behaviour or configuration changes.
- Keep commit messages in the imperative mood, e.g. `add iPhone rotation
  handling` or `fix CSRF check for PATCH /api/hosts`.

## Good first issues

- Add an iOS "press home" test using a fake WDA server.
- Improve the noVNC reconnect UX.
- Vendor noVNC assets into releases.
- Add a TOTP second factor.
- Package for Homebrew, Nix and `cargo install`.

## Code layout

See [ARCHITECTURE.md](ARCHITECTURE.md). In short:

- `api.rs` is HTTP-only; keep device protocol logic out of it.
- `wda.rs` is the only place that talks to WebDriverAgent.
- `ws/*.rs` own their protocol loops.
- `security.rs` is the place for cross-cutting request policy.
- `store.rs` must stay synchronous and atomic.

## Adding a device kind

1. Add a variant to `HostKind` in `store.rs` (JSON name is part of the format).
2. Validate and canonicalise its target in `target.rs`.
3. Add a WebSocket route under `/ws/` and a viewer module in `web/`.
4. Update the API view, UI picker, tests and docs.
