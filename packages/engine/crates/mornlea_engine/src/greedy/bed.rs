//! 床的有限模型（bed-and-sleep）几何发射：保留 model tag 6 的单值半高板。
//!
//! 床尾与床头 × 四个朝向共八个方块形态共用同一张几何——9/16 半高板（四面
//! 侧板 + 平顶），与 `physics` 的床碰撞体（`bedCollisionHeight` = 9/16）同线；
//! 朝向差异不进几何，由 Go 侧逐形态的床面材质层表达（枕头/毯沿亮带随朝向
//! 旋转）。角高度用 4-bit 原值 8（呈现高度 (8+1)/16 = 9/16），落在「顶面顶点
//! 7..=15」的合法域内且避开水柱满格 15 与耕地顶面 14 的专用取值。
//!
//! 与火把相同的口径：光照取正上方相邻格、AO 记满（格内几何没有共面邻居可
//! 采）；不参与贪心合并（tag 路径天然 1×1）；**不出底面**——床必须贴支撑
//! 放置，底面与支撑块顶面共面，无条件发射只会产生 z-fighting。每格固定
//! 5 条 quad（≤ 普通方块的 6），整段输出上界不变。
//!
//! 枚举次序 `y → z → x` 与每格内「四侧板自 −X/−X/−Z/+Z、末尾平顶」的发射
//! 次序固定，Go 侧 `TestNativeMeshBedQuadsRoundTripThroughGoUnpack` 逐条对齐。

use crate::light::LightScratch;
use crate::quad::{Face, Quad};

use super::{MeshAccess, QuadStage};

/// 床侧板/平顶顶缘的 4-bit 角高度原值：呈现高度 (8+1)/16 = 9/16，与床碰撞
/// 体同线。取 8 而非 7：9/16 是 spec 写死的半高契约值，不是「任意半格」。
pub(crate) const BED_TOP_RAW: u8 = 8;

/// 床每格面实例数的**固定上界**：四片侧板 + 一片平顶。
///
/// 5 小于普通方块的 6，整段输出上界不变，Go 侧 `maxNativeQuads` 依旧覆盖
/// 最坏情况。数值由本模块的固定数量断言钉住，因此常量只在测试构建里存在。
#[cfg(test)]
pub(crate) const BED_QUADS_PER_CELL: usize = 5;

/// emit_bed 发射单格床几何（tag 6 的发射半边，由 model dispatcher 对每个床
/// 格调用一次）：四片半高侧板 + 一片平顶，次序固定、Go 侧逐条对齐（−X、+X、
/// −Z、+Z、顶）。
///
/// 角顺序与 `compute_ao` 一致：局部 (u,v) 的 (0,0)(1,0)(1,1)(0,1)。±Z 面的
/// u 轴是 x、v 轴是纵向 y；±X 面的 u 轴是纵向 y、v 轴是 z——侧板顶缘四角
/// 因此分别落在 (0,1)(1,1) 或 (1,0)(1,1) 两角。平顶四角全部取 `BED_TOP_RAW`。
/// `cell` 是床格坐标 `[x, y, z]`；光照取床顶上方格（与火把同一口径），AO
/// 记满。
///
/// 材质路由按 `Face` 枚举序取面：0..5 = −X/+X/−Y/+Y/−Z/+Z。平顶 quad 读
/// face 3（PosY）——生产注册表（`assets.Registry.Material`）在此挂逐形态床
/// 面层（枕头/毯沿亮带随朝向旋转），是「多朝向床形态可辨」的呈现载体；四
/// 片侧板各读自身面材质，生产注册表恒为橡木木板层、表达橡木床架。五条
/// quad 不得共用单一 face 材质：糊满任何一面都会让床整体退成单一木色、八
/// 个朝向形态不可辨。任一材质缺失即整格跳过：五条面是一个完整形状，缺面
/// 发射只会得到残盒。
pub(crate) fn emit_bed<A: MeshAccess, S: QuadStage>(
    access: &A,
    light: &LightScratch<'_>,
    cell: [i32; 3],
    stage: &mut S,
) -> Result<(), S::Error> {
    let id = access.block(cell[0], cell[1], cell[2]);
    let Some(mat_top) = access.material(id, 3) else {
        return Ok(());
    };
    let Some(mat_neg_x) = access.material(id, 0) else {
        return Ok(());
    };
    let Some(mat_pos_x) = access.material(id, 1) else {
        return Ok(());
    };
    let Some(mat_neg_z) = access.material(id, 4) else {
        return Ok(());
    };
    let Some(mat_pos_z) = access.material(id, 5) else {
        return Ok(());
    };
    let light_above = light.at(cell[0], cell[1] + 1, cell[2]);
    let top_raw = BED_TOP_RAW;
    let sides: [(Face, u16, [u8; 4]); 4] = [
        (Face::NegX, mat_neg_x, [0, top_raw, top_raw, 0]),
        (Face::PosX, mat_pos_x, [0, top_raw, top_raw, 0]),
        (Face::NegZ, mat_neg_z, [0, 0, top_raw, top_raw]),
        (Face::PosZ, mat_pos_z, [0, 0, top_raw, top_raw]),
    ];
    for (face, material, corners) in sides {
        stage.push(Quad {
            x: cell[0] as u8,
            y: cell[1] as u8,
            z: cell[2] as u8,
            w: 1,
            h: 1,
            face,
            material,
            ao: 0xff,
            light: light_above,
            corners,
            back: false,
        })?;
    }
    stage.push(Quad {
        x: cell[0] as u8,
        y: cell[1] as u8,
        z: cell[2] as u8,
        w: 1,
        h: 1,
        face: Face::PosY,
        material: mat_top,
        ao: 0xff,
        light: light_above,
        corners: [top_raw; 4],
        back: false,
    })
}

#[cfg(test)]
mod tests {
    use super::{BED_QUADS_PER_CELL, BED_TOP_RAW, emit_bed};
    use crate::input::MeshInput;
    use crate::light::LIGHT_VOLUME;
    use crate::light::LightScratch;
    use crate::quad::{Face, Quad};

    use super::super::test_support::{ENTRY_BYTES, REGISTRY_OFFSET};

    const ENTRIES: usize = 3;
    const BED_ID: u16 = 76;
    /// 夹具刻意非同质：顶面（face 3=PosY）材质与四个侧面材质取不同值——
    /// 与生产注册表「顶面=逐形态床面层、侧面=橡木板层」的路由同构。若
    /// emit_bed 把五条 quad 糊成单一 face 材质，本夹具的材质断言立即变红；
    /// 六面同材质的夹具对这类路由缺陷是盲的。
    const BED_TOP_MATERIAL: u16 = 700;
    const BED_SIDE_MATERIAL: u16 = 91;

    /// bed_input 造一份含单条床条目（model 6）的合法 mesh 输入：条目 0=空气、
    /// 1=屏障（opaque）、2=床（非 opaque、model 6，顶面/侧面异材质）。布局
    /// 常量复用 test_support，避免手写偏移漂移。
    fn bed_input() -> MeshInput<'static> {
        let words_per_row = 1;
        let mut bytes =
            vec![0; REGISTRY_OFFSET + ENTRIES * ENTRY_BYTES + ENTRIES * words_per_row * 8];
        bytes[0..4].copy_from_slice(b"MGM1");
        bytes[8..10].copy_from_slice(&(ENTRIES as u16).to_le_bytes());
        bytes[10..12].copy_from_slice(&(words_per_row as u16).to_le_bytes());
        bytes[12..14].copy_from_slice(&0_u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&1_u16.to_le_bytes());
        let mut write_entry = |index: usize, id: u16, opaque: bool, model: u8| {
            let entry = REGISTRY_OFFSET + index * ENTRY_BYTES;
            bytes[entry..entry + 2].copy_from_slice(&id.to_le_bytes());
            bytes[entry + 2] = u8::from(opaque);
            for face in 0..6 {
                let material = if face == 3 {
                    BED_TOP_MATERIAL
                } else {
                    BED_SIDE_MATERIAL
                };
                bytes[entry + 4 + face * 2..entry + 6 + face * 2]
                    .copy_from_slice(&material.to_le_bytes());
            }
            bytes[entry + 19] = model;
        };
        write_entry(0, 0, false, 0);
        write_entry(1, 1, true, 0);
        write_entry(2, BED_ID, false, 6);
        // 中心区段的格 (8,8,8) 写成床：emit_bed 从方块矩阵反查该格的材质。
        let cell = 16 + (13 * 4096 + ((8 << 8) | (8 << 4) | 8)) * 2;
        bytes[cell..cell + 2].copy_from_slice(&BED_ID.to_le_bytes());
        MeshInput::parse(Box::leak(bytes.into_boxed_slice())).unwrap()
    }

    /// 单格床：恰好 5 条 quad，四侧板 + 平顶，角高度全部落在 9/16，1×1 不
    /// 合并、无正背；平顶材质取 face 3（PosY）的床面层、侧板各取自身面的
    /// 材质——路由接错任何一面都会红。次序与 Go 侧 FFI 往返用例逐条对齐；
    /// 写错任何一角同样会红。
    #[test]
    fn bed_cell_emits_half_height_slab_quads() {
        let input = bed_input();
        let light = LightScratch::new(
            Box::leak(vec![0; LIGHT_VOLUME].into_boxed_slice()),
            Box::leak(vec![0; LIGHT_VOLUME].into_boxed_slice()),
        );
        let mut output = vec![0_u64; 6 * 4096];
        let count = {
            // The byte-lane stage keeps this unit test on the same packing path
            // the ABI entry uses; the typed lane is covered by the topic tests.
            let mut stage = super::super::PackedStage {
                output: &mut output,
                written: 0,
            };
            emit_bed(&input, &light, [8, 8, 8], &mut stage).unwrap();
            stage.written
        };
        assert_eq!(
            count, BED_QUADS_PER_CELL,
            "单格床必须恰好发射 {BED_QUADS_PER_CELL} 条面实例"
        );

        let quads: Vec<Quad> = output[..count].iter().copied().map(Quad::unpack).collect();
        let top = BED_TOP_RAW;
        let want = [
            (Face::NegX, BED_SIDE_MATERIAL, [0, top, top, 0]),
            (Face::PosX, BED_SIDE_MATERIAL, [0, top, top, 0]),
            (Face::NegZ, BED_SIDE_MATERIAL, [0, 0, top, top]),
            (Face::PosZ, BED_SIDE_MATERIAL, [0, 0, top, top]),
            (Face::PosY, BED_TOP_MATERIAL, [top, top, top, top]),
        ];
        for (i, (quad, (face, material, corners))) in quads.iter().zip(want.iter()).enumerate() {
            assert_eq!(quad.face, *face, "床 quad[{i}] 面次序错误");
            assert_eq!(quad.corners, *corners, "床 quad[{i}] 角高度错误");
            assert_eq!((quad.w, quad.h), (1, 1), "床 quad 必须是 1×1");
            assert!(!quad.back, "床 quad 不带正背位");
            assert_eq!(quad.material, *material, "床 quad[{i}] 材质路由错误");
            assert_eq!(quad.ao, 0xff, "格内几何无共面邻居，AO 记满");
            assert_eq!((quad.x, quad.y, quad.z), (8, 8, 8), "坐标必须全在本格");
        }
    }
}
