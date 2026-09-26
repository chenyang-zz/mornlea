//! fluid_rescan:流体重扫扫描内核(确定性、无状态)。
//!
//! 逐条镜像 Go `internal/sim/realm/environment.go` 的重扫记账契约
//! (`enqueueChunkFluids` 及其两个不动点判定):队列的 `Enqueue` 在 kernel
//! 里变成输出坐标流,平面编排、预算分摊与游标推进留在 Go 侧。被扫描的
//! 区块是盒中心区块(Go 侧五段平面各自的「当前 chunk」),裙边与元数据
//! 描述它的 3×3 邻域。
//!
//! 记账三档(与 Go 一致):均匀非流体区段计 1;均匀水源区段且区段级
//! 不动点成立(下方区段 + 四个水平邻区段全部「均匀且不可替换」)计 1;
//! 其余区段逐格记账,扫描范围内每格计 1。区段循环前先查额度,进入一个
//! 区段后必须整段完成,单次调用至多超支一个区段。水源格额外过五邻
//! 不动点(下方 + 四个水平邻格全部不可替换),成立则不产出;流动水
//! 一律产出。
//!
//! 输入布局 MFL1 v1(header + 中心区块 + 裙边 + 元数据):
//! - header 26 字节:u32 layout_version=1 | i32 center_chunk_x |
//!   i32 center_chunk_z | u16 x0 | u16 x1 | u16 z0 | u16 z1(盒内局部列
//!   0..17,含裙边)| u8 start_section(0..23)| u8 reserved=0 | u32 budget;
//! - 中心区块 24 区段记录(按 y 区段 0..23):u8 kind(0=均匀、1=密集)|
//!   u8 pad=0;kind=0 追加 u16 uniform_id(记录共 4 字节),kind=1 追加
//!   4096×u16 LE(区段内序 x + z*16 + y16*256,与 Go `world.blockIndex`
//!   一致);
//! - 裙边 68 列 × 384 u16(列序固定:(x=-1,z=0..15)、(x=16,z=0..15)、
//!   (z=-1,x=0..15)、(z=16,x=0..15)、四角 (-1,-1)/(16,-1)/(-1,16)/(16,16);
//!   列内 y 0..383);
//! - 元数据 9 区块 × 24 区段 × 3B(u8 uniform_flag + u16 id,flag=0 时
//!   id=0):区块序 中心、(-1,-1)、(0,-1)、(1,-1)、(-1,0)、(1,0)、
//!   (-1,1)、(0,1)、(1,1)。
//!
//! 盒内局部坐标:中心区块局部 (lx,lz) ∈ 0..15 映射盒 (lx+1, lz+1);y
//! 全高 0..383 是世界高度指标,对应世界 y_base + 0..383(y_base =
//! `core.MinY` = −64,由 Go 编码方保证,亦是输出世界 y 的换算基数)。
//!
//! 未就绪邻块的编码约定由 Go 侧承担:元数据表记 flag=1(均匀)+ id=Barrier,
//! 对应裙边列整列填 Barrier——kernel 不感知就绪状态,Barrier 的「实心不可
//! 替换」语义由此进入区段级与五邻不动点判定,镜像 Go 对未就绪读 Barrier
//! 的行为。扫描区域域宽含裙边列(0 与 17),但生产调用方只扫描中心列
//! 1..=16:触及最外裙边列的区域一旦在该列遇到水源,五邻不动点读会越出盒
//! 外,在 `skirt_column` 以 unreachable panic 拒绝——该路径在生产不可达,
//! 布局不收紧域宽只是保留防御性解析空间。
//!
//! 输出布局:流体格世界坐标流(每条 12 字节:u32 x、u32 y、u32 z LE;
//! 世界坐标可为负,按二进制补码编码,Go 侧以 int32 重读)+ 尾部 summary
//! 8 字节(u32 spent | u8 done | u8[3] pad)。done 表示从 start_section
//! 起的扫描范围在预算内全部完成;spent 是本次记账总数。

use crate::fluid_eval::{BARRIER, WATER_SOURCE, is_fluid, replaceable};
use crate::native::contracts::KernelError;
use crate::worldgen::{read_i32, read_u32};

/// 输入布局版本:header `layout_version` 字段的唯一合法值。
pub(crate) const RESCAN_LAYOUT_VERSION: u32 = 1;
/// 输入头部长度(字段和钉位:4+4+4+2+2+2+2+1+1+4)。
pub(crate) const RESCAN_HEADER_BYTES: usize = 26;
/// 每区块区段数,镜像 `core.SectionsPerChunk`。
const SECTIONS_PER_CHUNK: usize = 24;
/// 区段边长,镜像 `core.SectionSize`。
const SECTION_SIZE: usize = 16;
/// 盒内每轴列数:中心 16 列 + 两侧裙边各 1 列。
const BOX_COLUMNS: usize = SECTION_SIZE + 2;
/// 世界全高(方块数)= 24 区段 × 16,即裙边列长。
const WORLD_HEIGHT: usize = SECTIONS_PER_CHUNK * SECTION_SIZE;
/// 世界最低方块 y,镜像 `core.MinY`:盒内 y 指标 0..383 对应世界
/// y = WORLD_MIN_Y + 指标。
const WORLD_MIN_Y: i32 = -64;
/// 裙边列数:四边各 16 列 + 四角 4 列。
const SKIRT_COLUMNS: usize = 4 * SECTION_SIZE + 4;
/// 裙边单列字节数:全高 384 个 u16。
const SKIRT_COLUMN_BYTES: usize = WORLD_HEIGHT * 2;
/// 元数据表覆盖的区块数:3×3 邻域。
const METADATA_CHUNKS: usize = 9;
/// 元数据表字节数:9 区块 × 24 区段 × 3B。
const METADATA_BYTES: usize = METADATA_CHUNKS * SECTIONS_PER_CHUNK * 3;
/// 均匀区段记录字节数:kind + pad + u16 uniform_id。
const UNIFORM_SECTION_BYTES: usize = 4;
/// 密集区段记录字节数:kind + pad + 4096×u16。
const DENSE_SECTION_BYTES: usize = 2 + SECTION_SIZE * SECTION_SIZE * SECTION_SIZE * 2;
/// 区段记录 kind:均匀。
const SECTION_KIND_UNIFORM: u8 = 0;
/// 区段记录 kind:密集。
const SECTION_KIND_DENSE: u8 = 1;
/// 元数据 uniform_flag:非均匀(id 必须为 0)。
const METADATA_NON_UNIFORM: u8 = 0;
/// 单条输出坐标字节数:u32 x、u32 y、u32 z(LE)。
pub(crate) const RESCAN_POSITION_BYTES: usize = 12;
/// 输出尾部 summary 字节数:u32 spent + u8 done + u8[3] pad。
pub(crate) const RESCAN_SUMMARY_BYTES: usize = 8;

/// 水源密封判定的五个邻格偏移:下方 + 四个水平方向,与 Go
/// `fluidSealedSourceOffsets` 同序(结果与顺序无关,保序只为可读)。
const SEALED_OFFSETS: [(i32, i32, i32); 5] =
    [(0, -1, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)];

/// 解析后的输入视图:全部访问只读原始字节,不复制区段数据。
pub(crate) struct RescanView<'a> {
    bytes: &'a [u8],
    /// 每区段记录的起始偏移。
    section_offsets: [usize; SECTIONS_PER_CHUNK],
    /// 均匀区段的 uniform_id;密集区段为 `None`。
    section_uniform: [Option<u16>; SECTIONS_PER_CHUNK],
    skirt_offset: usize,
    metadata_offset: usize,
    center_x: i32,
    center_z: i32,
    x0: usize,
    x1: usize,
    z0: usize,
    z1: usize,
    start_section: usize,
    budget: u32,
}

/// 元数据表的 (dx,dz) → 区块序映射:中心、(-1,-1)、(0,-1)、(1,-1)、
/// (-1,0)、(1,0)、(-1,1)、(0,1)、(1,1)。只被内部固定方向调用。
pub(crate) fn chunk_meta_index(dx: i32, dz: i32) -> usize {
    match (dx, dz) {
        (0, 0) => 0,
        (-1, -1) => 1,
        (0, -1) => 2,
        (1, -1) => 3,
        (-1, 0) => 4,
        (1, 0) => 5,
        (-1, 1) => 6,
        (0, 1) => 7,
        (1, 1) => 8,
        _ => unreachable!("元数据表只覆盖 3×3 邻域的固定方向"),
    }
}

/// 盒内局部列 (bx,bz) → 裙边列序:四边主序 (x=-1,z=0..15)、(x=16,
/// z=0..15)、(z=-1,x=0..15)、(z=16,x=0..15),随后四角 (-1,-1)/(16,-1)/
/// (-1,16)/(16,16)。角列只是布局定长的一部分:五邻偏移无对角,重扫
/// 永远不会读角列。
pub(crate) fn skirt_column(bx: i32, bz: i32) -> usize {
    match (bx, bz) {
        (0, 1..=16) => (bz - 1) as usize,
        (17, 1..=16) => SECTION_SIZE + (bz - 1) as usize,
        (1..=16, 0) => 2 * SECTION_SIZE + (bx - 1) as usize,
        (1..=16, 17) => 3 * SECTION_SIZE + (bx - 1) as usize,
        (0, 0) => 4 * SECTION_SIZE,
        (17, 0) => 4 * SECTION_SIZE + 1,
        (0, 17) => 4 * SECTION_SIZE + 2,
        (17, 17) => 4 * SECTION_SIZE + 3,
        _ => unreachable!("盒内局部列必须在 0..=17"),
    }
}

/// Dense section cell index for one box cell: `x + z*16 + y16*256` with x
/// fastest, the section-record order the MFL1 wire layout pins. Kept beside
/// `skirt_column` so every reader shares one index rule.
pub(crate) fn section_cell_index(bx: i32, y: i32, bz: i32) -> usize {
    (bx - 1) as usize
        + (bz - 1) as usize * SECTION_SIZE
        + y as usize % SECTION_SIZE * SECTION_SIZE * SECTION_SIZE
}

/// Box cell to world position: x/z use the center chunk's `×16` base (box
/// column 1 is local 0) and y uses `core.MinY`. The legacy byte encoder and
/// the typed provider both route through here so their coordinates cannot
/// drift apart.
pub(crate) fn box_to_world(center: [i32; 2], bx: i32, y: i32, bz: i32) -> [i32; 3] {
    [
        center[0] * SECTION_SIZE as i32 + bx - 1,
        y + WORLD_MIN_Y,
        center[1] * SECTION_SIZE as i32 + bz - 1,
    ]
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// 读元数据表 (chunk, section) 的均匀值;flag=0(非均匀)返回 `None`。
/// 输入已通过 `parse_rescan_input` 校验,索引必定在界内。
fn metadata_uniform(
    bytes: &[u8],
    metadata_offset: usize,
    chunk: usize,
    section: usize,
) -> Option<u16> {
    let offset = metadata_offset + (chunk * SECTIONS_PER_CHUNK + section) * 3;
    if bytes[offset] == METADATA_NON_UNIFORM {
        None
    } else {
        Some(read_u16(bytes, offset + 1))
    }
}

/// 解析并校验 MFL1 输入;任何违约(长度、layout_version、reserved、
/// start_section、区域界、区段记录 kind/pad、总长度、元数据 flag/id、
/// 中心区块坐标溢出)返回 `None`,由 FFI 层转为 `MORNLEA_STATUS_INPUT`。
pub(crate) fn parse_rescan_input(bytes: &[u8]) -> Option<RescanView<'_>> {
    if bytes.len() < RESCAN_HEADER_BYTES {
        return None;
    }
    if read_u32(bytes, 0) != RESCAN_LAYOUT_VERSION || bytes[21] != 0 {
        return None;
    }
    let center_x = read_i32(bytes, 4);
    let center_z = read_i32(bytes, 8);
    let x0 = read_u16(bytes, 12);
    let x1 = read_u16(bytes, 14);
    let z0 = read_u16(bytes, 16);
    let z1 = read_u16(bytes, 18);
    let start_section = bytes[20];
    let budget = read_u32(bytes, 22);
    if start_section as usize >= SECTIONS_PER_CHUNK {
        return None;
    }
    // 扫描区域是盒内局部列闭区间;布局域含裙边(0 与 17),生产调用方
    // (Go 五段平面)只会传中心列 1..=16,但布局本身不收紧。
    if x0 > x1 || x1 as usize >= BOX_COLUMNS || z0 > z1 || z1 as usize >= BOX_COLUMNS {
        return None;
    }
    // 世界坐标与裙边读都以 center×16 为基数;基数或 [−1, 16] 偏移溢出
    // i32 的区块无法在本 kernel 内表示,按输入违约拒绝(镜像 lod 对越界
    // tile 的处理)。
    for center in [center_x, center_z] {
        let base = center.checked_mul(SECTION_SIZE as i32)?;
        base.checked_add(BOX_COLUMNS as i32 - 2)?;
        base.checked_sub(1)?;
    }

    // 24 条区段记录:kind 决定记录长度,记录间紧密排列。
    let mut section_offsets = [0_usize; SECTIONS_PER_CHUNK];
    let mut section_uniform: [Option<u16>; SECTIONS_PER_CHUNK] = [None; SECTIONS_PER_CHUNK];
    let mut offset = RESCAN_HEADER_BYTES;
    for section in 0..SECTIONS_PER_CHUNK {
        if bytes.len() < offset + 2 {
            return None;
        }
        let kind = bytes[offset];
        if bytes[offset + 1] != 0 {
            return None;
        }
        section_offsets[section] = offset;
        match kind {
            SECTION_KIND_UNIFORM => {
                if bytes.len() < offset + UNIFORM_SECTION_BYTES {
                    return None;
                }
                section_uniform[section] = Some(read_u16(bytes, offset + 2));
                offset += UNIFORM_SECTION_BYTES;
            }
            SECTION_KIND_DENSE => {
                let end = offset.checked_add(DENSE_SECTION_BYTES)?;
                if bytes.len() < end {
                    return None;
                }
                offset = end;
            }
            _ => return None,
        }
    }

    // 裙边与元数据表的总长是固定剩余量:输入必须精确到字节。
    let skirt_offset = offset;
    let metadata_offset = skirt_offset.checked_add(SKIRT_COLUMNS * SKIRT_COLUMN_BYTES)?;
    if bytes.len() != metadata_offset.checked_add(METADATA_BYTES)? {
        return None;
    }
    // 元数据表:flag 只允许 0/1,flag=0(非均匀)时 id 必须为 0。
    for record in bytes[metadata_offset..].chunks_exact(3) {
        let flag = record[0];
        let id = u16::from_le_bytes([record[1], record[2]]);
        if flag > 1 || (flag == METADATA_NON_UNIFORM && id != 0) {
            return None;
        }
    }

    Some(RescanView {
        bytes,
        section_offsets,
        section_uniform,
        skirt_offset,
        metadata_offset,
        center_x,
        center_z,
        x0: x0 as usize,
        x1: x1 as usize,
        z0: z0 as usize,
        z1: z1 as usize,
        start_section: start_section as usize,
        budget,
    })
}

impl<'a> RescanView<'a> {
    /// 读盒内单格(bx、bz ∈ 0..=17,y ∈ 0..384);调用方保证范围内。
    fn box_cell(&self, bx: i32, y: i32, bz: i32) -> u16 {
        debug_assert!((0..BOX_COLUMNS as i32).contains(&bx));
        debug_assert!((0..BOX_COLUMNS as i32).contains(&bz));
        debug_assert!((0..WORLD_HEIGHT as i32).contains(&y));
        if (1..=SECTION_SIZE as i32).contains(&bx) && (1..=SECTION_SIZE as i32).contains(&bz) {
            let section = y as usize / SECTION_SIZE;
            match self.section_uniform[section] {
                Some(id) => id,
                None => {
                    let base = self.section_offsets[section] + 2;
                    let index = section_cell_index(bx, y, bz);
                    read_u16(self.bytes, base + index * 2)
                }
            }
        } else {
            let column = skirt_column(bx, bz);
            read_u16(
                self.bytes,
                self.skirt_offset + column * SKIRT_COLUMN_BYTES + y as usize * 2,
            )
        }
    }

    /// 镜像 Go `fluidRescanBlockAt`:世界 y 越界读 Barrier;盒内读中心
    /// 区段数据或裙边列。Go 侧「chunk 未就绪读 Barrier」由编码方用
    /// Barrier 裙边表达,kernel 不感知就绪状态。
    fn block_at(&self, bx: i32, y: i32, bz: i32) -> u16 {
        if y < 0 || y >= WORLD_HEIGHT as i32 {
            return BARRIER;
        }
        self.box_cell(bx, y, bz)
    }
}

/// Capability seam for the shared rescan accounting loop.
///
/// The loop reads box cells, center-section uniforms, neighbour metadata
/// uniforms and world positions through this trait, so the legacy byte view
/// and the typed native view run exactly one accounting implementation.
/// `cell` answers the world-height barrier rule for a y outside the world
/// column and `Err(KernelError::MissingHalo)` for a horizontal coordinate the
/// view does not own; the seal predicates propagate that error instead of
/// substituting a barrier, because a fabricated barrier would silently change
/// which cells are treated as fixed points.
pub(crate) trait RescanSource {
    /// Reads one box cell `(bx, y, bz)`, applying the `fluidRescanBlockAt`
    /// world-height rule; a coordinate outside the owned halo is
    /// `Err(KernelError::MissingHalo)`.
    fn cell(&self, bx: i32, y: i32, bz: i32) -> Result<u16, KernelError>;

    /// The uniform id of the center chunk's section, if that section is
    /// uniform. Center section records always win over the metadata table.
    fn section_uniform(&self, section: usize) -> Option<u16>;

    /// A neighbour chunk's metadata uniform id for one section.
    fn meta_uniform(&self, dx: i32, dz: i32, section: usize) -> Option<u16>;

    /// The world position of a box cell.
    fn position(&self, bx: i32, y: i32, bz: i32) -> [i32; 3];

    /// Mirrors Go `fluidSourceIsFixedPoint` over the five-neighbour seal rule
    /// (below plus the four horizontals, all unreplaceable for new level-1
    /// water). Every neighbour read goes through `cell`, so a missing halo
    /// propagates as `MissingHalo` and is never read as a seal.
    fn source_is_fixed_point(&self, bx: i32, y: i32, bz: i32) -> Result<bool, KernelError> {
        for &(dx, dy, dz) in &SEALED_OFFSETS {
            if replaceable(self.cell(bx + dx, y + dy, bz + dz)?, 1) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Mirrors Go `fluidSectionIsFixedPoint`: the section below plus the four
    /// horizontal neighbour sections are all uniform and unreplaceable. The
    /// below section is judged from the center chunk's own section records,
    /// never from the metadata table, and section 0 is sealed below (Go's
    /// out-of-range section rule). A missing or non-uniform neighbour record
    /// is not a seal. The result shares the error channel with
    /// `source_is_fixed_point` so the loop propagates a read failure instead
    /// of inventing one.
    fn section_is_fixed_point(&self, section: usize) -> Result<bool, KernelError> {
        let below_is_unreplaceable = section == 0
            || matches!(
                self.section_uniform(section - 1),
                Some(id) if !replaceable(id, 1)
            );
        if !below_is_unreplaceable {
            return Ok(false);
        }
        for &(dx, dz) in &[(1, 0), (-1, 0), (0, 1), (0, -1)] {
            match self.meta_uniform(dx, dz, section) {
                Some(id) if !replaceable(id, 1) => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }
}

impl RescanSource for RescanView<'_> {
    /// The parsed legacy box owns every column `0..=17` and the full height,
    /// so this adapter keeps the historical reads (and the historical
    /// `skirt_column` panic for a coordinate outside the box) unchanged.
    fn cell(&self, bx: i32, y: i32, bz: i32) -> Result<u16, KernelError> {
        Ok(self.block_at(bx, y, bz))
    }

    fn section_uniform(&self, section: usize) -> Option<u16> {
        self.section_uniform.get(section).copied().flatten()
    }

    fn meta_uniform(&self, dx: i32, dz: i32, section: usize) -> Option<u16> {
        metadata_uniform(
            self.bytes,
            self.metadata_offset,
            chunk_meta_index(dx, dz),
            section,
        )
    }

    fn position(&self, bx: i32, y: i32, bz: i32) -> [i32; 3] {
        box_to_world([self.center_x, self.center_z], bx, y, bz)
    }
}

/// Shared rescan accounting loop, mirroring Go `enqueueChunkFluids`.
///
/// The budget is checked before every section, an entered section always
/// completes (one call may overshoot by at most one section), the two uniform
/// fast paths cost one each, and every other cell inside the span costs one in
/// `y16` outer, `z` middle, `x` inner order. Positions are appended through
/// `sink` in that same order. The returned triple is
/// `(spent, done, next_section)`: on a budget break `next_section` is the
/// section that was not entered, and on completion it is the section count.
pub(crate) fn rescan_scan<S, F>(
    source: &S,
    span: (usize, usize, usize, usize),
    start_section: usize,
    budget: u32,
    sink: &mut F,
) -> Result<(u64, bool, usize), KernelError>
where
    S: RescanSource,
    F: FnMut([i32; 3]),
{
    let (x0, x1, z0, z1) = span;
    let mut spent: u64 = 0;
    let budget = u64::from(budget);
    let mut done = true;
    let mut section = start_section;
    while section < SECTIONS_PER_CHUNK {
        // 区段循环前查额度:进入一个区段后必须整段完成,单次调用至多
        // 超支一个区段(spent 在进入前 < budget,整段最多再计 4096)。
        if spent >= budget {
            done = false;
            break;
        }
        if let Some(id) = source.section_uniform(section) {
            // 档 1:均匀非流体区段整段计 1。
            if !is_fluid(id) {
                spent += 1;
                section += 1;
                continue;
            }
            // 档 2:均匀水源区段且区段级不动点成立,整段计 1;均匀
            // 流动水区段落到档 3(Go 同样只在均匀 + 源 + 不动点时捷径)。
            if id == WATER_SOURCE && source.section_is_fixed_point(section)? {
                spent += 1;
                section += 1;
                continue;
            }
        }
        // 档 3:逐格记账,扫描范围内每格计 1;循环序 y16 外、z 中、x 内,
        // 与 Go 一致,产出坐标流因此确定。
        for y16 in 0..SECTION_SIZE {
            let y = (section * SECTION_SIZE + y16) as i32;
            for bz in z0..=z1 {
                for bx in x0..=x1 {
                    spent += 1;
                    let id = source.cell(bx as i32, y, bz as i32)?;
                    if !is_fluid(id) {
                        continue;
                    }
                    if id == WATER_SOURCE
                        && source.source_is_fixed_point(bx as i32, y, bz as i32)?
                    {
                        continue;
                    }
                    sink(source.position(bx as i32, y, bz as i32));
                }
            }
        }
        section += 1;
    }
    // 扫描范围至多 18×18 列 × 全高 384,spent 不可能超出 u32。
    debug_assert!(spent <= u32::MAX as u64);
    Ok((spent, done, section))
}

/// 执行重扫扫描并编码输出(坐标流 + summary)。
///
/// 扫描循环镜像 Go `enqueueChunkFluids` 的结构:区段循环前查额度 →
/// 均匀段捷径(非流体计 1;水源 + 区段级不动点计 1)→ 逐格循环(每格
/// 计 1,流体格产出,水源格过五邻不动点)。`done` 为假时调用方以
/// `spent` 重放记账可推出续扫区段(记账确定性)。
pub(crate) fn fluid_rescan(view: &RescanView) -> Vec<u8> {
    let mut output = Vec::new();
    let mut push = |position: [i32; 3]| {
        let mut entry = [0_u8; RESCAN_POSITION_BYTES];
        entry[0..4].copy_from_slice(&position[0].to_le_bytes());
        entry[4..8].copy_from_slice(&position[1].to_le_bytes());
        entry[8..12].copy_from_slice(&position[2].to_le_bytes());
        output.extend_from_slice(&entry);
    };
    let (spent, done, _) = rescan_scan(
        view,
        (view.x0, view.x1, view.z0, view.z1),
        view.start_section,
        view.budget,
        &mut push,
    )
    .unwrap_or_else(|_| {
        unreachable!("the parsed legacy view owns every coordinate the scan reads")
    });
    let mut tail = [0_u8; RESCAN_SUMMARY_BYTES];
    tail[0..4].copy_from_slice(&(spent as u32).to_le_bytes());
    tail[4] = u8::from(done);
    output.extend_from_slice(&tail);
    output
}

/// 测试助手:按 MFL1 布局手工拼装输入盒(本模块与 `ffi.rs` 的 FFI 测试
/// 共用,镜像 `fluid_eval::encode_eval_input` 的惯例)。
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// 测试专用「非作物实心方块」样本(协议稳定值,钉位见 `fluid_eval`)。
    pub(crate) const STONE: u16 = 2;

    /// 单个区段记录的内容:均匀值或 4096 格密集数据。
    enum SectionRecord {
        Uniform(u16),
        Dense(Vec<u16>),
    }

    pub(crate) struct RescanBox {
        pub center_x: i32,
        pub center_z: i32,
        pub x0: u16,
        pub x1: u16,
        pub z0: u16,
        pub z1: u16,
        pub start_section: u8,
        pub budget: u32,
        /// 刻意可改,用于构造违约输入。
        pub layout_version: u32,
        pub reserved: u8,
        sections: Vec<SectionRecord>,
        skirt: Vec<u16>,
        metadata: Vec<u8>,
    }

    impl RescanBox {
        /// 全实心默认盒:24 个均匀石段、石裙边、全均匀石元数据、全区块
        /// 列扫描(盒列 1..=16)、start_section=0、budget 充裕。
        pub(crate) fn new(center_x: i32, center_z: i32) -> Self {
            Self {
                center_x,
                center_z,
                x0: 1,
                x1: SECTION_SIZE as u16,
                z0: 1,
                z1: SECTION_SIZE as u16,
                start_section: 0,
                budget: 100_000,
                layout_version: RESCAN_LAYOUT_VERSION,
                reserved: 0,
                sections: (0..SECTIONS_PER_CHUNK)
                    .map(|_| SectionRecord::Uniform(STONE))
                    .collect(),
                skirt: vec![STONE; SKIRT_COLUMNS * WORLD_HEIGHT],
                metadata: [1, STONE as u8, 0]
                    .iter()
                    .copied()
                    .cycle()
                    .take(METADATA_BYTES)
                    .collect(),
            }
        }

        /// 把区段置为均匀。
        pub(crate) fn uniform_section(&mut self, section: usize, id: u16) {
            self.sections[section] = SectionRecord::Uniform(id);
        }

        /// 把区段置为密集并按 (lx, y16, lz) 填充。
        pub(crate) fn dense_section(
            &mut self,
            section: usize,
            fill: impl Fn(usize, usize, usize) -> u16,
        ) {
            let mut cells = Vec::with_capacity(SECTION_SIZE.pow(3));
            for y16 in 0..SECTION_SIZE {
                for lz in 0..SECTION_SIZE {
                    for lx in 0..SECTION_SIZE {
                        cells.push(fill(lx, y16, lz));
                    }
                }
            }
            self.sections[section] = SectionRecord::Dense(cells);
        }

        /// 在密集区段上改单格(区段须已转密集,否则视为夹具错误)。
        pub(crate) fn set_center_cell(
            &mut self,
            section: usize,
            lx: usize,
            y16: usize,
            lz: usize,
            id: u16,
        ) {
            let SectionRecord::Dense(cells) = &mut self.sections[section] else {
                panic!("set_center_cell 要求区段已是密集记录");
            };
            cells[lx + lz * SECTION_SIZE + y16 * SECTION_SIZE * SECTION_SIZE] = id;
        }

        /// 写裙边单格(bx/bz 为盒内局部列 0..=17)。
        pub(crate) fn set_skirt_cell(&mut self, bx: i32, y: usize, bz: i32, id: u16) {
            let column = skirt_column(bx, bz);
            self.skirt[column * WORLD_HEIGHT + y] = id;
        }

        /// 写邻区块元数据 (flag, id);(0,0) 是中心条目。
        pub(crate) fn set_meta(&mut self, dx: i32, dz: i32, section: usize, flag: u8, id: u16) {
            let offset = (chunk_meta_index(dx, dz) * SECTIONS_PER_CHUNK + section) * 3;
            self.metadata[offset] = flag;
            self.metadata[offset + 1..offset + 3].copy_from_slice(&id.to_le_bytes());
        }

        /// 编码为完整 MFL1 输入字节流。
        pub(crate) fn build(&self) -> Vec<u8> {
            let mut bytes = Vec::with_capacity(
                RESCAN_HEADER_BYTES
                    + SECTIONS_PER_CHUNK * DENSE_SECTION_BYTES
                    + SKIRT_COLUMNS * SKIRT_COLUMN_BYTES
                    + METADATA_BYTES,
            );
            bytes.extend_from_slice(&self.layout_version.to_le_bytes());
            bytes.extend_from_slice(&self.center_x.to_le_bytes());
            bytes.extend_from_slice(&self.center_z.to_le_bytes());
            for value in [self.x0, self.x1, self.z0, self.z1] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.push(self.start_section);
            bytes.push(self.reserved);
            bytes.extend_from_slice(&self.budget.to_le_bytes());
            for section in &self.sections {
                match section {
                    SectionRecord::Uniform(id) => {
                        bytes.push(SECTION_KIND_UNIFORM);
                        bytes.push(0);
                        bytes.extend_from_slice(&id.to_le_bytes());
                    }
                    SectionRecord::Dense(cells) => {
                        bytes.push(SECTION_KIND_DENSE);
                        bytes.push(0);
                        for id in cells {
                            bytes.extend_from_slice(&id.to_le_bytes());
                        }
                    }
                }
            }
            for id in &self.skirt {
                bytes.extend_from_slice(&id.to_le_bytes());
            }
            bytes.extend_from_slice(&self.metadata);
            bytes
        }

        /// 解析并扫描,返回输出字节(测试期望输入必然合法)。
        pub(crate) fn run(&self) -> Vec<u8> {
            let bytes = self.build();
            let view = parse_rescan_input(&bytes).expect("测试盒必须是合法输入");
            fluid_rescan(&view)
        }
    }

    /// 解码输出尾部 summary 为 (spent, done)。
    pub(crate) fn decode_summary(output: &[u8]) -> (u32, bool) {
        let tail = &output[output.len() - RESCAN_SUMMARY_BYTES..];
        (
            u32::from_le_bytes(tail[0..4].try_into().expect("定长切片")),
            tail[4] == 1,
        )
    }

    /// 解码全部坐标为 (x, y, z)(二进制补码重读,与 Go 侧约定一致)。
    pub(crate) fn decode_positions(output: &[u8]) -> Vec<(i32, i32, i32)> {
        output[..output.len() - RESCAN_SUMMARY_BYTES]
            .chunks_exact(RESCAN_POSITION_BYTES)
            .map(|entry| {
                (
                    i32::from_le_bytes(entry[0..4].try_into().expect("定长切片")),
                    i32::from_le_bytes(entry[4..8].try_into().expect("定长切片")),
                    i32::from_le_bytes(entry[8..12].try_into().expect("定长切片")),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{RescanBox, STONE, decode_positions, decode_summary};
    use super::*;
    use crate::fluid_eval::{AIR, WATER_SOURCE};

    #[test]
    fn mfl1_layout_bytes_are_pinned() {
        // 字段和钉位:header 4+4+4+2+2+2+2+1+1+4 = 26;均匀段记录 4B、
        // 密集段记录 2+8192 = 8194B;裙边 68 列 × 384 u16 = 52224B;元数据
        // 9×24×3 = 648B;单条坐标 12B;summary 8B。
        assert_eq!(RESCAN_HEADER_BYTES, 26);
        assert_eq!(UNIFORM_SECTION_BYTES, 4);
        assert_eq!(DENSE_SECTION_BYTES, 8194);
        assert_eq!(SKIRT_COLUMNS * SKIRT_COLUMN_BYTES, 52_224);
        assert_eq!(METADATA_BYTES, 648);
        assert_eq!(RESCAN_POSITION_BYTES, 12);
        assert_eq!(RESCAN_SUMMARY_BYTES, 8);
        // 全均匀盒的最小输入长度。
        let bytes = RescanBox::new(0, 0).build();
        assert_eq!(bytes.len(), 26 + 24 * 4 + 52_224 + 648);
    }

    #[test]
    fn parse_validates_header_contract() {
        let base = RescanBox::new(-3, 2).build();
        let view = parse_rescan_input(&base).expect("合法盒必须通过解析");
        assert_eq!((view.center_x, view.center_z), (-3, 2));
        assert_eq!((view.x0, view.x1, view.z0, view.z1), (1, 16, 1, 16));
        assert_eq!(view.start_section, 0);
        assert_eq!(view.budget, 100_000);

        let mut wrong_version = base.clone();
        wrong_version[0..4].copy_from_slice(&2_u32.to_le_bytes());
        assert!(parse_rescan_input(&wrong_version).is_none());
        let mut reserved = base.clone();
        reserved[21] = 1;
        assert!(parse_rescan_input(&reserved).is_none());
        let mut start = base.clone();
        start[20] = 24;
        assert!(parse_rescan_input(&start).is_none());
        // 区域界:盒内局部列只到 17。
        for (offset, value) in [(12_usize, 18_u16), (14, 18), (16, 18), (18, 18)] {
            let mut bytes = base.clone();
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
            assert!(parse_rescan_input(&bytes).is_none(), "offset={offset}");
        }
        // 区间倒挂(x1/z1 比 x0/z0 小)。
        for offset in [14_usize, 18] {
            let mut bytes = base.clone();
            bytes[offset..offset + 2].copy_from_slice(&0_u16.to_le_bytes());
            assert!(parse_rescan_input(&bytes).is_none(), "倒挂 offset={offset}");
        }
        // 裙边列 17 在布局域内:生产只传中心列,但布局不得拒绝。
        let mut skirt_region = base.clone();
        skirt_region[14..16].copy_from_slice(&17_u16.to_le_bytes());
        skirt_region[18..20].copy_from_slice(&17_u16.to_le_bytes());
        assert!(parse_rescan_input(&skirt_region).is_some());
        // 中心区块坐标 ×16 后越出 i32 世界坐标域。
        let mut extreme = base.clone();
        extreme[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(parse_rescan_input(&extreme).is_none());
    }

    #[test]
    fn parse_validates_section_records_and_total_length() {
        let base = RescanBox::new(0, 0).build();
        let mut bad_kind = base.clone();
        bad_kind[RESCAN_HEADER_BYTES] = 2;
        assert!(parse_rescan_input(&bad_kind).is_none());
        let mut pad = base.clone();
        pad[RESCAN_HEADER_BYTES + 1] = 1;
        assert!(parse_rescan_input(&pad).is_none());
        assert!(parse_rescan_input(&base[..base.len() - 1]).is_none());
        let mut long = base.clone();
        long.push(0);
        assert!(parse_rescan_input(&long).is_none());

        // 密集段记录:均匀性读作 None,其余段不受影响。
        let mut dense = RescanBox::new(0, 0);
        dense.dense_section(0, |_, _, _| STONE);
        let bytes = dense.build();
        let view = parse_rescan_input(&bytes).expect("密集段盒必须合法");
        assert_eq!(view.section_uniform[0], None);
        assert_eq!(view.section_uniform[1], Some(STONE));
    }

    #[test]
    fn parse_validates_metadata_table() {
        let base = RescanBox::new(0, 0).build();
        let metadata_offset = base.len() - METADATA_BYTES;
        // flag 只允许 0/1。
        let mut bad_flag = base.clone();
        bad_flag[metadata_offset] = 2;
        assert!(parse_rescan_input(&bad_flag).is_none());
        // flag=0 时 id 必须为 0。
        let mut nonzero = base.clone();
        nonzero[metadata_offset] = 0;
        nonzero[metadata_offset + 1] = 2;
        assert!(parse_rescan_input(&nonzero).is_none());
        // flag=0 + id=0 合法(非均匀元数据)。
        let mut sparse = base.clone();
        sparse[metadata_offset..metadata_offset + 3].copy_from_slice(&[0, 0, 0]);
        assert!(parse_rescan_input(&sparse).is_some());
    }

    #[test]
    fn box_accessor_reads_center_records_and_skirt_columns() {
        let mut box_ = RescanBox::new(0, 0);
        // 中心密集段:每格写入唯一标记(区段内序 x + z*16 + y16*256)。
        box_.dense_section(5, |lx, y16, lz| (1000 + lx + lz * 16 + y16 * 256) as u16);
        // 裙边列:列序 × 全高唯一标记。
        for bx in 0..=17 {
            for bz in 0..=17 {
                if (1..=16).contains(&bx) && (1..=16).contains(&bz) {
                    continue;
                }
                let column = skirt_column(bx, bz);
                for y in 0..WORLD_HEIGHT {
                    box_.set_skirt_cell(bx, y, bz, (20_000 + column * 384 + y) as u16);
                }
            }
        }
        let bytes = box_.build();
        let view = parse_rescan_input(&bytes).expect("标记盒必须合法");
        // 中心列映射 (lx+1, lz+1),逐段核对采样点。
        for (lx, y16, lz) in [(0, 0, 0), (15, 0, 15), (3, 7, 9), (15, 15, 0)] {
            assert_eq!(
                view.box_cell(lx as i32 + 1, (5 * 16 + y16) as i32, lz as i32 + 1),
                1000 + lx + lz * 16 + y16 * 256,
                "({lx},{y16},{lz})"
            );
        }
        // 非密集段仍是均匀记录。
        assert_eq!(view.box_cell(1, 0, 1), STONE);
        // 全部 18×18 列 × 采样 y 逐格核对(覆盖段界与裙边四边四角)。
        for y in [0_usize, 79, 80, 383] {
            for bx in 0..=17_i32 {
                for bz in 0..=17_i32 {
                    let want = if (1..=16).contains(&bx) && (1..=16).contains(&bz) {
                        if y / 16 == 5 {
                            1000 + (bx - 1) + (bz - 1) * 16 + (y % 16) as i32 * 256
                        } else {
                            STONE as i32
                        }
                    } else {
                        20_000 + skirt_column(bx, bz) as i32 * 384 + y as i32
                    };
                    assert_eq!(
                        view.box_cell(bx, y as i32, bz),
                        want as u16,
                        "({bx},{y},{bz})"
                    );
                }
            }
        }
        // 世界 y 越界读 Barrier(镜像 Go fluidRescanBlockAt 的越界分支)。
        assert_eq!(view.block_at(3, -1, 3), BARRIER);
        assert_eq!(view.block_at(3, WORLD_HEIGHT as i32, 3), BARRIER);
        assert_eq!(view.block_at(0, -1, 0), BARRIER);
    }

    #[test]
    fn uniform_section_accounting_tiers_mirror_go() {
        // 档 1:均匀非流体区段计 1。档 2:均匀水源 + 区段级不动点计 1
        // (下方段 0 均匀石 + 四邻元数据均匀石)。
        let mut sealed = RescanBox::new(0, 0);
        sealed.uniform_section(1, WATER_SOURCE);
        let output = sealed.run();
        assert_eq!(decode_summary(&output), (24, true));
        assert!(decode_positions(&output).is_empty());

        // 元数据 (1,0) 非均匀 → 区段级不动点不成立 → 档 3 逐格 4096;
        // 段内互为水源邻居、边缘邻裙边石,全部密封 → 无产出。
        let mut per_cell = RescanBox::new(0, 0);
        per_cell.uniform_section(2, WATER_SOURCE);
        per_cell.set_meta(1, 0, 2, 0, 0);
        let output = per_cell.run();
        assert_eq!(decode_summary(&output), (2 + 4096 + 21, true));
        assert!(decode_positions(&output).is_empty());

        // 邻区块元数据是均匀空气 → 不可替换性被破坏 → 逐格。
        let mut open_side = RescanBox::new(0, 0);
        open_side.uniform_section(1, WATER_SOURCE);
        open_side.set_meta(0, 1, 1, 1, AIR);
        let output = open_side.run();
        assert_eq!(decode_summary(&output), (1 + 4096 + 22, true));
        assert!(decode_positions(&output).is_empty());
    }

    #[test]
    fn section_fixed_point_below_uses_section_records_over_metadata() {
        // 裁决钉位:下方区段判定用中心区段记录(Go 读同一 chunk 活数据),
        // 不看元数据表中心条目。两个方向各构造一次不一致输入。
        let mut records_dense = RescanBox::new(0, 0);
        records_dense.dense_section(0, |_, _, _| STONE);
        records_dense.uniform_section(1, WATER_SOURCE);
        // 记录说非均匀 → 不动点不成立 → 段 0 与段 1 都逐格。
        let output = records_dense.run();
        assert_eq!(decode_summary(&output), (4096 + 4096 + 22, true));
        assert!(decode_positions(&output).is_empty());

        let mut meta_sparse = RescanBox::new(0, 0);
        meta_sparse.uniform_section(1, WATER_SOURCE);
        meta_sparse.set_meta(0, 0, 0, 0, 0);
        // 记录说均匀石 → 不动点成立 → 段 1 仍计 1。
        let output = meta_sparse.run();
        assert_eq!(decode_summary(&output), (24, true));
    }

    #[test]
    fn uniform_short_grass_neighbor_breaks_section_fixed_point() {
        // 段 1 均匀水源,元数据 (0,1) 同段是均匀短草:短草可替换,区段级
        // 不动点不成立 → 档 3 逐格 4096。段内边缘源的 −x 邻格读裙边(石)
        // 仍密封,五邻不动点全部成立,无产出——本用例钉的是重扫侧与
        // `fluid_eval::replaceable` 共用同一植物谓词,短草不会让水源区段
        // 被误判成不动点而漏扫。
        let mut box_ = RescanBox::new(0, 0);
        box_.uniform_section(1, WATER_SOURCE);
        box_.set_meta(0, 1, 1, 1, crate::fluid_eval::SHORT_GRASS);
        let output = box_.run();
        assert_eq!(decode_summary(&output), (1 + 4096 + 22, true));
        assert!(decode_positions(&output).is_empty());
    }

    #[test]
    fn unsealed_edge_sources_emit_through_skirt_column() {
        // 段 3 均匀水源,元数据 (0,1) 均匀空气破坏区段级不动点;裙边列
        // (bx=0, bz=5) 全高空气 → 中心列 (lx=0, lz=4) 的 16 个源格的 −x
        // 邻格可替换 → 逐 y16 产出;其余格仍全部密封。
        let mut box_ = RescanBox::new(0, 0);
        box_.uniform_section(3, WATER_SOURCE);
        box_.set_meta(0, 1, 3, 1, AIR);
        for y in 0..WORLD_HEIGHT {
            box_.set_skirt_cell(0, y, 5, AIR);
        }
        let output = box_.run();
        let want: Vec<(i32, i32, i32)> = (0..16).map(|y16| (0, 3 * 16 + y16 - 64, 4)).collect();
        assert_eq!(decode_positions(&output), want);
        assert_eq!(decode_summary(&output), (3 + 4096 + 20, true));
    }

    #[test]
    fn mixed_dense_section_emits_in_scan_order_with_world_coords() {
        let mut box_ = RescanBox::new(-3, 2);
        box_.dense_section(2, |_, _, _| STONE);
        // 流动水:不做不动点判定,直接产出。
        box_.set_center_cell(2, 3, 1, 4, WATER_SOURCE + 2);
        // 源格,下方空气 → 未密封 → 产出。
        box_.set_center_cell(2, 5, 2, 6, WATER_SOURCE);
        box_.set_center_cell(2, 5, 1, 6, AIR);
        // 源格,下方与水平邻居均石 → 密封,不产出。
        box_.set_center_cell(2, 8, 3, 9, WATER_SOURCE);
        // 角列源格:水平邻格读裙边(石)→ 密封。
        box_.set_center_cell(2, 0, 5, 0, WATER_SOURCE);
        // 段底源格:下方跨段读段 1(均匀石)→ 密封。
        box_.set_center_cell(2, 15, 0, 7, WATER_SOURCE);
        let output = box_.run();
        assert_eq!(
            decode_positions(&output),
            vec![(-45, -31, 36), (-43, -30, 38)]
        );
        assert_eq!(decode_summary(&output), (2 + 4096 + 21, true));
    }

    #[test]
    fn positions_encode_negative_world_coordinates_as_twos_complement() {
        let mut box_ = RescanBox::new(-3, 2);
        box_.dense_section(0, |_, _, _| STONE);
        box_.set_center_cell(0, 1, 0, 1, WATER_SOURCE + 4);
        let output = box_.run();
        // (−47, −64, 33):x/y 为负,u32 LE 按二进制补码钉位。
        assert_eq!(
            output[..RESCAN_POSITION_BYTES],
            [
                0xD1, 0xFF, 0xFF, 0xFF, // −47
                0xC0, 0xFF, 0xFF, 0xFF, // −64
                0x21, 0x00, 0x00, 0x00, // 33
            ]
        );
        assert_eq!(decode_summary(&output), (4096 + 23, true));
    }

    #[test]
    fn budget_exhaustion_overshoot_and_resume_mirror_go() {
        let mut box_ = RescanBox::new(0, 0);
        // 段 0..4 均匀石(各 1),段 5 密集石(4096),段 6..23 均匀石。
        box_.dense_section(5, |_, _, _| STONE);
        let base = box_.build();

        let run_with = |start: u8, budget: u32| -> Vec<u8> {
            let mut bytes = base.clone();
            bytes[20] = start;
            bytes[22..26].copy_from_slice(&budget.to_le_bytes());
            let view = parse_rescan_input(&bytes).expect("测试盒必须合法");
            fluid_rescan(&view)
        };

        // 段 0..4 计 5 后进入段 5 并整段完成,spent = 5 + 4096 = 4101;
        // 段 6 前的额度检查 4101 ≥ 10 → 未扫完。
        let output = run_with(0, 10);
        let (spent, done) = decode_summary(&output);
        assert_eq!(
            (spent, done, decode_positions(&output).is_empty()),
            (4101, false, true)
        );
        // 单次调用至多超支一个区段:超支量 ≤ 一个区段的满额 4096。
        assert!(spent - 10 <= 4096);

        // 从段 6 续扫:18 个均匀段 → 扫完。
        assert_eq!(decode_summary(&run_with(6, 1000)), (18, true));

        // 精确边界:额度检查发生在每个区段之前,spent == budget 时下一个
        // 区段前的检查立即失败(Go 的 `>=` 语义);完成全部 24 段需要在
        // 末段前的每个检查点都严格小于,即 budget ≥ 4119 才 done。
        assert_eq!(decode_summary(&run_with(0, 4101)), (4101, false));
        assert_eq!(decode_summary(&run_with(0, 4118)), (4118, false));
        assert_eq!(decode_summary(&run_with(0, 4119)), (4119, true));

        // budget=0:第一个区段前即返回,零进度、未扫完。
        assert_eq!(decode_summary(&run_with(0, 0)), (0, false));

        // 起点即末段:一个均匀段计 1 后完成。
        assert_eq!(decode_summary(&run_with(23, 1)), (1, true));
    }

    #[test]
    fn world_bottom_source_reads_barrier_below() {
        // 世界底(y 指标 0,世界 y=−64)的正下方越界读 Barrier:Barrier
        // 不可替换,底部源格天然密封——与 Go `fluidRescanBlockAt` 的越界
        // 分支一致。
        let mut box_ = RescanBox::new(0, 0);
        box_.dense_section(0, |_, _, _| STONE);
        box_.set_center_cell(0, 2, 0, 2, WATER_SOURCE);
        let output = box_.run();
        assert!(decode_positions(&output).is_empty());
        assert_eq!(decode_summary(&output), (4096 + 23, true));
    }
}
