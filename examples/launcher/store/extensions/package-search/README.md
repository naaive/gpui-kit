# Package Search

Search package registries and web documentation from the launcher, as you type.

## Commands

- **Search npm**: packages on the npm registry with version, weekly downloads,
  license and links. Open on npmjs.com, open the repository, or copy
  `npm install <name>` (pnpm, Yarn and Bun variants are in a submenu).
- **Search crates.io**: Rust crates with version and downloads. Copy
  `cargo add <name>`, open docs.rs or crates.io.
- **Search PyPI**: PyPI has no search API, so this looks a package up by its
  exact name (normalized as PyPI does) and says so when no package has that
  name. Copy `pip install <name>` (uv, Poetry and pipx in a submenu).
- **Search MDN**: pages of MDN Web Docs with their summary; open one or copy
  its URL.

Every command takes an optional query argument, so typing `Search npm react`
in the root search starts with results.

## Setup

None. No account or token is needed.

## Permissions

GET requests only, each to one path:

- `registry.npmjs.org` `/-/v1/search`
- `crates.io` `/api/v1/crates` (with a User-Agent naming this extension, as
  crates.io requires)
- `pypi.org` `/pypi/…` (the JSON API of one package)
- `developer.mozilla.org` `/api/v1/search`
