# Security Policy

## Supported Versions

Only the latest released version of this project is supported with security updates.

---

## Reporting a Vulnerability

If you discover a security vulnerability, **please do not open a public issue**. Instead, report it privately using [GitHub Security Advisories](https://github.com/rotyro-tools/shuflo/security/advisories/new).

Please include:

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Any suggested fixes

We will acknowledge your report as soon as reasonably possible.

---

## Disclosure Policy

- We aim to respond to security reports within **7 days**
- We will work on a fix and coordinate disclosure if needed
- Public disclosure should only happen **after a fix is released**

---

## Scope

Areas of particular concern for this app:

- **Spotify login** — Authorization Code + PKCE through a one-shot listener on `127.0.0.1:8898` (or `8899`); the callback is checked against a random `state`
- **Token storage** — the refresh token is kept in the OS keychain (`keyring`); access tokens stay in memory and never reach the webview
- **IPC commands** — Tauri commands are window-origin guarded, and playlist and backup IDs are validated before they reach URLs or file paths
- **Outbound requests** — HTTPS to `api.spotify.com` and `accounts.spotify.com` only; pagination links pointing anywhere else are refused
- **Webview content policy** — the window loads only its own files, plus playlist cover images from Spotify's image hosts (`*.scdn.co`, `*.spotifycdn.com`); all API calls happen in Rust
- **Backups** — playlist orders are written as JSON under the app data folder

---

## Thank You

We appreciate responsible disclosure and the efforts of security researchers who help keep open source projects safe.
