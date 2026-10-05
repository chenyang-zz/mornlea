# Mornlea model assets

This directory owns the original character and farming-tool handoff. It inherits repository guidance. Child folders inherit this guide because they share the same asset lifecycle and validation boundary.

## Directory map

- `characters/block_farmer_v06/`: current editable character, runtime wardrobe modules, contracts, and preview
- `tools/prototype_v01/`: independent rigid tool prototypes and tool-only editable source
- `README.md`: authoring and integration limitations
- `asset_manifest.json`, `SHA256SUMS`, `validation_report.json`: inventory and validation evidence

## Ownership and dependency boundaries

Blender scenes own authored geometry and motion. Runtime GLBs derive from those scenes; manifests must refer only to relative paths that exist here. Publication is not runtime registration or controller integration. Do not import obsolete character assets into the independent tool source. Do not turn grip or effect marker nodes into gameplay authority.

## Lifecycle and synchronization

Keep semantic lower_snake_case filenames and explicit versions. After any model edit, re-export and freshly import affected GLBs, verify joint and animation contracts, and update the inventory, hashes, and validation evidence together. Keep the README explicit about unverified engine behavior. Preserve one shared rest skeleton and inverse-bind matrices across character and wardrobe assets.

## Focused verification

Run `sha256sum -c SHA256SUMS` from this directory. Reopen affected Blender sources and import each affected GLB in a fresh scene. Check embedded buffer bounds, finite accessor values, joint names and inverse-bind matrices, named clips, grip/effect markers, and relative contract paths. Use asset review for checks not enforced by a production test; do not represent an asset check as an engine acceptance gate.
