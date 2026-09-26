//! 有限模型几何发射（model dispatcher 的发射半边）：本文件承载 `mesh_models`
//! 调度器与火把（placeable-torches）两种形态的几何；床的半高板几何见
//! `super::bed`。
//!
//! 五种火把形态共用一张竖直火柄纹理（材质 alpha 收窄视觉），mesher 只负责发射
//! 「形态几何」：
//!
//! - **落地（model 1）**：与植物完全同构的两条交叉斜面（face 6/7 × 正背各
//!   一）——这是 8 字节 quad 格式里唯一「格内居中竖直」的表达，两条对角面
//!   都过格心的竖直线，任何水平视角下恰好各留一条。
//! - **墙面（model 2..5）**：三片 quad——承载倾斜的两片轴向薄板 + 贴住支撑
//!   面的一片平帽。薄板顶缘用角高度表达「向远离支撑方向倾斜」（支撑侧
//!   9/16、远离侧 14/16），帽面落在支撑平面上表达「贴近支撑面」。角高度走
//!   与流体/短方块相同的位通道（bit 12..19/55..62，恒 1×1 不贪心合并），
//!   terrain 着色器按材质门控解码；今日渲染里火柄因世界坐标锁定 UV 保持
//!   竖直、倾斜体现在薄板的斜顶边。
//!
//! 与植物相同的口径：光照取正上方相邻格、AO 记满（格内几何没有共面邻居可
//! 采，硬算只会得到与视角无关的脏阴影）；不参与贪心合并；枚举次序
//! `y → z → x` 保证输出确定。

use crate::light::LightScratch;
use crate::quad::{Face, Quad};

use super::bed::emit_bed;
use super::{MeshAccess, PLANT_QUADS, QuadStage};

/// 落地火把每格面实例数的**固定上界**：两条交叉斜面 × 正背各一。
///
/// 4 等于植物的上界、小于普通方块的 6，整段输出上界不变，Go 侧
/// `maxNativeQuads = 6 * BlocksPerSection` 依旧覆盖最坏情况。数值由
/// `torch_tests` 的固定数量断言钉住（发射函数的循环结构本身只产出这个数），
/// 因此常量只在测试构建里存在。
#[cfg(test)]
pub(crate) const TORCH_QUADS_PER_STANDING_CELL: usize = 4;
/// 墙面火把每格面实例数的**固定上界**：两片倾斜薄板 + 一片贴面帽。
///
/// 同上：3 小于普通方块的 6，由 `torch_tests` 钉住，仅测试构建存在。
#[cfg(test)]
pub(crate) const TORCH_QUADS_PER_WALL_CELL: usize = 3;
/// 墙面火把**支撑侧**顶缘的 4-bit 角高度原值：呈现高度 (8+1)/16 = 9/16。
///
/// 支撑侧低、远离侧高，两角高度差即「向远离支撑方向倾斜」的倾斜度。
pub(crate) const TORCH_WALL_TOP_NEAR_RAW: u8 = 8;
/// 墙面火把**远离支撑侧**顶缘的 4-bit 角高度原值：呈现高度 (13+1)/16 = 14/16。
///
/// 取 13 而不是 14：角高度 14 是耕地顶面的生产取值，避开同一数值在不同语义
/// 间误传；也留出与满格 15（流体水柱内部专用）的余量。
pub(crate) const TORCH_WALL_TOP_FAR_RAW: u8 = 13;

/// mesh_models emits the model geometry for every cell carrying a finite
/// model tag. This is the emission half of the model dispatcher: tag 0
/// (default) never enters this function and keeps the existing geometry;
/// tags 1..=5 dispatch to the torch forms and tag 6 to the bed (see
/// `super::bed`); unknown values from 7 up are already rejected while the
/// registry is parsed and validated, so they never arrive here. The closed
/// range `match` therefore stays exhaustive, and a future tag is forced to be
/// handled explicitly instead of silently falling back.
///
/// Emission shares the `y → z → x` walk and the registry-only dispatch with
/// the byte lane's packing path, so both lanes publish the same stream.
pub(crate) fn mesh_models<A: MeshAccess, S: QuadStage>(
    access: &A,
    light: &LightScratch<'_>,
    stage: &mut S,
) -> Result<(), S::Error> {
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                let id = access.block(x, y, z);
                // 空气早退：绝大多数格是空气，先挡掉能省下一次 registry 二分。
                if id == access.air_id() {
                    continue;
                }
                let tag = access.model(id);
                if tag == 0 {
                    continue;
                }
                let Some(material) = access.material(id, 0) else {
                    continue;
                };
                let light_above = light.at(x, y + 1, z);
                match tag {
                    1 => emit_standing([x, y, z], material, light_above, stage)?,
                    2..=5 => emit_wall(tag, [x, y, z], material, light_above, stage)?,
                    6 => emit_bed(access, light, [x, y, z], stage)?,
                    // Registry validation already rejected every unknown tag
                    // from 7 up, so this arm exists for exhaustiveness only.
                    _ => unreachable!("unknown model tags are rejected by registry validation"),
                }
            }
        }
    }
    Ok(())
}

/// emit_standing 发射落地形态：两条交叉斜面 × 正背各一，复用植物的
/// `PLANT_QUADS` 编组（face 6/7 + 正背位）。
///
/// `cell` 是火把格坐标 `[x, y, z]`——与 `compute_ao`/`fluid_corners` 的坐标
/// 参数式样一致，三个分量合并传递也压住 clippy 的 too_many_arguments 上限。
fn emit_standing<S: QuadStage>(
    cell: [i32; 3],
    material: u16,
    light_above: u8,
    stage: &mut S,
) -> Result<(), S::Error> {
    for (face, back) in PLANT_QUADS {
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
            corners: [0; 4],
            back,
        })?;
    }
    Ok(())
}

/// emit_wall 发射墙面形态：两片倾斜薄板 + 一片贴面帽，次序固定、Go 侧逐条
/// 对齐。
///
/// 角顺序与 `compute_ao` 一致：局部 (u,v) 的 (0,0)(1,0)(1,1)(0,1)。薄板的
/// u 轴是倾斜方向（±X 墙 → Z 法线面上 u=x；±Z 墙 → X 法线面上 v=z），顶缘
/// 两个角分别取 near/far、底缘两角恒 0（贴地）。`cell` 是火把格坐标
/// `[x, y, z]`（坐标分量合并传递的式样见 `emit_standing`）。
fn emit_wall<S: QuadStage>(
    tag: u8,
    cell: [i32; 3],
    material: u16,
    light_above: u8,
    stage: &mut S,
) -> Result<(), S::Error> {
    let near = TORCH_WALL_TOP_NEAR_RAW;
    let far = TORCH_WALL_TOP_FAR_RAW;
    // (倾斜薄板的 face 对, 贴面帽 face, 薄板四角)。墙面形态名 = 命中面名，
    // 支撑格在 face.Opposite() 方向：wall +X 的支撑在 −X 侧（近侧 x+0 是
    // 角 3、远侧 x+1 是角 2），其余同理镜像。
    let (plates, cap, corners) = match tag {
        2 => ([Face::NegZ, Face::PosZ], Face::NegX, [0, 0, far, near]),
        3 => ([Face::NegZ, Face::PosZ], Face::PosX, [0, 0, near, far]),
        4 => ([Face::NegX, Face::PosX], Face::NegZ, [0, near, far, 0]),
        _ => ([Face::NegX, Face::PosX], Face::PosZ, [0, far, near, 0]),
    };
    for face in plates {
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
        face: cap,
        material,
        ao: 0xff,
        light: light_above,
        corners: [0; 4],
        back: false,
    })
}
