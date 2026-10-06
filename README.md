# Shuflo

[![Release](https://github.com/rotyro-tools/shuflo/actions/workflows/release.yml/badge.svg)](https://github.com/rotyro-tools/shuflo/actions/workflows/release.yml)
[![Coverage](https://img.shields.io/badge/coverage-100%25-brightgreen)](https://github.com/rotyro-tools/shuflo)
[![Commitizen friendly](https://img.shields.io/badge/commitizen-friendly-brightgreen.svg)](http://commitizen.github.io/cz-cli/)

Truly random in-place shuffle for Spotify playlists. Built with [Tauri](https://tauri.app).

Shuflo rewrites a playlist you own or collaborate on into a uniformly random order, so you can press play with Spotify's shuffle turned off and get a fair mix.

> Shuflo is not affiliated with or endorsed by Spotify.

---

## Features

- **Unbiased shuffle** — Fisher–Yates over the OS random source; every order is equally likely
- **Fast** — a 1,000-track playlist is rewritten in about 30 requests
- **Never lose a playlist** — the order is saved before every change, restored automatically if a write fails, and restorable by hand from Settings
- **One-click Spotify login** — Authorization Code + PKCE, no client secret; the login is kept in the OS keychain
- **Regular desktop app** — one window in the Dock/taskbar, a small tray menu, and optional launch at login
- **Auto-updates** — checks for new versions at startup

## Platforms

| Platform              | Support |
| --------------------- | ------- |
| macOS (Apple Silicon) | ✅      |
| macOS (Intel)         | ✅      |
| Windows               | ✅      |
| Linux                 | ✅      |

> **macOS:** on first launch you may see an "unidentified developer" warning — go to **System Settings → Privacy & Security** and click **Open Anyway** next to Shuflo. macOS may also ask once to let Shuflo use its keychain item; click **Always Allow**.
>
> **Windows:** you may see a SmartScreen warning — click **More info → Run anyway**.

---

## Installation

Download the latest release for your platform from [Releases](https://github.com/rotyro-tools/shuflo/releases).

| Platform            | File                          |
| ------------------- | ----------------------------- |
| macOS Apple Silicon | `Shuflo_x.y.z_aarch64.dmg`    |
| macOS Intel         | `Shuflo_x.y.z_x64.dmg`        |
| Windows             | `Shuflo_x.y.z_x64-setup.exe`  |
| Linux               | `shuflo_x.y.z_amd64.AppImage` |

---

## Usage

1. Launch Shuflo — its window opens (on macOS, closing it keeps Shuflo in the Dock; on Windows and Linux, closing it quits)
2. Add your Spotify app's Client ID — the first step explains where to get it (see [Spotify setup](#spotify-setup-one-time))
3. Click **Connect Spotify** and log in with the account that owns the playlist, or one of its collaborators
4. Pick the playlist, or paste its link
5. Click **Shuffle playlist**
6. Play the playlist in Spotify **with Spotify's shuffle turned off**

### What a shuffle changes

Shuflo replaces the playlist's contents with the same tracks in a new order. Every track's **date added** becomes the time of the shuffle, and **added by** becomes the logged-in account.

Playlists with local files or tracks that are no longer available on Spotify are left untouched, because rewriting them would drop those tracks. Remove them in Spotify first.

**Open in Spotify** opens the desktop app when it's installed, and the web player otherwise. If a Spotify app still shows the old order, reload it and check the playlist is sorted by **Custom order**.

### Restore a previous order

Shuflo saves the playlist's order before every change and keeps the 10 most recent per playlist. Click **Settings** at the bottom of the window, go to **Restore previous order**, and click **Restore** twice on the one you want.

---

## Configuration

Click **Settings** at the bottom of the window, or **Settings** in the tray menu.

- **Spotify account** — see which account is connected, and log out
- **Launch at login** — start Shuflo when you log in to your computer
- **Restore previous order** — put back one of the 10 saved orders (see above)
- **Advanced** — change the Spotify Client ID, or open the backups folder
- **Check for Updates** — download and install a newer version

---

## Spotify setup (one time)

Shuflo talks to Spotify through a Spotify developer app. The owner of the Spotify account that owns the playlist does this once:

1. Log in at [developer.spotify.com/dashboard](https://developer.spotify.com/dashboard) **as the account that owns the playlist** and create an app. Spotify requires the app owner to have Premium, and allows up to 5 users in development mode.
2. Under **Redirect URIs**, add both:
   - `http://127.0.0.1:8898/callback`
   - `http://127.0.0.1:8899/callback` (used when 8898 is busy)
3. Tick **Web API**, save, and copy the **Client ID**. With PKCE the Client ID is not a secret.
4. Paste the Client ID into Shuflo's first setup step. It can be changed later in **Settings → Advanced** (changing it logs out, since a login belongs to one Spotify app).

### Collaborators with their own account

Someone invited as a collaborator on the playlist can shuffle it from their own Spotify account:

1. The app's owner adds that account under **User Management** in the Spotify Developer Dashboard (up to 5 accounts in development mode). Until then Spotify refuses the login, and Shuflo says so.
2. The collaborator uses the same Client ID and connects with their own account.

Every shuffle sets "added by" to the account that shuffled. Backups stay on the computer that made them. If two people change the playlist at the same time, Shuflo stops before writing, or tells you afterwards so you can restore the previous order.

---

## Development

### Prerequisites

- [Node.js](https://nodejs.org) 24+
- [Rust](https://rustup.rs) (stable)
- **Linux only:** `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`

### Setup

```bash
git clone https://github.com/rotyro-tools/shuflo.git
cd shuflo
npm ci
npm run tauri dev
```

### Scripts

| Command                 | Description                      |
| ----------------------- | -------------------------------- |
| `npm run tauri dev`     | Start dev server with hot reload |
| `npm run tauri build`   | Build production app             |
| `npm run lint`          | Run ESLint                       |
| `npm run format`        | Format with Prettier             |
| `npm run typecheck`     | Type-check with TypeScript       |
| `npm run test`          | Run unit tests                   |
| `npm run test:coverage` | Run tests with coverage          |
| `npm run cm`            | Commitizen commit wizard         |
| `npm run release:new`   | Bump version, changelog and tag  |
