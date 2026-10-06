# Contributing

Thank you for your interest in contributing to this project. These guidelines are intended to make contributing clear and easy for everyone.

## How to Contribute Code

1. Fork the repository and create a topic branch from `main`:
   ```bash
   git checkout -b feat/my-feature
   ```
2. Make changes and keep commits focused and atomic.
3. Run tests, linting, and formatting locally.
4. Push your branch and open a pull request against `main`.
5. Describe the changes and link related issues (if any).

## Tests

- Add or update tests for new features and bug fixes.
- Ensure test coverage remains as close to 100% as possible.
- For test failures, run tests locally and include the failing output in your PR for context.

## Commit Messages

This project follows the [Conventional Commits](https://www.conventionalcommits.org) specification. Examples:

- `feat: show the playlist cover in the main window`
- `fix: retry playlist reads after a network error`
- `chore(ci): update Node matrix`

Commit messages are validated using commitlint and Husky (pre-commit hooks).

To help format your commits correctly, you can use Commitizen:

```bash
npm run cm
```

This will guide you through creating a properly formatted commit message interactively.

## Quality Requirements

All of the following must pass before a PR can be merged. The pre-commit hook enforces them locally:

```bash
npm run lint          # ESLint
npm run format:check  # Prettier
npm run typecheck     # TypeScript
npm test              # Vitest (TypeScript)
cargo fmt --check     # Rust formatting
cargo clippy          # Rust lints (warnings treated as errors)
cargo test            # Rust unit tests
```

Run everything at once:

```bash
npm run lint && npm run format:check && npm run typecheck && npm test && \
cargo fmt --manifest-path src-tauri/Cargo.toml --check && \
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings && \
cargo test --manifest-path src-tauri/Cargo.toml
```

## Review & Release

- Maintainers will review and request changes as needed.
- Releases are handled via the release script and GitHub Actions; maintainers will handle publishing.

Thank you for helping to improve this project!
