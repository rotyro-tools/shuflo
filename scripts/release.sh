#!/usr/bin/env bash
set -euo pipefail

# ── Usage ─────────────────────────────────────────────────────────────────────
# npm run release:new -- patch   # 0.1.0 → 0.1.1
# npm run release:new -- minor   # 0.1.0 → 0.2.0
# npm run release:new -- major   # 0.1.0 → 1.0.0
# npm run release:new -- 2.0.0   # explicit version

BUMP="${1:-}"
if [[ -z "$BUMP" ]]; then
  echo "Usage: $0 <patch|minor|major|x.y.z>" >&2
  exit 1
fi

# ── Pre-flight checks ─────────────────────────────────────────────────────────
if [[ -n "$(git status --porcelain)" ]]; then
  echo "❌  Working tree is dirty. Commit or stash changes first." >&2
  exit 1
fi

CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$CURRENT_BRANCH" != "main" ]]; then
  echo "❌  Must be on main branch (currently on '$CURRENT_BRANCH')." >&2
  exit 1
fi

# ── Resolve new version ───────────────────────────────────────────────────────
CURRENT_VERSION="$(node -p "require('./package.json').version")"

bump_semver() {
  local version="$1" part="$2"
  IFS='.' read -r major minor patch <<< "$version"
  case "$part" in
    major) echo "$((major + 1)).0.0" ;;
    minor) echo "${major}.$((minor + 1)).0" ;;
    patch) echo "${major}.${minor}.$((patch + 1))" ;;
    *)     echo "$part" ;;  # explicit version passthrough
  esac
}

NEW_VERSION="$(bump_semver "$CURRENT_VERSION" "$BUMP")"

if git rev-parse "v${NEW_VERSION}" &>/dev/null; then
  echo "❌  Tag v${NEW_VERSION} already exists." >&2
  exit 1
fi

echo "🔖  Bumping ${CURRENT_VERSION} → ${NEW_VERSION}"

# ── Bump version in all three files ──────────────────────────────────────────
# package.json
node -e "
  const fs = require('fs');
  const pkg = JSON.parse(fs.readFileSync('package.json', 'utf8'));
  pkg.version = '${NEW_VERSION}';
  fs.writeFileSync('package.json', JSON.stringify(pkg, null, 2) + '\n');
"

# src-tauri/tauri.conf.json
node -e "
  const fs = require('fs');
  const cfg = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8'));
  cfg.version = '${NEW_VERSION}';
  fs.writeFileSync('src-tauri/tauri.conf.json', JSON.stringify(cfg, null, 2) + '\n');
"

# src-tauri/Cargo.toml  (perl for cross-platform first-match substitution)
perl -i -0pe "s/^version = \"${CURRENT_VERSION}\"/version = \"${NEW_VERSION}\"/m" \
  src-tauri/Cargo.toml

# Refresh Cargo.lock
cargo generate-lockfile --quiet --manifest-path src-tauri/Cargo.toml

# Refresh package-lock.json
npm install --package-lock-only --silent

# ── Generate changelog from conventional commits ──────────────────────────────
LAST_TAG="$(git describe --tags --abbrev=0 2>/dev/null || echo "")"
LOG_RANGE="${LAST_TAG:+${LAST_TAG}..HEAD}"

get_commits() {
  local pattern="$1"
  git log ${LOG_RANGE} --oneline --no-decorate \
    | grep -E "^[a-f0-9]+ ${pattern}" \
    | sed -E "s/^[a-f0-9]+ ${pattern}: /- /" \
    || true
}

ADDED="$(get_commits 'feat(\([^)]+\))?')"
FIXED="$(get_commits 'fix(\([^)]+\))?')"
CHANGED="$(get_commits '(refactor|perf|style)(\([^)]+\))?')"

CHANGELOG_ENTRY="## [${NEW_VERSION}] — $(date +%Y-%m-%d)"
[[ -n "$ADDED"   ]] && CHANGELOG_ENTRY+=$'\n\n### Added\n'"$ADDED"
[[ -n "$FIXED"   ]] && CHANGELOG_ENTRY+=$'\n\n### Fixed\n'"$FIXED"
[[ -n "$CHANGED" ]] && CHANGELOG_ENTRY+=$'\n\n### Changed\n'"$CHANGED"

echo ""
echo "──────────────────────────────────────────"
echo "$CHANGELOG_ENTRY"
echo "──────────────────────────────────────────"
echo ""

read -rp "Proceed with commit + tag? [y/N] " CONFIRM
if [[ "$CONFIRM" != "y" && "$CONFIRM" != "Y" ]]; then
  echo "Aborted. Restoring files..."
  git checkout -- package.json package-lock.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
  exit 0
fi

# Prepend entry to CHANGELOG.md
CHANGELOG_FILE="CHANGELOG.md"
if [[ -f "$CHANGELOG_FILE" ]]; then
  EXISTING="$(cat "$CHANGELOG_FILE")"
  echo -e "${CHANGELOG_ENTRY}\n\n${EXISTING}" > "$CHANGELOG_FILE"
else
  echo "$CHANGELOG_ENTRY" > "$CHANGELOG_FILE"
fi

# ── Commit + tag ──────────────────────────────────────────────────────────────
git add package.json package-lock.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock "$CHANGELOG_FILE"
git commit -m "chore(release): v${NEW_VERSION}"
git tag -a "v${NEW_VERSION}" -m "Release v${NEW_VERSION}"

echo ""
echo "✅  Done. Push with:"
echo "    git push origin main --tags"
