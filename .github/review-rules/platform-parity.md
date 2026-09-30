---
paths:
  - src-tauri/src/**
---
The `media`, `persistent_backdrop`, and `overlay_z_order` modules expose the same
interface through their Windows and macOS backends. Flag a change that alters the
interface on one platform only, or that adds `cfg(target_os)` branching outside
the backend files. Platform behavior that works on one OS only is a finding.
