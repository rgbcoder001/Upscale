# Contributing

## Commit identity

The repository is public, so every commit and tag must use a GitHub noreply address. Set this once per clone:

```sh
git config user.name "<your GitHub username>"
git config user.email "<id>+<username>@users.noreply.github.com"
git config core.hooksPath .githooks
```

`.githooks/pre-push` refuses pushes with any other address, and the `identity` CI job checks every commit and tag. Don't bypass them; fix the identity instead.

Keep personal details out of committed files: names, hardware, home network or storage names, and library paths.

## Conventions

- **Rust:** stable toolchain, `cargo fmt`, and `clippy -D warnings`. Use `thiserror` in `core` and `anyhow` at the edges.
- **`core`:** no Tauri dependency. The app and the CLI use the same API.
- **External commands:** built by pure functions with snapshot tests, as argument vectors. Never use shell strings, because paths contain spaces, apostrophes and brackets.
- **Binary video pipes:** never pass them through text or line decoding.
- **Numbers:** always use `.` as the decimal separator, whatever the locale.
- **Platform differences:** keep them in `platform.rs` and `tools/`.
- **Tests:** fixtures go in `core/tests/fixtures/`. A test that needs a GPU, Topaz or network goes in the ignored machine test group.
