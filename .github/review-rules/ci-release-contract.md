---
paths:
  - .github/workflows/**
  - src-tauri/tauri*.conf.json
---

Keep the CI contract: changed-area frontend and Rust/Tauri jobs feed the stable
`Pull request validation` gate, native validation runs on Windows and macOS, and
Dependabot stays separate from pull-request CI. Keep the release contract: `v*`
tags publish a signed Windows x86-64 NSIS installer with updater metadata and a
universal macOS DMG; without Apple Developer ID secrets the macOS DMG is unsigned
and its updater stays disabled. Flag any change that breaks either contract.
