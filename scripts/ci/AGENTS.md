# CI Script Guide

## Scope

This directory owns repository-local CI contracts: native artifact identity and
verification, prerequisite checks, package/race inventory, and named entry
points consumed by workflows. Workflows choose supported runners and invoke
these scripts; they do not duplicate their validation policy.

## Boundaries

- Native artifact manifests are candidate-SHA and platform bound. Treat every
  downloaded artifact as untrusted until `verify-native-artifact.sh` completes.
- Manifest paths are repository-relative and must resolve below the explicit
  repository root. Verification must finish before any library copy or native
  dependent command.
- Scripts fail closed on invalid arguments, malformed files, missing tools, and
  I/O failures. Caches and locally built products never substitute for a
  verified candidate artifact.
- Keep platform ownership explicit: Linux server bundles and macOS client
  libraries have different required file sets. Do not add cross-platform
  fallback lookup or rebuild behavior.

## Entry points and validation

- `platform-id.sh` normalizes the supported runner platform.
- `package-native-artifact.sh` writes deterministic manifest v1 files.
- `verify-native-artifact.sh` validates manifest v1 files and stages verified
  dynamic libraries in the engine `deps` directory.
- `run-rust-quality.sh` prepares the locked Agent and sealed previous runtime
  in an owned temporary directory before the unchanged workspace Rust gate.
- Add real-script regression coverage in `packages/audit`; run `bash -n` for
  changed shell scripts and the focused audit test before broader CI gates.
