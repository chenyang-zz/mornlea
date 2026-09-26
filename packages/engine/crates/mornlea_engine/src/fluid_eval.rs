//! fluid_eval:流体单格规则批量求值内核(无状态纯函数)。
//!
//! 逐条镜像 Go `internal/fluid/rules.go` 的三段纯整数规则——`evalCell` 的
//! 「陈旧项跳过 → 非源存活判定 → 垂直优先即返 → 水平传播等级 +1 且 ≤7」、
//! `flowingSurvives` 的「上方任意流体或更强水平邻居」、`Replaceable` 的判定表
//! (空气/植物(作物、短草与树苗)/门四态/源不可替换/弱水可被强水替换,分支顺序与
//! Go 侧一致)。
//! kernel 只回答「本次求值想写哪些格」,队列、预算与写入编排留在 Go 侧。
//!
//! 输入布局 v1:`u32 layout_version=1` + `u32 item_count` + 每项 14 字节
//! (7 个 u16 LE 方块编号,槽位序 0=自格、1=上、2=下、3=+x、4=−x、5=+z、
//! 6=−z,与 Go `sixNeighbors` 同序);输出布局:每项 12 字节 = 4 条候选写入
//! × 3B(目标槽位 u8(0..6;0xFF=无写入哨兵)+ BlockID u16 LE)。

use crate::worldgen::read_u32;

/// 输入布局版本:输入头部 `layout_version` 字段的唯一合法值。
pub(crate) const EVAL_LAYOUT_VERSION: u32 = 1;
/// 输入头部长度:layout_version + item_count 两个 u32。
pub(crate) const EVAL_HEADER_BYTES: usize = 8;
/// 单项输入字节数:7 个 u16 槽位。
pub(crate) const EVAL_ITEM_BYTES: usize = 14;
/// 单项输出字节数:4 条候选写入 × 3B。
pub(crate) const EVAL_ITEM_OUTPUT_BYTES: usize = 12;
/// 每项槽位数:自格 + 上/下 + 四个水平方向。
pub(crate) const EVAL_SLOTS_PER_ITEM: usize = 7;

const SLOT_SELF: usize = 0;
const SLOT_ABOVE: usize = 1;
const SLOT_BELOW: usize = 2;
/// 水平槽位序:+x、−x、+z、−z,与 Go `horizontalNeighbors` 同序。
const HORIZONTAL_SLOTS: [usize; 4] = [3, 4, 5, 6];

/// 输出槽位条目的「无写入」哨兵:槽位字节固定 0xFF,BlockID 补零。
pub(crate) const SLOT_NO_WRITE: u8 = 0xFF;

/// 水平传播下界:等级 7 之外不再产生更弱流动水。
const MAX_FLUID_LEVEL: u8 = 7;

// 方块编号是协议稳定值,与 Go `internal/core/block.go` 的 iota 逐一对应
// (两侧注释互指;该文件明确约定这些编号只能追加、不能重排,故此处固化为
// 字面量是安全的,数值已于实现时用 Go 侧实测钉位):
// Air=0、Barrier=1、Stone=2、WaterSource=27..WaterLevel7=34(流体 8 连号,
// WaterLevelN == WaterSource+N)、WheatStage0..7=37..44、Workbench=45、
// PotatoStage0..7=46..53、CarrotStage0..7=54..61、门下半 62..69
// (南/西/北/东 × 关/开,开态 = 63/65/67/69)、门上半 70、ShortGrass=84、
// Sapling=89(只追加在四档雪层 85..88 之后)。
pub(crate) const AIR: u16 = 0;
/// Barrier:越界/未就绪世界读的替身,重扫 kernel 与 Go `fluidRescanBlockAt`
/// 共用同一语义(Barrier 不可替换,视作密封)。
pub(crate) const BARRIER: u16 = 1;
pub(crate) const WATER_SOURCE: u16 = 27;
const WATER_LEVEL_7: u16 = 34;
const WHEAT_STAGE_0: u16 = 37;
const WHEAT_STAGE_7: u16 = 44;
const POTATO_STAGE_0: u16 = 46;
const POTATO_STAGE_7: u16 = 53;
const CARROT_STAGE_0: u16 = 54;
const CARROT_STAGE_7: u16 = 61;
const DOOR_LOWER_SOUTH_CLOSED: u16 = 62;
const DOOR_LOWER_SOUTH_OPEN: u16 = 63;
const DOOR_LOWER_WEST_OPEN: u16 = 65;
const DOOR_LOWER_NORTH_OPEN: u16 = 67;
const DOOR_LOWER_EAST_OPEN: u16 = 69;
const DOOR_UPPER: u16 = 70;
/// ShortGrass:原创短草的协议稳定编号,被流动水覆盖时零掉落且不受掉落容量
/// 限制——该结算语义在 Go sim 写入侧,这里只参与可替换判定。
pub(crate) const SHORT_GRASS: u16 = 84;
/// Sapling:橡树树苗的协议稳定编号(Go `core.SaplingID`)。树苗是第二个非作物
/// 植物方块(短草是第一个),与作物、短草一样对流动水可替换;被覆盖后的掉落
/// 结算(1 个树苗及其容量原子拒绝)在 Go sim 写入侧,不在作物掉落表上——
/// kernel 只回答「能不能写」,这里只参与可替换判定。
pub(crate) const SAPLING: u16 = 89;

/// 镜像 Go `core.IsFluid`:流体 = 源 + 7 档流动水,共 8 个连续编号。
pub(crate) fn is_fluid(id: u16) -> bool {
    (WATER_SOURCE..=WATER_LEVEL_7).contains(&id)
}

/// 镜像 Go `core.FluidLevel`:源读作 0,流动水读作 1..7(数字越大越弱)。
/// 只对 `is_fluid` 为真的编号有流体语义,与 Go 侧约定一致。
fn fluid_level(id: u16) -> u8 {
    if id == WATER_SOURCE {
        0
    } else {
        (id - WATER_SOURCE) as u8
    }
}

/// 镜像 Go `core.IsCrop`:小麦/马铃薯/胡萝卜三段连续区间(工作台 45 把
/// 小麦与马铃薯隔开,因此不能并成一段)。
fn is_crop(id: u16) -> bool {
    (WHEAT_STAGE_0..=WHEAT_STAGE_7).contains(&id)
        || (POTATO_STAGE_0..=POTATO_STAGE_7).contains(&id)
        || (CARROT_STAGE_0..=CARROT_STAGE_7).contains(&id)
}

/// 镜像 Go `core.IsPlant`:作物 ∪ 短草 ∪ 树苗。植物的「可被流动水替换」共有
/// 语义收口在这一个谓词,短草与树苗不得只在编号上特判——否则后续植物消费者
/// 会与作物判定面漂移。农业状态机语义仍只认 `is_crop`。
fn is_plant(id: u16) -> bool {
    is_crop(id) || id == SHORT_GRASS || id == SAPLING
}

/// 镜像 Go `core.IsDoor`:下半 62..69 加上半 70。
fn is_door(id: u16) -> bool {
    (DOOR_LOWER_SOUTH_CLOSED..=DOOR_UPPER).contains(&id)
}

/// 镜像 Go `core.IsDoorOpen` 对四个开启下半门的判定。
fn is_door_open_lower(id: u16) -> bool {
    matches!(
        id,
        DOOR_LOWER_SOUTH_OPEN | DOOR_LOWER_WEST_OPEN | DOOR_LOWER_NORTH_OPEN | DOOR_LOWER_EAST_OPEN
    )
}

/// 镜像 Go `fluid.Replaceable` 的判定表,分支顺序与 Go 侧一致:
/// 空气→真;上半门→假;开启下半门→真;关闭门→假;植物(作物、短草与树苗)→真;
/// 非流体→假;源→假;流体按等级比较(更弱的可被更强的新水替换)。
///
/// `new_level` 由调用方按当前传播算出(垂直恒为 1,水平为 N+1),本函数只做
/// 纯比较;作物与树苗冲毁后的掉落结算是权威写入侧(Go sim)的职责,短草覆盖
/// 零掉落且不预留容量——三者 kernel 都不感知。
/// 重扫 kernel 以 `replaceable(id, 1)` 复用本表做密封(不动点)判定。
pub(crate) fn replaceable(target: u16, new_level: u8) -> bool {
    if target == AIR {
        return true;
    }
    if is_door(target) {
        // 门:开启可流入(视作空气),关闭实心不可流入,上半视作实心。
        if target == DOOR_UPPER {
            return false;
        }
        return is_door_open_lower(target);
    }
    if is_plant(target) {
        return true;
    }
    if !is_fluid(target) {
        // 非空气、非植物、非流体、非可流入开启门:实心方块一律不可替换。
        return false;
    }
    if target == WATER_SOURCE {
        // 源的流体等级读作 0,若不特判会被 0 > new_level 误判;「源永不可
        // 替换」是一条独立规则,显式分支表达而不是靠等级比较凑对。
        return false;
    }
    fluid_level(target) > new_level
}

/// 镜像 Go `fluid.flowingSurvives`:上方是任意流体,或水平邻居中存在
/// 流体等级严格小于自身的流体(源的等级读作 0,天然满足「更小」)。
///
/// 只读槽位切片,不做任何写入——同 tick 内的存活判定全部只看 tick 起始
/// 状态,这是 Go 侧避免振荡的同一约束在 kernel 的体现。
fn flowing_survives(cells: &[u16; EVAL_SLOTS_PER_ITEM], self_id: u16) -> bool {
    let level = fluid_level(self_id);
    if is_fluid(cells[SLOT_ABOVE]) {
        return true;
    }
    HORIZONTAL_SLOTS
        .iter()
        .any(|&slot| is_fluid(cells[slot]) && fluid_level(cells[slot]) < level)
}

/// 水平四邻中源的数量：无限水升级的唯一判据（≥2 即升源）。
///
/// 只读槽位切片，与 `flowing_survives` 同为 tick 起始状态快照；计数本身不做
/// 写入判定，调用方按自格类型分两处使用（空气自格走陈旧例外，流动自格走
/// 垂直之后、水平之前）。
fn horizontal_source_count(cells: &[u16; EVAL_SLOTS_PER_ITEM]) -> usize {
    HORIZONTAL_SLOTS
        .iter()
        .filter(|&&slot| cells[slot] == WATER_SOURCE)
        .count()
}

/// 把一条候选写入编码进输出项的第 index 条槽位条目(3 字节)。
fn write_entry(out: &mut [u8; EVAL_ITEM_OUTPUT_BYTES], index: usize, slot: u8, id: u16) {
    let entry = &mut out[index * 3..index * 3 + 3];
    entry[0] = slot;
    entry[1..3].copy_from_slice(&id.to_le_bytes());
}

/// 对一项 7 格邻域执行单次规则求值,把至多 4 条候选写入编码进定长输出槽。
///
/// 写入形状与 Go `evalCell` 的返回 map 一一对应:陈旧项/等级 7 到界为空;
/// 非源消亡只有自格 1 条(写 `Air`);无限水只有自格 1 条(写源);垂直优先只有下方 1 条(写等级 1);
/// 水平传播至多 4 条(四个水平方向,槽位序 +x、−x、+z、−z)。已用槽位
/// 从 entry 0 起连续排布,其余为「无写入」哨兵(0xFF, 0x00, 0x00)。
pub(crate) fn eval_one(cells: &[u16; EVAL_SLOTS_PER_ITEM], out: &mut [u8; EVAL_ITEM_OUTPUT_BYTES]) {
    // 无写哨兵打底:每条 3 字节槽位为 (0xFF, 0x00, 0x00)。
    for entry in out.chunks_exact_mut(3) {
        entry[0] = SLOT_NO_WRITE;
        entry[1] = 0;
        entry[2] = 0;
    }

    let self_id = cells[SLOT_SELF];
    if !is_fluid(self_id) {
        // 队列里的格在真正被处理前可能已因外部原因变非流体;陈旧待更新项
        // 直接跳过,不产生变化(与「非源消亡写 Air」是两回事)。其中空气是
        // 无限水的唯一例外：水平四邻≥2源时自格升源；石头等实心与植物仍空写。
        // 空气自格无垂直传播，命中即升级返回。
        if self_id == AIR && horizontal_source_count(cells) >= 2 {
            write_entry(out, 0, SLOT_SELF as u8, WATER_SOURCE);
        }
        return;
    }

    if self_id != WATER_SOURCE {
        // 「源方块永不自然消失」+「流动方块失去支撑后消失」:只有非源
        // 流动格才需要存活判定;消亡时本格本 tick 只写 Air,不再传播。
        if !flowing_survives(cells, self_id) {
            write_entry(out, 0, SLOT_SELF as u8, AIR);
            return;
        }
    }

    // 「垂直优先」:下方可替换时只向下写最强流动水(等级 1),本次不再向
    // 任何水平方向传播。目标编号依赖 WaterSourceID..WaterLevel7ID 连续
    // 排布、WaterLevelN == WaterSource+N 的稳定约定(Go 侧同一条算式)。
    if replaceable(cells[SLOT_BELOW], 1) {
        write_entry(out, 0, SLOT_BELOW as u8, WATER_SOURCE + 1);
        return;
    }

    // 「无限水（流动水自格）」：水平四邻≥2源时自格升源并返回，优先级高于
    // 等级水，不改变垂直优先（垂直已在上方返回）。源自格永不重写自身；
    // 升级只发生在流动格，`replaceable` 判定表不动。
    if self_id != WATER_SOURCE && horizontal_source_count(cells) >= 2 {
        write_entry(out, 0, SLOT_SELF as u8, WATER_SOURCE);
        return;
    }

    // 「水平传播递减」+「水平传播上界」:下方不可替换时才水平扩散,等级
    // 从当前格等级 +1(源读作 0,其水平邻居因此得到等级 1);等级 7 已是
    // 传播下界,世界中不得出现等级 > 7 的流体方块。
    let next_level = fluid_level(self_id) + 1;
    if next_level > MAX_FLUID_LEVEL {
        return;
    }
    let next_id = WATER_SOURCE + u16::from(next_level);
    let mut count = 0;
    for slot in HORIZONTAL_SLOTS {
        if replaceable(cells[slot], next_level) {
            write_entry(out, count, slot as u8, next_id);
            count += 1;
        }
    }
}

/// 解析输入头部并校验长度契约:返回 item_count。
///
/// 违约(长度不足 8 字节、`layout_version != 1`、长度 != 8 + item_count×14、
/// 乘法/加法回绕)一律返回 `None`,由 FFI 层转为 `MORNLEA_STATUS_INPUT`。
pub(crate) fn parse_eval_input(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < EVAL_HEADER_BYTES {
        return None;
    }
    if read_u32(bytes, 0) != EVAL_LAYOUT_VERSION {
        return None;
    }
    let item_count = read_u32(bytes, 4) as usize;
    let expected = EVAL_HEADER_BYTES.checked_add(item_count.checked_mul(EVAL_ITEM_BYTES)?)?;
    if expected != bytes.len() {
        return None;
    }
    Some(item_count)
}

/// 读取第 index 项的 7 格槽位(输入已通过 `parse_eval_input` 校验)。
pub(crate) fn read_eval_item(bytes: &[u8], index: usize) -> [u16; EVAL_SLOTS_PER_ITEM] {
    let base = EVAL_HEADER_BYTES + index * EVAL_ITEM_BYTES;
    let mut cells = [0_u16; EVAL_SLOTS_PER_ITEM];
    for (slot, cell) in cells.iter_mut().enumerate() {
        let offset = base + slot * 2;
        *cell = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    }
    cells
}

/// 测试助手:按布局 v1 编码一项集合为完整输入字节流,供本模块与
/// `ffi.rs` 的 FFI 测试共用(ffi 侧金标准须与逐字节布局一致)。
#[cfg(test)]
pub(crate) fn encode_eval_input(items: &[[u16; EVAL_SLOTS_PER_ITEM]]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(EVAL_HEADER_BYTES + items.len() * EVAL_ITEM_BYTES);
    bytes.extend_from_slice(&EVAL_LAYOUT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for item in items {
        for cell in item {
            bytes.extend_from_slice(&cell.to_le_bytes());
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试专用钉位:生产规则只按区间判定,Stone/Grass/Workbench 只在测试里充当
    // 「非植物实心方块」样本(数值同为 core/block.go 的协议稳定值)。
    const STONE: u16 = 2;
    const GRASS: u16 = 4;
    const WORKBENCH: u16 = 45;

    /// 把一条输出项的 12 字节解码回 (槽位, BlockID) 列表(过滤哨兵)。
    fn decode_entries(bytes: &[u8; EVAL_ITEM_OUTPUT_BYTES]) -> Vec<(u8, u16)> {
        bytes
            .chunks_exact(3)
            .filter(|entry| entry[0] != SLOT_NO_WRITE)
            .map(|entry| (entry[0], u16::from_le_bytes([entry[1], entry[2]])))
            .collect()
    }

    /// 断言一项输出恰好是期望写集,且未用槽位是「无写入」哨兵。
    fn assert_writes(bytes: &[u8; EVAL_ITEM_OUTPUT_BYTES], want: &[(u8, u16)]) {
        assert_eq!(decode_entries(bytes), want.to_vec());
        for entry in bytes.chunks_exact(3).skip(want.len()) {
            assert_eq!(entry, &[SLOT_NO_WRITE, 0, 0]);
        }
    }

    fn eval(cells: &[u16; EVAL_SLOTS_PER_ITEM]) -> [u8; EVAL_ITEM_OUTPUT_BYTES] {
        let mut out = [0xFF_u8; EVAL_ITEM_OUTPUT_BYTES];
        eval_one(cells, &mut out);
        out
    }

    /// cells 数组按槽位序拼装:自格、上、下、+x、−x、+z、−z。
    fn cells(
        self_id: u16,
        above: u16,
        below: u16,
        pos_x: u16,
        neg_x: u16,
        pos_z: u16,
        neg_z: u16,
    ) -> [u16; EVAL_SLOTS_PER_ITEM] {
        [self_id, above, below, pos_x, neg_x, pos_z, neg_z]
    }

    #[test]
    fn block_id_constants_match_go_core() {
        // 方块编号钉位:这些数值与 Go `internal/core/block.go` 的 iota 实测值
        // 逐一对应(空气 0、Barrier 1、石头 2、源 27、流动水 28..34、麦
        // 37..44、工作台 45、马铃薯 46..53、胡萝卜 54..61、下半门 62..69、
        // 上半门 70、短草 84、树苗 89)。重排即破坏协议稳定契约,本断言负责在
        // Rust 侧被误改时报警。
        assert_eq!(AIR, 0);
        assert_eq!(BARRIER, 1);
        assert_eq!(WATER_SOURCE, 27);
        assert_eq!(WATER_SOURCE + 1, 28);
        assert_eq!(WATER_LEVEL_7, 34);
        assert_eq!(WATER_LEVEL_7 - WATER_SOURCE, 7);
        assert_eq!((WHEAT_STAGE_0, WHEAT_STAGE_7), (37, 44));
        assert_eq!(WORKBENCH, 45);
        assert_eq!((POTATO_STAGE_0, POTATO_STAGE_7), (46, 53));
        assert_eq!((CARROT_STAGE_0, CARROT_STAGE_7), (54, 61));
        assert_eq!(DOOR_LOWER_SOUTH_CLOSED, 62);
        assert_eq!(DOOR_UPPER, 70);
        assert_eq!(SHORT_GRASS, 84);
        assert_eq!(SAPLING, 89);
        assert_eq!(STONE, 2);
    }

    #[test]
    fn fluid_predicates_match_go_core() {
        assert!(is_fluid(WATER_SOURCE));
        assert!(is_fluid(WATER_LEVEL_7));
        assert!(!is_fluid(WATER_SOURCE - 1));
        assert!(!is_fluid(WATER_LEVEL_7 + 1));
        assert_eq!(fluid_level(WATER_SOURCE), 0);
        assert_eq!(fluid_level(WATER_SOURCE + 3), 3);
        assert_eq!(fluid_level(WATER_LEVEL_7), 7);
        // 作物三族各自覆盖,工作台(45)在麦与马铃薯之间必须不是作物。
        for id in [
            WHEAT_STAGE_0,
            WHEAT_STAGE_7,
            POTATO_STAGE_0,
            POTATO_STAGE_7,
            CARROT_STAGE_0,
            CARROT_STAGE_7,
        ] {
            assert!(is_crop(id));
            assert!(is_plant(id));
        }
        assert!(!is_crop(WORKBENCH));
        assert!(!is_crop(STONE));
        // 短草是植物但不是作物:植物共有语义(可替换、透光、零碰撞)收口在
        // `is_plant`,农业状态机仍只认 `is_crop`(镜像 Go `core.IsPlant`)。
        assert!(is_plant(SHORT_GRASS));
        assert!(!is_crop(SHORT_GRASS));
        // 树苗同理:第二个非作物植物方块(短草是第一个),与短草共用植物放行,
        // 但不进入任何作物分支。
        assert!(is_plant(SAPLING));
        assert!(!is_crop(SAPLING));
        assert!(!is_plant(WORKBENCH));
        assert!(!is_plant(STONE));
        // 门区间与四个开启下半门。
        assert!(is_door(DOOR_LOWER_SOUTH_CLOSED));
        assert!(is_door(DOOR_UPPER));
        assert!(!is_door(DOOR_UPPER + 1));
        assert!(!is_door(DOOR_LOWER_SOUTH_CLOSED - 1));
        for id in [
            DOOR_LOWER_SOUTH_OPEN,
            DOOR_LOWER_WEST_OPEN,
            DOOR_LOWER_NORTH_OPEN,
            DOOR_LOWER_EAST_OPEN,
        ] {
            assert!(is_door_open_lower(id));
        }
        assert!(!is_door_open_lower(DOOR_LOWER_SOUTH_CLOSED));
        assert!(!is_door_open_lower(DOOR_UPPER));
    }

    #[test]
    fn replaceable_table_mirrors_go() {
        // 空气可替换。
        assert!(replaceable(AIR, 1));
        // 作物可替换(水淹即冲毁,掉落结算在权威写入侧)。
        assert!(replaceable(WHEAT_STAGE_0, 3));
        assert!(replaceable(POTATO_STAGE_7, 1));
        assert!(replaceable(CARROT_STAGE_0 + 3, 7));
        // 短草可替换且零掉落:与作物同走植物放行,但 sim 写入侧不为它
        // 产出任何掉落物,也不预留掉落容量。
        assert!(replaceable(SHORT_GRASS, 1));
        assert!(replaceable(SHORT_GRASS, 7));
        // 树苗可替换:水淹即冲毁,掉落 1 个自身与容量原子拒绝都在 sim 写入侧。
        assert!(replaceable(SAPLING, 1));
        assert!(replaceable(SAPLING, 7));
        // 门:开启下半可流入,关闭下半与上半不可。
        assert!(replaceable(DOOR_LOWER_SOUTH_OPEN, 1));
        assert!(!replaceable(DOOR_LOWER_SOUTH_CLOSED, 1));
        assert!(!replaceable(DOOR_UPPER, 1));
        // 非植物实心方块不可替换;短草脚下的草方块(GRASS=4)是「清草不清
        // 草皮」的对照,必须继续挡水。
        assert!(!replaceable(STONE, 1));
        assert!(!replaceable(WORKBENCH, 7));
        assert!(!replaceable(GRASS, 1));
        assert!(!replaceable(GRASS, 7));
        // 源不可替换(显式分支,不靠等级比较凑对)。
        assert!(!replaceable(WATER_SOURCE, 7));
        // 流体按等级比较:更弱(等级更大)可被替换,相等或更强不可。
        assert!(replaceable(WATER_SOURCE + 5, 3));
        assert!(!replaceable(WATER_SOURCE + 3, 3));
        assert!(!replaceable(WATER_SOURCE + 2, 3));
    }

    #[test]
    fn source_writes_level_one_down_when_below_replaceable() {
        // 源格、下方空气:垂直优先,只写下方一条等级 1,不再水平传播。
        let out = eval(&cells(WATER_SOURCE, STONE, AIR, STONE, STONE, STONE, STONE));
        assert_writes(&out, &[(2, WATER_SOURCE + 1)]);
    }

    #[test]
    fn flowing_cell_surviving_on_above_fluid_writes_down() {
        // 非源流动格靠上方流体保活;下方为空气 → 同样垂直优先写等级 1
        // (垂直写入恒为最强流动水,与自身等级无关)。
        let out = eval(&cells(
            WATER_SOURCE + 4,
            WATER_SOURCE,
            AIR,
            STONE,
            STONE,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[(2, WATER_SOURCE + 1)]);
    }

    #[test]
    fn horizontal_spread_increments_level() {
        // 下方不可替换时水平扩散:自身等级 2 → 四邻居得到等级 3,
        // 槽位序 +x、−x、+z、−z(上方置源保活,存活判定见下方各测试)。
        let out = eval(&cells(
            WATER_SOURCE + 2,
            WATER_SOURCE,
            STONE,
            AIR,
            AIR,
            AIR,
            AIR,
        ));
        let next = WATER_SOURCE + 3;
        assert_writes(&out, &[(3, next), (4, next), (5, next), (6, next)]);
    }

    #[test]
    fn source_horizontal_spread_is_level_one() {
        // 源的等级读作 0,水平邻居得到等级 1(下方为门上半不可垂直写)。
        let out = eval(&cells(WATER_SOURCE, STONE, DOOR_UPPER, AIR, AIR, AIR, AIR));
        let next = WATER_SOURCE + 1;
        assert_writes(&out, &[(3, next), (4, next), (5, next), (6, next)]);
    }

    #[test]
    fn level_seven_does_not_spread() {
        // 等级 7 是传播下界:靠上方流体保活、下方与水平邻居均不可写 → 空写。
        let out = eval(&cells(
            WATER_LEVEL_7,
            WATER_SOURCE,
            STONE,
            STONE,
            STONE,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[]);
    }

    #[test]
    fn dying_flowing_cell_writes_air_to_self() {
        // 上方非流体、水平邻居无更强流体:非源流动格失去支撑,自格消亡写
        // Air,且不再传播(下方虽为空气也 Must Not 写)。
        let out = eval(&cells(
            WATER_SOURCE + 2,
            STONE,
            AIR,
            STONE,
            STONE,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[(0, AIR)]);
    }

    #[test]
    fn flowing_cell_survives_on_stronger_horizontal_neighbor() {
        // 水平邻居等级更小(更强,含源读 0)即存活;本格等级 3 向空气扩散等级 4。
        let out = eval(&cells(
            WATER_SOURCE + 3,
            STONE,
            STONE,
            WATER_SOURCE,
            AIR,
            AIR,
            AIR,
        ));
        let next = WATER_SOURCE + 4;
        assert_writes(&out, &[(4, next), (5, next), (6, next)]);
    }

    #[test]
    fn equal_level_horizontal_neighbor_does_not_save_cell() {
        // 水平邻居等级相等不算「更强」,本格仍消亡。
        let out = eval(&cells(
            WATER_SOURCE + 2,
            STONE,
            STONE,
            WATER_SOURCE + 2,
            STONE,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[(0, AIR)]);
    }

    #[test]
    fn stale_non_fluid_item_writes_nothing() {
        // 队列陈旧项:自格已变非流体,直接空写(不写 Air——那是消亡,不是陈旧)。
        let out = eval(&cells(STONE, WATER_SOURCE, AIR, AIR, AIR, AIR, AIR));
        assert_writes(&out, &[]);
    }

    #[test]
    fn crops_are_replaced_by_horizontal_spread() {
        // 水平传播可替换作物(小麦/马铃薯/胡萝卜任意阶段)。
        let out = eval(&cells(
            WATER_SOURCE + 2,
            WATER_SOURCE,
            STONE,
            WHEAT_STAGE_0,
            POTATO_STAGE_7,
            CARROT_STAGE_0 + 3,
            STONE,
        ));
        let next = WATER_SOURCE + 3;
        assert_writes(&out, &[(3, next), (4, next), (5, next)]);
    }

    #[test]
    fn short_grass_is_replaced_vertically_and_horizontally() {
        // 短草与作物同走植物放行:垂直优先命中下方短草,只写下方一条等级 1,
        // 不再向任何水平方向传播。
        let out = eval(&cells(
            WATER_SOURCE,
            STONE,
            SHORT_GRASS,
            STONE,
            STONE,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[(2, WATER_SOURCE + 1)]);
        // 水平分支:+x 与 −z 是短草可写入,−x 是短草脚下的草方块必须挡水,
        // +z 空气照常写入。
        let out = eval(&cells(
            WATER_SOURCE,
            STONE,
            STONE,
            SHORT_GRASS,
            GRASS,
            AIR,
            SHORT_GRASS,
        ));
        let next = WATER_SOURCE + 1;
        assert_writes(&out, &[(3, next), (5, next), (6, next)]);
    }

    #[test]
    fn sapling_is_replaced_vertically_and_horizontally() {
        // 树苗与作物、短草同走植物放行:垂直优先命中下方树苗,只写下方一条
        // 等级 1,不再向任何水平方向传播。
        let out = eval(&cells(
            WATER_SOURCE,
            STONE,
            SAPLING,
            STONE,
            STONE,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[(2, WATER_SOURCE + 1)]);
        // 水平分支:+x 与 −z 是树苗可写入,−x 是树苗脚下的草方块必须挡水,
        // +z 空气照常写入。
        let out = eval(&cells(
            WATER_SOURCE,
            STONE,
            STONE,
            SAPLING,
            GRASS,
            AIR,
            SAPLING,
        ));
        let next = WATER_SOURCE + 1;
        assert_writes(&out, &[(3, next), (5, next), (6, next)]);
    }

    #[test]
    fn door_states_gate_flow() {
        // 开启下半门视作空气可流入;关闭下半门与上半门视作实心不可。
        let out = eval(&cells(
            WATER_SOURCE + 2,
            WATER_SOURCE,
            STONE,
            DOOR_LOWER_SOUTH_OPEN,
            DOOR_LOWER_SOUTH_CLOSED,
            DOOR_UPPER,
            AIR,
        ));
        let next = WATER_SOURCE + 3;
        assert_writes(&out, &[(3, next), (6, next)]);
    }

    #[test]
    fn source_and_stronger_fluid_are_not_replaced() {
        // 水平邻居是源 → 不可替换;更弱流动水(等级更大)→ 可被强水替换。
        let out = eval(&cells(
            WATER_SOURCE + 2,
            STONE,
            STONE,
            WATER_SOURCE,
            WATER_SOURCE + 5,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[(4, WATER_SOURCE + 3)]);
    }

    #[test]
    fn vertical_write_only_when_below_replaceable() {
        // 下方是作物同样触发垂直优先(作物可替换),只写下方一条。
        let out = eval(&cells(
            WATER_SOURCE,
            STONE,
            WHEAT_STAGE_0 + 5,
            AIR,
            AIR,
            AIR,
            AIR,
        ));
        assert_writes(&out, &[(2, WATER_SOURCE + 1)]);
        // 下方是源 → 不可垂直写,转入水平扩散。
        let out = eval(&cells(
            WATER_SOURCE,
            STONE,
            WATER_SOURCE,
            AIR,
            STONE,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[(3, WATER_SOURCE + 1)]);
    }

    #[test]
    fn output_entries_pack_contiguously_with_sentinels() {
        // 逐字节布局:已用槽位从 entry 0 连续排布,未用槽位为 FF 00 00。
        let out = eval(&cells(
            WATER_SOURCE + 2,
            WATER_SOURCE,
            STONE,
            AIR,
            STONE,
            STONE,
            STONE,
        ));
        assert_eq!(
            out,
            [
                3, 0x1E, 0x00, // +x ← 等级 3 = 27+3 = 30 = 0x001E
                0xFF, 0x00, 0x00, 0xFF, 0x00, 0x00, 0xFF, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn infinite_water_two_sources_upgrade() {
        // 自格为空气，+x/−x 为源，其余为石头：按无限水应升源而非等级水。
        let out = eval(&cells(
            AIR,
            STONE,
            STONE,
            WATER_SOURCE,
            WATER_SOURCE,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[(0, WATER_SOURCE)]);
    }

    #[test]
    fn infinite_water_flowing_upgrade() {
        // 流动水自格、下方实心时双源夹流水应升源，优先级高于等级水。
        let out = eval(&cells(
            WATER_SOURCE + 3,
            STONE,
            STONE,
            WATER_SOURCE,
            WATER_SOURCE,
            AIR,
            AIR,
        ));
        assert_writes(&out, &[(0, WATER_SOURCE)]);
    }

    #[test]
    fn infinite_water_vertical_priority_preserved() {
        // 下方可替换时流动水仍只向下写等级 1，不升源（垂直优先不被覆盖）。
        let out = eval(&cells(
            WATER_SOURCE + 2,
            STONE,
            AIR,
            WATER_SOURCE,
            WATER_SOURCE,
            STONE,
            STONE,
        ));
        assert_writes(&out, &[(2, WATER_SOURCE + 1)]);
    }

    #[test]
    fn parse_eval_input_validates_layout_and_length() {
        let items = [cells(WATER_SOURCE, STONE, AIR, STONE, STONE, STONE, STONE)];
        let bytes = encode_eval_input(&items);
        assert_eq!(parse_eval_input(&bytes), Some(1));
        assert_eq!(parse_eval_input(&[]), None);
        assert_eq!(parse_eval_input(&bytes[..7]), None);
        // layout_version 违约。
        let mut wrong_version = bytes.clone();
        wrong_version[0..4].copy_from_slice(&2_u32.to_le_bytes());
        assert_eq!(parse_eval_input(&wrong_version), None);
        // 长度与 item_count 不一致(短/长)。
        assert_eq!(parse_eval_input(&bytes[..bytes.len() - 1]), None);
        let mut long = bytes.clone();
        long.push(0);
        assert_eq!(parse_eval_input(&long), None);
        // item_count 巨大到乘法回绕,必须拒绝而不是 panic。
        let mut huge = Vec::new();
        huge.extend_from_slice(&1_u32.to_le_bytes());
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_eval_input(&huge), None);
    }

    #[test]
    fn read_eval_item_decodes_slots_in_order() {
        let items = [
            cells(WATER_SOURCE, 1, 2, 3, 4, 5, 6),
            cells(WATER_LEVEL_7, 7, 8, 9, 10, 11, 12),
        ];
        let bytes = encode_eval_input(&items);
        assert_eq!(read_eval_item(&bytes, 0), [27, 1, 2, 3, 4, 5, 6]);
        assert_eq!(read_eval_item(&bytes, 1), [34, 7, 8, 9, 10, 11, 12]);
    }
}
