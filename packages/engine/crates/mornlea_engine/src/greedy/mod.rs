use crate::input::MeshInput;
use crate::light::LightScratch;
use crate::native::contracts::{KernelError, MeshQuad, MeshRegistryEntry, MeshView};
use crate::quad::{FULL_FLUID_HEIGHT, Face, Quad, plant_material};

mod bed;
mod torch;

use torch::mesh_models;
#[cfg(test)]
pub(crate) use torch::{TORCH_QUADS_PER_STANDING_CELL, TORCH_QUADS_PER_WALL_CELL};

/// Direct-write lane failure: the caller slice is short.
/// Test-only since the ABI publishes from the staged typed stream, whose
/// short destination reports the exact count instead.
#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum MeshError {
    OutputOverflow,
}

/// Read capability the shared geometry core needs from one section lane.
///
/// The byte ABI lane (`MeshInput` plus its borrowed `RegistryView`) and the
/// typed lane (`MeshView` plus its validated contract registry) answer the
/// same geometry questions, so emission runs once for both lanes and neither
/// can drift from the other's face order, merge rules or finite-model dispatch.
pub(crate) trait MeshAccess {
    fn block(&self, x: i32, y: i32, z: i32) -> u16;
    fn air_id(&self) -> u16;
    fn face_visible(&self, id: u16, adjacent: u16) -> bool;
    fn material(&self, id: u16, face: usize) -> Option<u16>;
    fn model(&self, id: u16) -> u8;
    fn fluid_height(&self, id: u16) -> Option<u8>;
    fn block_top_raw(&self, id: u16) -> Option<u8>;
    fn opaque(&self, id: u16) -> bool;
}

impl MeshAccess for MeshInput<'_> {
    fn block(&self, x: i32, y: i32, z: i32) -> u16 {
        MeshInput::block(self, x, y, z)
    }

    fn air_id(&self) -> u16 {
        self.air_id
    }

    fn face_visible(&self, id: u16, adjacent: u16) -> bool {
        self.registry.face_visible(id, adjacent)
    }

    fn material(&self, id: u16, face: usize) -> Option<u16> {
        self.registry.material(id, face)
    }

    fn model(&self, id: u16) -> u8 {
        self.registry.model(id)
    }

    fn fluid_height(&self, id: u16) -> Option<u8> {
        self.registry.fluid_height(id)
    }

    fn block_top_raw(&self, id: u16) -> Option<u8> {
        self.registry.block_top_raw(id)
    }

    fn opaque(&self, id: u16) -> bool {
        self.registry.opaque(id)
    }
}

/// Read access over one validated typed section view.
///
/// Mirrors the byte lane's query surface exactly: ids resolve through the
/// strictly sorted entry table, an absent id answers the same neutral values
/// `RegistryView` returns, a face outside `0..6` has no material, and the `0`
/// sentinel in the fluid/top height fields means "not that geometry" just like
/// the wire fields.
pub(crate) struct TypedMeshAccess<'a> {
    view: MeshView<'a>,
    entries: &'a [MeshRegistryEntry],
    visibility: &'a [u64],
    words_per_row: usize,
}

impl<'a> TypedMeshAccess<'a> {
    pub(crate) fn new(view: &MeshView<'a>) -> Self {
        let entries = view.registry.entries();
        Self {
            view: *view,
            entries,
            visibility: view.registry.visibility(),
            words_per_row: entries.len().div_ceil(64),
        }
    }

    fn index(&self, id: u16) -> Option<usize> {
        self.entries
            .binary_search_by(|entry| entry.id.cmp(&id))
            .ok()
    }
}

impl MeshAccess for TypedMeshAccess<'_> {
    fn block(&self, x: i32, y: i32, z: i32) -> u16 {
        match crate::input::typed_cell_index(x, y, z) {
            Some(index) => self.view.blocks[index],
            None => self.view.registry.barrier(),
        }
    }

    fn air_id(&self) -> u16 {
        self.view.registry.air()
    }

    fn face_visible(&self, id: u16, adjacent: u16) -> bool {
        let (Some(row), Some(column)) = (self.index(id), self.index(adjacent)) else {
            return false;
        };
        self.visibility[row * self.words_per_row + column / 64] & (1_u64 << (column % 64)) != 0
    }

    fn material(&self, id: u16, face: usize) -> Option<u16> {
        if face >= 6 {
            return None;
        }
        self.index(id)
            .map(|index| self.entries[index].material[face])
    }

    fn model(&self, id: u16) -> u8 {
        self.index(id)
            .map_or(0, |index| self.entries[index].model as u8)
    }

    fn fluid_height(&self, id: u16) -> Option<u8> {
        let index = self.index(id)?;
        match self.entries[index].fluid_height {
            0 => None,
            raw => Some(raw),
        }
    }

    fn block_top_raw(&self, id: u16) -> Option<u8> {
        let index = self.index(id)?;
        match self.entries[index].block_top_raw {
            0 => None,
            raw => Some(raw),
        }
    }

    fn opaque(&self, id: u16) -> bool {
        self.index(id)
            .is_some_and(|index| self.entries[index].opaque)
    }
}

/// Destination the shared geometry core writes emitted records into.
///
/// The byte ABI lane packs each record into the caller's `u64` destination as
/// it goes; that write-as-you-go behavior is pinned by the existing ABI tests.
/// The typed lane stores checked contract records in the fixed scratch stage
/// and publishes only after the exact count is known. The associated error
/// keeps each lane's failure vocabulary: `MeshError::OutputOverflow` for the
/// raw destination and `KernelError` for the typed entry.
pub(crate) trait QuadStage {
    type Error;

    fn push(&mut self, quad: Quad) -> Result<(), Self::Error>;
}

/// Byte-lane stage: packs straight into the caller's `u64` destination.
/// Test-only since the ABI publishes from the staged typed stream; the
/// `greedy` unit suite keeps it as the direct-write vehicle.
#[cfg(test)]
struct PackedStage<'a> {
    output: &'a mut [u64],
    written: usize,
}

#[cfg(test)]
impl QuadStage for PackedStage<'_> {
    type Error = MeshError;

    fn push(&mut self, quad: Quad) -> Result<(), MeshError> {
        let index = self.written;
        let Some(slot) = self.output.get_mut(index) else {
            return Err(MeshError::OutputOverflow);
        };
        *slot = quad.pack();
        self.written += 1;
        Ok(())
    }
}

/// Typed stage: stores checked contract records in the caller's fixed staging
/// array.
///
/// Both a field-domain violation in one emission and exhaustion of the static
/// stage bound are invariant failures, not caller-capacity failures: the
/// conservative bound covers every structurally and semantically accepted
/// registry, so reaching past it means the geometry contract itself broke.
/// Either way no destination slot is published.
pub(crate) struct MeshStage<'a> {
    stage: &'a mut [MeshQuad],
    written: usize,
}

impl<'a> MeshStage<'a> {
    pub(crate) fn new(stage: &'a mut [MeshQuad]) -> Self {
        Self { stage, written: 0 }
    }

    /// Published prefix of the staged records.
    pub(crate) fn output(&self) -> &[MeshQuad] {
        &self.stage[..self.written]
    }
}

impl QuadStage for MeshStage<'_> {
    type Error = KernelError;

    fn push(&mut self, quad: Quad) -> Result<(), KernelError> {
        let index = self.written;
        let Some(slot) = self.stage.get_mut(index) else {
            return Err(KernelError::OutputInvariant);
        };
        *slot = quad.staged().map_err(|_| KernelError::OutputInvariant)?;
        self.written += 1;
        Ok(())
    }
}

#[derive(Copy, Clone, Default, Eq, PartialEq)]
struct MaskCell {
    used: bool,
    material: u16,
    ao: u8,
    light: u8,
    /// 该面所属方块是否是流体：为真时禁止贪心合并（见 mesh_section 的说明）。
    fluid: bool,
    /// 该面所属方块是否是非满格短方块（registry `block_top_raw` 非零）：
    /// 同样禁止贪心合并——常量角高度与流体共用 w/h 那 8 bit，且相邻格的
    /// 下沉上缘必须逐格保持，合并会抹成一张跨格平面。
    short: bool,
    /// 四个顶点的 4-bit 高度原值，顺序见 `Quad::corners`。
    corners: [u8; 4],
}

const FACES: [Face; 6] = [
    Face::NegX,
    Face::PosX,
    Face::NegY,
    Face::PosY,
    Face::NegZ,
    Face::PosZ,
];

/// 单个植物格产生的面实例数**固定上界**。
///
/// 两条对角线 × 正反两面 = 4，`plant-visual-presentation` 把「每格面实例数 MUST
/// 有固定上界」写成 MUST。4 小于普通方块的 6，所以整段的输出上界不变，Go 侧的
/// `maxNativeQuads = 6 * BlocksPerSection` 依旧覆盖最坏情况。
///
/// **上界由 `a_section_full_of_plants_stays_within_the_fixed_bound` 守**：它把整段
/// 塞满作物、断言总数恰好 `4 * 4096`，常量一旦被改大就变红。此前这里还有一条
/// 就地的 `assert_eq!(count - before, PLANT_QUADS_PER_CELL)`，但 `PLANT_QUADS` 的
/// 长度与本常量编译期绑定、循环每轮 `+1`，那条断言恒真、挡不住任何东西，已删。
const PLANT_QUADS_PER_CELL: usize = 4;

/// 植物格的四条 quad：对角线 A 正/背，对角线 B 正/背。次序固定，Go oracle 逐条对齐。
const PLANT_QUADS: [(Face, bool); PLANT_QUADS_PER_CELL] = [
    (Face::PlantDiagA, false),
    (Face::PlantDiagA, true),
    (Face::PlantDiagB, false),
    (Face::PlantDiagB, true),
];

/// Direct-write byte-lane entry: packs straight into the caller slice.
/// Test-only since the ABI publishes from the staged typed stream; the
/// `greedy` unit suite keeps it as the direct-write vehicle.
#[cfg(test)]
pub(crate) fn mesh_section(
    input: &MeshInput<'_>,
    light: &LightScratch<'_>,
    output: &mut [u64],
) -> Result<usize, MeshError> {
    if center_is_air(input) {
        return Ok(0);
    }

    let mut stage = PackedStage { output, written: 0 };
    mesh_geometry(input, light, &mut stage)?;
    Ok(stage.written)
}

/// Shared emission core for both section lanes.
///
/// Face, slice, row and cell order, the greedy merge rules, the plant cross
/// quads and the finite-model dispatch all live here once, so the typed lane
/// publishes exactly the stream the byte ABI lane packs for the same section.
pub(crate) fn mesh_geometry<A: MeshAccess, S: QuadStage>(
    access: &A,
    light: &LightScratch<'_>,
    stage: &mut S,
) -> Result<(), S::Error> {
    for face in FACES {
        let axis = (face as usize) >> 1;
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let step = if (face as u8) & 1 == 1 { 1 } else { -1 };

        for slice in 0..16 {
            let mut mask = [MaskCell::default(); 256];
            let mut any = false;
            for vi in 0..16 {
                for ui in 0..16 {
                    let mut p = [0; 3];
                    p[axis] = slice;
                    p[u] = ui;
                    p[v] = vi;
                    let id = access.block(p[0], p[1], p[2]);
                    let mut q = p;
                    q[axis] += step;
                    if !access.face_visible(id, access.block(q[0], q[1], q[2])) {
                        continue;
                    }
                    let Some(material) = access.material(id, face as usize) else {
                        continue;
                    };
                    // Axial faces of a finite-model block are exempt: torch
                    // forms 1..=5 and bed 6 publish their geometry through
                    // `mesh_models` alone. Plants reach the same exemption by
                    // having an all-zero visibility row; torch and bed rows may
                    // be nonzero (Go's `FaceVisible` answers true for a
                    // non-opaque neighbor), so the skip must be explicit here.
                    // Unknown tags from 7 up were already rejected by the
                    // registry validation, so they never reach this point.
                    if access.model(id) != 0 {
                        continue;
                    }
                    let fluid = access.fluid_height(id).is_some();
                    let top_raw = access.block_top_raw(id);
                    mask[(vi * 16 + ui) as usize] = MaskCell {
                        used: true,
                        material,
                        ao: compute_ao(access, p, axis, u, v, step),
                        light: light.at(q[0], q[1], q[2]),
                        fluid,
                        short: !fluid && top_raw.is_some(),
                        corners: if fluid {
                            fluid_corners(access, p, face, axis, u, v)
                        } else if let Some(raw) = top_raw {
                            short_block_corners(p, face, axis, u, v, raw)
                        } else {
                            [0; 4]
                        },
                    };
                    any = true;
                }
            }
            if !any {
                continue;
            }

            for vi in 0..16 {
                let mut ui = 0;
                while ui < 16 {
                    let cell = mask[vi * 16 + ui];
                    if !cell.used {
                        ui += 1;
                        continue;
                    }

                    // 流体与非满格短方块都按 1×1 出面，不贪心合并：两者的角
                    // 高度都借走了 w/h 那 8 bit（见 Quad::corners 的位布局），
                    // 合并后尺寸再也无法表达；而且短方块的下沉上缘是逐格属性，
                    // 合并会把相邻格抹成一张跨格平面。
                    let mut width = 1;
                    let mut height = 1;
                    if !cell.fluid && !cell.short {
                        while ui + width < 16 && mask[vi * 16 + ui + width] == cell {
                            width += 1;
                        }
                        'grow: while vi + height < 16 {
                            for offset in 0..width {
                                if mask[(vi + height) * 16 + ui + offset] != cell {
                                    break 'grow;
                                }
                            }
                            height += 1;
                        }
                    }
                    for dv in 0..height {
                        for du in 0..width {
                            mask[(vi + dv) * 16 + ui + du] = MaskCell::default();
                        }
                    }

                    let mut p = [0; 3];
                    p[axis] = slice;
                    p[u] = ui as i32;
                    p[v] = vi as i32;
                    stage.push(Quad {
                        x: p[0] as u8,
                        y: p[1] as u8,
                        z: p[2] as u8,
                        w: width as u8,
                        h: height as u8,
                        face,
                        material: cell.material,
                        ao: cell.ao,
                        light: cell.light,
                        corners: cell.corners,
                        back: false,
                    })?;
                    ui += width;
                }
            }
        }
    }

    mesh_plants(access, light, stage)?;
    mesh_models(access, light, stage)?;
    Ok(())
}

/// is_plant 报告一格是不是植物，判据是它的 material 落在植物区间。
///
/// 取 face 0 的 material 即可：植物六个面共用同一层（交叉斜面没有"朝向"），
/// Go 侧 `assets.Registry.Material` 对作物的全部 face 返回同一个 `LayerWheatN`。
fn is_plant<A: MeshAccess>(access: &A, id: u16) -> bool {
    matches!(access.material(id, 0), Some(material) if plant_material(material))
}

/// mesh_plants emits the cross quads for every plant cell in the section.
///
/// 植物格的六个轴向面已经被上面的循环挡掉了——出面规则的唯一真值源是 Go 的
/// `assets.Registry.FaceVisible`，它对作物一律返回 false，烘焙进可见性位图后
/// Rust 这边连 mask 都不会写进去。于是这里是植物几何的**全部**来源。
///
/// 三条规则：
///
///  - **每格恰好 4 条**（两条对角线 × 正反两面），`w = h = 1`、不参与贪心合并。
///    合并后 `w`/`h` 那 8 bit 已被正背标志占用，尺寸再也表达不出来；而且交叉斜面
///    的形状是逐格的，合并等于把相邻两株抹成一片更大的斜板。
///  - **光照取正上方相邻格 `(x, y+1, z)`**。交叉斜面长在方块内部，没有"相邻空气面"
///    可采——轴向面采的是 `p + step` 那一格，植物根本不出轴向面。取上方格既是
///    `plant-visual-presentation` 的明文规则，也让露天作物直接拿到满天空光 15：
///    植物自身那格在列顶图里就是列顶，`sky_light` 判的是"严格高于列顶"，因此
///    该格自身恒为 0，只有它上方那格才是 15。
///  - **AO 记满**（`0xff`，四角均无遮蔽）。AO 的计算前提是"面贴在方块表面、四周
///    有共面邻居"，交叉斜面两者都不满足，硬算只会得到与视角无关的脏阴影。
///
/// 枚举次序是 `y → z → x`（与区段方块的存储次序一致），保证输出确定。
fn mesh_plants<A: MeshAccess, S: QuadStage>(
    access: &A,
    light: &LightScratch<'_>,
    stage: &mut S,
) -> Result<(), S::Error> {
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                let id = access.block(x, y, z);
                // 空气早退：绝大多数格是空气，先挡掉能省下一次 registry 二分。
                if id == access.air_id() || !is_plant(access, id) {
                    continue;
                }
                let Some(material) = access.material(id, 0) else {
                    continue;
                };
                let light_above = light.at(x, y + 1, z);
                for (face, back) in PLANT_QUADS {
                    stage.push(Quad {
                        x: x as u8,
                        y: y as u8,
                        z: z as u8,
                        w: 1,
                        h: 1,
                        face,
                        material,
                        ao: 0xff,
                        light: light_above,
                        corners: [0; 4],
                        back,
                    })?;
                }
            }
        }
    }
    Ok(())
}

/// cell_height 返回一格流体的 4-bit 高度原值，非流体返回 `None`。
///
/// 规则（design D2，全整数、无浮点）：
///
/// - 上方也是流体 → 取满格 `FULL_FLUID_HEIGHT`（15），使水柱内部无斜面、与上格无缝；
/// - 否则取 registry 里烘焙好的 `h_raw`（Go 侧 `14 - level`，源 14、最弱 7）。
fn cell_height<A: MeshAccess>(access: &A, x: i32, y: i32, z: i32) -> Option<u8> {
    let raw = access.fluid_height(access.block(x, y, z))?;
    if access.fluid_height(access.block(x, y + 1, z)).is_some() {
        return Some(FULL_FLUID_HEIGHT);
    }
    Some(raw)
}

/// corner_height 返回顶点格 `(vx, vz)` 在第 `y` 层上的 4-bit 角高度。
///
/// 一个顶点被四列共享：`(vx-1,vz-1) (vx,vz-1) (vx-1,vz) (vx,vz)`。角高度取其中
/// **流体格** `h_raw` 的整数平均（向下取整）；四格中任一格上方也是流体则直接取满格
/// 15。整除是唯一的算术，**不引入任何浮点**（spec：呈现不得引入浮点不确定性）。
///
/// 因为结果只由顶点坐标决定，两个水平相邻的流体格在共享边上必然读出同一个值，
/// 斜面于是天然连续。四格全非流体时返回 0，调用方只在流体格上调用它，不会命中。
fn corner_height<A: MeshAccess>(access: &A, vx: i32, y: i32, vz: i32) -> u8 {
    let mut sum = 0_u32;
    let mut count = 0_u32;
    for (dx, dz) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
        let Some(height) = cell_height(access, vx + dx, y, vz + dz) else {
            continue;
        };
        if height == FULL_FLUID_HEIGHT {
            return FULL_FLUID_HEIGHT;
        }
        sum += u32::from(height);
        count += 1;
    }
    if count == 0 {
        return 0;
    }
    (sum / count) as u8
}

/// fluid_corners 求一个流体面四个顶点的高度原值，顺序见 `Quad::corners`。
///
/// 只有落在该格顶面那一层（世界 y == `p[1] + 1`）的顶点才带高度；侧面的两个下顶点
/// 与底面的四个顶点都在方块底面，语义上高度为 0。顶面四角全部带高度，底面全 0
/// 因而按普通 1×1 quad 打包——它本来就是平的，没有可插值的东西。
fn fluid_corners<A: MeshAccess>(
    access: &A,
    p: [i32; 3],
    face: Face,
    axis: usize,
    u: usize,
    v: usize,
) -> [u8; 4] {
    let mut corners = [0; 4];
    for (index, [du, dv]) in [[-1, -1], [1, -1], [1, 1], [-1, 1]].into_iter().enumerate() {
        let mut vertex = p;
        // 正方向面贴在方块的 +1 边，负方向面贴在 0 边。
        vertex[axis] += i32::from((face as u8) & 1);
        vertex[u] += (du + 1) / 2;
        vertex[v] += (dv + 1) / 2;
        if vertex[1] == p[1] + 1 {
            corners[index] = corner_height(access, vertex[0], p[1], vertex[2]);
        }
    }
    corners
}

/// short_block_corners 求一个非满格短方块的面四个顶点的高度原值，顺序见
/// `Quad::corners`。
///
/// 形状与 `fluid_corners` 一致：只有落在该格顶层（世界 y == `p[1] + 1`）的顶点
/// 才带高度——于是顶面四角全下沉、四个侧面上缘两角下沉、底面四角为 0，
/// 与短方块的碰撞盒（底面整格、顶面 (h_raw+1)/16）逐面一致。
///
/// 与流体的差异：不走 `corner_height` 邻域平均，直接采用 registry 常量
/// `raw`。短方块是刚体方块，相邻同类格的高度由 registry 保证恒等，不存在
/// 需要插值的斜面；这也天然规避了流体规则里「上方也是流体则取满格 15」的
/// 污染——贴着上方方块的耕地不会被错误拉平。
fn short_block_corners(
    p: [i32; 3],
    face: Face,
    axis: usize,
    u: usize,
    v: usize,
    raw: u8,
) -> [u8; 4] {
    let mut corners = [0; 4];
    for (index, [du, dv]) in [[-1, -1], [1, -1], [1, 1], [-1, 1]].into_iter().enumerate() {
        let mut vertex = p;
        // 正方向面贴在方块的 +1 边，负方向面贴在 0 边。
        vertex[axis] += i32::from((face as u8) & 1);
        vertex[u] += (du + 1) / 2;
        vertex[v] += (dv + 1) / 2;
        if vertex[1] == p[1] + 1 {
            corners[index] = raw;
        }
    }
    corners
}

pub(crate) fn center_is_air(input: &MeshInput<'_>) -> bool {
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                if input.block(x, y, z) != input.air_id {
                    return false;
                }
            }
        }
    }
    true
}

fn compute_ao<A: MeshAccess>(
    access: &A,
    p: [i32; 3],
    axis: usize,
    u: usize,
    v: usize,
    step: i32,
) -> u8 {
    let mut base = p;
    base[axis] += step;
    let solid = |du: i32, dv: i32| {
        let mut q = base;
        q[u] += du;
        q[v] += dv;
        u8::from(access.opaque(access.block(q[0], q[1], q[2])))
    };

    let mut ao = 0;
    for (index, [du, dv]) in [[-1, -1], [1, -1], [1, 1], [-1, 1]].into_iter().enumerate() {
        let side_u = solid(du, 0);
        let side_v = solid(0, dv);
        let level = if side_u == 1 && side_v == 1 {
            0
        } else {
            3 - side_u - side_v - solid(du, dv)
        };
        ao |= level << (index * 2);
    }
    ao
}

#[cfg(test)]
mod farmland_top_tests;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod plant_tests;
#[cfg(test)]
mod section_boundary_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod torch_tests;
#[cfg(test)]
mod water_corner_tests;
