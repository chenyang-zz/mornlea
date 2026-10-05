# Mornlea character and farming-tool assets

Original model assets, published 2026-10-03. This is an asset-only handoff; no runtime code, resource registry, controller, collision, particle effect, or sound integration is included.

## Directory map

- `characters/block_farmer_v06/runtime/`: dressed animated character, base body, and six separate wardrobe GLBs
- `characters/block_farmer_v06/source/`: editable Blender 4.3.2 scene
- `characters/block_farmer_v06/previews/`: latest v06 side-view transition video, shown at half speed
- `characters/block_farmer_v06/animation_manifest_v06.json`: clip names, timings, and loop intent
- `characters/block_farmer_v06/wardrobe_contract_v06.json`: relative resource paths, shared skeleton, and body-masking rules
- `tools/prototype_v01/runtime/`: independent hoe, axe, and watering-can GLBs
- `tools/prototype_v01/source/`: editable tool-only Blender scene collection; no obsolete character is retained
- `asset_manifest.json`, `SHA256SUMS`, and `validation_report.json`: inventory, byte integrity, and asset-level checks

Filenames use lower_snake_case, semantic asset names, and explicit versions. Character v06 and tool prototype v01 are separate version lines. Older character versions, comparison renders, redundant ZIP archives, temporary files, and incomplete rebuild scripts are intentionally absent.

## Character and wardrobe contract

One Blender unit equals one meter. Blender authoring is Z-up and faces -Y; standard GLB conversion is Y-up and faces +Z. Do not scale by 100 or apply the axis conversion twice.

The dressed character and base body each contain the same twelve clips: Idle, Walk, Run, Crouch_Down, Crouch_Idle, Crouch_Up, Prone_Down, Prone_Idle, Prone_Up, Sleep_Enter, Sleep_Loop, and Sleep_Exit. The six wardrobe modules are hair, shirt, overalls, boots, hat, and pouch. Every character GLB retains the common 21-joint skin and matching inverse-bind matrices; wardrobe GLBs contain no independent animation clips.

Use one shared master skeleton when integrating wardrobe meshes. Do not animate each imported wardrobe skeleton separately. Preserve the rest pose, joint names, inverse-bind matrices, and slot body masks. Body regions covered by shirt, overalls, or boots are hidden according to the wardrobe contract. These are articulated modular shells, not cloth simulation.

The v06 transition correction is present in the shipped source and GLBs. Authored motion assumes a ground plane and fixed root; terrain-aware IK, controller transitions, interruption behavior, retargeting, and production-engine acceptance remain unimplemented or unverified. The video is a visual reference, not proof of engine integration.

## Independent tool prototypes

The hoe, axe, and watering can are rigid assets. Their root origins are primary grip points. All three expose `Grip_Primary` and `Grip_Secondary`; hoe and axe expose `Effect_Contact`, and the can exposes `Effect_WaterOutlet`. The hoe and axe handles point along Blender +Z / GLB +Y; the can spout points along Blender +X.

The prototypes were made before the current character rig. They are not attached to v06, and they include no animations, IK controller, physics colliders, particles, audio, farming logic, damage, or water simulation. Position and orient the primary grip at the character's hand socket; secondary-hand solving and event timing need separate engine implementation and validation. Treat effect markers as reference transforms, not gameplay authority or physical joints.

## Editing and export

Open `source/block_farmer_animated_v06.blend` in Blender 4.3.2 for character edits. The tool source opens at `ASSET_Hoe`; select `ASSET_Axe` or `ASSET_WateringCan` for the other clean tool scenes. All materials are self-contained solid-color PBR; no external textures or linked libraries are required.

For export, select only the intended model plus its rig for characters, or the intended tool hierarchy for tools. Preserve existing action names, units, rest skeleton, inverse-bind matrices, grip/effect nodes, and body masks. Export binary glTF 2.0 with skins and named animation actions enabled for dressed/base characters; export wardrobe modules and tools without animation. Re-import each exported file into a fresh Blender scene before replacing these artifacts. This package does not claim a one-command rebuild from historical scripts; the editable scenes are the authoring source of truth.

## Validation

From this directory run `sha256sum -c SHA256SUMS`. `validation_report.json` records structural GLB checks, finite numeric accessor values, buffer bounds, relative contract paths, skeleton compatibility, clip inventory, self-contained references, and source reopen checks performed during packaging. Source sanitization changes only local output/UI paths; the character geometry, skin weights, materials, rig transforms, and action curves were checked unchanged.

These checks do not constitute a complete glTF conformance certification or Godot/Mornlea integration test. No game code was changed or executed by this publication.
