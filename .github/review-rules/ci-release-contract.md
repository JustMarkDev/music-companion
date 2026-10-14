---
paths:
  - .github/workflows/**
  - .releaserc.json
  - src-tauri/tauri*.conf.json
---

Keep the CI contract: changed-area frontend and Rust/Tauri jobs feed the stable
`CI` gate, native validation runs on Windows and macOS, and
Dependabot stays separate from pull-request CI, and the `release` job in `ci.yml` runs only after the `CI` gate passes on a push to `main`. Keep the release contract: semantic-release on pushes to `main` cuts `v*` tags from
conventional commits and bumps `tauri.conf.json`, `Cargo.toml` and `Cargo.lock`
together; a draft release gets a signed Windows x86-64 NSIS installer with updater
metadata and a universal macOS DMG, and is published only after every build
succeeds; the macOS build also creates updater artifacts, but it joins the updater manifest
only when Apple Developer ID secrets are set; without them the DMG is unsigned and
macOS updates stay manual. Flag any change that breaks either contract.
