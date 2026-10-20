# AGENTS.md

## Tests

- Every change to behavior must come with tests. New modules and new logic need unit tests in a `#[cfg(test)] mod tests` block in the same file; bug fixes need a regression test that fails without the fix.
- Tests must not depend on a desktop session, audio device or network. Use the simulated output (`Player::new_test`) for playback; mark anything that needs real hardware with `#[ignore = "reason"]`.
- Tests must pass on Linux, macOS and Windows. CI (`.github/workflows/ci.yml`) runs them on all three on every push, pull request and release.

## Before committing

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

All three must pass locally; CI enforces the same checks.
