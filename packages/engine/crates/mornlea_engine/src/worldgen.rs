//! worldgen:世界生成的唯一生产内核。
//!
//! 本模块逐条镜像旧 Go `internal/worldgen` 实现:2D Perlin/fbm 高度图、
//! 地表分层(草/土/石/基岩/雪/沙/黏土/砂砾)、splitmix 系整数矿石哈希与
//! 8×8 候选格橡树。冻结语义要求与 change drop-go-test-oracles 删除 Go
//! oracle 前的迁移基线实现同种子逐位一致,该一致性由生产黑盒测试与
//! golden 字节锁锁定,因此:
//!
//! - 浮点只使用 IEEE 正确舍入的基本运算(加/乘/除/floor/截断转换),
//!   禁止 `mul_add` 与任何重结合;运算顺序与 Go 源码逐条对应。
//! - perm 表与材料 ID 由调用方(Go)传入:随机源语义与 block 注册表的
//!   所有权留在 Go,engine 不内置 RNG、不硬编码 BlockID。
//! - 整数运算全部按 Go 的补码回绕语义使用 wrapping 系列。

/// 世界 Y 下界,必须与 Go `core.MinY` 一致;header 校验强制相等。
pub(crate) const WORLD_MIN_Y: i32 = -64;
/// 世界 Y 上界(开区间),必须与 Go `core.MaxY` 一致;header 校验强制相等。
pub(crate) const WORLD_MAX_Y: i32 = 320;
/// 区块边长(X/Z 方向 16 格),与 Go `core.SectionSize` 一致。
pub(crate) const SECTION_SIZE: i32 = 16;
/// 区块世界坐标位移量,与 Go `core.SectionShift` 一致。
pub(crate) const SECTION_SHIFT: u32 = 4;
/// 单区块 dense 输出的 u16 数量:16×16×(320−(−64)) = 98304。
pub(crate) const CHUNK_VOLUME: usize =
    (SECTION_SIZE as usize) * (SECTION_SIZE as usize) * ((WORLD_MAX_Y - WORLD_MIN_Y) as usize);

// 地形常量,与 Go 版逐字一致。
const SEA_LEVEL: f64 = 64.0;
/// 海平面的整数 Y 值,注水判定用。单独写成 i32 常量而不是由 `SEA_LEVEL`
/// 转换,是为了避免在常量上下文里做浮点转换;`sea_level_constants_agree`
/// 测试钉死两者一致。
///
/// `pub(crate)`:engine ABI v6 起远环壳(lod 模块)的海平面钳制与注水判定
/// 必须引用同一个常量——两处各写一份 64 会在任一侧调整海平面时静默分叉
/// (fluid × 远环,变更 rust-engine-lod-shell 的 Ruling 22)。
pub(crate) const SEA_LEVEL_Y: i32 = 64;
const TERRAIN_AMP: f64 = 48.0;
const TERRAIN_SCALE: f64 = 1.0 / 256.0;
const OCTAVES: usize = 5;
const LACUNARITY: f64 = 2.0;
const GAIN: f64 = 0.5;
const SOIL_DEPTH: i32 = 4;

const SNOW_LINE: i32 = 88;
const SAND_LINE: i32 = 62;
const CLAY_NOISE_SCALE: f64 = 1.0 / 96.0;
const CLAY_NOISE_OFFSET_X: i32 = 417;
const CLAY_NOISE_OFFSET_Z: i32 = -193;
const CLAY_NOISE_THRESHOLD: f64 = 0.18;
const GRAVEL_NOISE_SCALE: f64 = 1.0 / 72.0;
const GRAVEL_NOISE_OFFSET_X: i32 = -271;
const GRAVEL_NOISE_OFFSET_Z: i32 = 613;
const GRAVEL_NOISE_THRESHOLD: f64 = 0.22;
const GRAVEL_MAX_DEPTH: i32 = 10;

const COAL_MAX_Y: i32 = 96;
const IRON_MAX_Y: i32 = 48;
const COAL_ODDS: u64 = 2048;
const IRON_ODDS: u64 = 4096;
const COAL_SALT: u64 = 0x9E37_79B9_7F4A_7C15;
const IRON_SALT: u64 = 0xC2B2_AE3D_27D4_EB4F;

const OAK_TREE_CELL_SHIFT: u32 = 3;
const OAK_TREE_SALT: u64 = 0xA24B_AED4_963E_E407;

/// 运行时树形几何(树苗长成橡树)的参数派生 salt,ASCII "SAPLTREE"。
///
/// 与 `OAK_TREE_SALT`(世界生成 8×8 候选格网格)和
/// `SHORT_GRASS_GENERATION_SALT`(自然短草列)完全独立:生长几何必须能从
/// 任意根坐标确定性派生,不能借用世界生成的候选格网格,否则同一坐标的
/// 生长结果会随区块生成顺序与候选格布局漂移。冻结后不得改动,改动即让
/// 既有世界里的树苗长成另一棵树。
const TREE_BLOCKS_SALT: u64 = 0x5341_504C_5452_4545;

/// 运行时树形几何写入的方块编号:协议稳定值,与 Go `core` 的 `OakLogID`
/// (17)与 `LeavesID`(19)逐一对应,只能追加不能重排。
const TREE_BLOCKS_OAK_LOG: u16 = 17;
const TREE_BLOCKS_LEAVES: u16 = 19;

/// 运行时树形几何的记录上限。
///
/// 最坏普通橡树(高 7、蓬松档)是 7 条原木 + 20+20+8+5+5 条树叶 = 65 条,
/// 128 是防御性上界:越过它说明层形实现已经偏离普通橡树家族,按输出溢出
/// 显式失败而不是静默截断。
const TREE_BLOCKS_MAX_RECORDS: usize = 128;

/// 自然短草列判定的冻结 salt(natural-grass-seeds design 决策 3)。
/// 只借用既有 `ore_hash` wrapping 整数哈希,不用全局 RNG、浮点概率或
/// 区块内坐标;`hash & 3 == 0` 给合格草地列恰 1/4 的独立稀疏命中,与玩家
/// 除草掉落的 salt 完全独立。
const SHORT_GRASS_GENERATION_SALT: u64 = 0x5348_4F52_5447_5253;

/// 调用方传入的方块材料表;engine 不硬编码任何 BlockID。
///
/// 15 项必须两两互异,**唯一例外是 `water` 允许等于 `air`**:air 是空判定
/// 哨兵,其余 ID 在分层/矿石/树/短草逻辑中参与等值比较或写入,重复 ID 会
/// 破坏与 Go 语义的对应关系,由 FFI 层拒绝。`short_grass` 参与装饰写入,
/// 不在豁免集合内——关闭注水的门控编码只豁免 `water == air` 这一对。
///
/// `water == air` 是 Go 侧 `fluidEnabled` 关闭时的门控编码(design D6):
/// water 只被写入、从不参与等值比较,填 air 编号即让注水步退化为把空气
/// 写回空气,生成结果与未引入流体的基线逐位一致,Rust 侧因此不需要任何
/// 开关分支。
#[derive(Clone, Copy)]
pub(crate) struct Materials {
    pub air: u16,
    pub stone: u16,
    pub dirt: u16,
    pub grass: u16,
    pub bedrock: u16,
    pub snow: u16,
    pub sand: u16,
    pub clay: u16,
    pub gravel: u16,
    pub iron_ore: u16,
    pub coal_ore: u16,
    pub oak_log: u16,
    pub leaves: u16,
    /// 海平面注水写入的方块;等于 `air` 时注水整体退化为空操作。
    pub water: u16,
    /// 草地表面装饰的短草方块;树与海水之后仍为空气的命中列才写入。
    pub short_grass: u16,
}

impl Materials {
    /// 运行时树形几何专用材料表:普通橡树(`rare = false`、`branch_count = 0`)
    /// 的层形判定只读 `air`、`oak_log`、`leaves` 三项,其余字段在运行时路径
    /// 不可达。
    ///
    /// 不可达字段一律填 1(既非空气也非原木/树叶):一旦后续改动让普通档读了
    /// 新字段,比较结果会立刻偏离测试预期,而不是静默取到 0 蒙混过关。三项
    /// 编号是协议稳定值,由 `runtime_block_ids_match_go_core` 钉位。
    const RUNTIME_TREE: Materials = Materials {
        air: 0,
        stone: 1,
        dirt: 1,
        grass: 1,
        bedrock: 1,
        snow: 1,
        sand: 1,
        clay: 1,
        gravel: 1,
        iron_ore: 1,
        coal_ore: 1,
        oak_log: TREE_BLOCKS_OAK_LOG,
        leaves: TREE_BLOCKS_LEAVES,
        water: 1,
        short_grass: 1,
    };

    /// 按 header 编码顺序展开为数组,供互异性校验使用。
    pub(crate) fn as_array(&self) -> [u16; 15] {
        [
            self.air,
            self.stone,
            self.dirt,
            self.grass,
            self.bedrock,
            self.snow,
            self.sand,
            self.clay,
            self.gravel,
            self.iron_ore,
            self.coal_ore,
            self.oak_log,
            self.leaves,
            self.water,
            self.short_grass,
        ]
    }
}

/// 单次 worldgen 调用的全部确定性输入:seed、材料表与 Go 播种的 perm 表。
pub(crate) struct WorldgenParams {
    pub seed: i64,
    pub materials: Materials,
    /// 512 项 Perlin 置换表;u8 取值域即合法域,索引 `perm[perm[i]+j]` 恒在界内。
    pub perm: [u8; 512],
}

/// Perlin 六次插值曲线 6t⁵−15t⁴+10t³,与 Go `fade` 逐条一致。
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + t * (b - a)
}

/// 从哈希低两位取 2D 梯度方向并与偏移做点积,与 Go `grad2` 一致。
fn grad2(h: u8, x: f64, y: f64) -> f64 {
    match h & 3 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        _ => -x - y,
    }
}

impl WorldgenParams {
    /// 2D Perlin 噪声,大致落在 [−1,1];运算顺序逐条镜像 Go `perlin.at`。
    fn noise_at(&self, x: f64, z: f64) -> f64 {
        let fx = x.floor();
        let fz = z.floor();
        // Go 侧为 `int(fx) & 255`:floor 后的 f64 截断为 64 位整数再取低 8 位;
        // 输入范围内截断不饱和,两侧结果一致。
        let xi = ((fx as i64) & 255) as usize;
        let zi = ((fz as i64) & 255) as usize;
        let xf = x - fx;
        let zf = z - fz;
        let u = fade(xf);
        let v = fade(zf);

        let perm = &self.perm;
        let aa = perm[perm[xi] as usize + zi];
        let ab = perm[perm[xi] as usize + zi + 1];
        let ba = perm[perm[xi + 1] as usize + zi];
        let bb = perm[perm[xi + 1] as usize + zi + 1];

        let x1 = lerp(grad2(aa, xf, zf), grad2(ba, xf - 1.0, zf), u);
        let x2 = lerp(grad2(ab, xf, zf - 1.0), grad2(bb, xf - 1.0, zf - 1.0), u);
        lerp(x1, x2, v)
    }

    /// 分形布朗运动,倍频叠加顺序与 Go `fbm` 一致(sum/norm 的除法最后执行)。
    fn fbm(&self, x: f64, z: f64) -> f64 {
        let mut sum = 0.0f64;
        let mut norm = 0.0f64;
        let mut amp = 1.0f64;
        let mut freq = 1.0f64;
        for _ in 0..OCTAVES {
            sum += self.noise_at(x * freq, z * freq) * amp;
            norm += amp;
            freq *= LACUNARITY;
            amp *= GAIN;
        }
        sum / norm
    }

    /// 世界坐标 (wx,wz) 处最高实心方块的 Y,与 Go `HeightAt` 一致(不截断上限)。
    pub(crate) fn height_at(&self, wx: i32, wz: i32) -> i32 {
        let n = self.fbm(f64::from(wx) * TERRAIN_SCALE, f64::from(wz) * TERRAIN_SCALE);
        // Go 为 int32(seaLevel + n*terrainAmp):f64 截断向零;高度域远离 i32 界,
        // 截断不饱和。
        (SEA_LEVEL + n * TERRAIN_AMP) as i32
    }

    /// 基础地层判定,与 Go `terrainBlockAt`(自由函数)一致。
    fn terrain_layer(&self, y: i32, height: i32) -> u16 {
        let m = &self.materials;
        if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) || y > height {
            m.air
        } else if y == WORLD_MIN_Y {
            m.bedrock
        } else if y == height {
            m.grass
        } else if y > height - SOIL_DEPTH {
            m.dirt
        } else {
            m.stone
        }
    }

    /// 自然材料分层(雪/沙/黏土/砂砾),与 Go `naturalBlockAt` 一致。
    ///
    /// 坐标加偏移按 Go int32 补码回绕语义使用 wrapping_add。
    fn natural_block_at(&self, x: i32, y: i32, z: i32, height: i32) -> u16 {
        let m = self.materials;
        let base = self.terrain_layer(y, height);
        if base == m.air || base == m.bedrock {
            return base;
        }

        let depth = height - y;
        if depth == 0 && height >= SNOW_LINE {
            return m.snow;
        }
        if height <= SAND_LINE && (0..SOIL_DEPTH).contains(&depth) {
            if depth >= 2
                && self.noise_at(
                    f64::from(x.wrapping_add(CLAY_NOISE_OFFSET_X)) * CLAY_NOISE_SCALE,
                    f64::from(z.wrapping_add(CLAY_NOISE_OFFSET_Z)) * CLAY_NOISE_SCALE,
                ) > CLAY_NOISE_THRESHOLD
            {
                return m.clay;
            }
            return m.sand;
        }
        if base == m.stone
            && depth <= GRAVEL_MAX_DEPTH
            && self.noise_at(
                f64::from(x.wrapping_add(GRAVEL_NOISE_OFFSET_X)) * GRAVEL_NOISE_SCALE,
                f64::from(z.wrapping_add(GRAVEL_NOISE_OFFSET_Z)) * GRAVEL_NOISE_SCALE,
            ) > GRAVEL_NOISE_THRESHOLD
        {
            return m.gravel;
        }
        base
    }

    /// 地层 + 矿石替换,与 Go `generatedBlockAt` 一致:矿石只替换石头,铁优先于煤。
    fn generated_block_at(&self, x: i32, y: i32, z: i32, height: i32) -> u16 {
        let m = self.materials;
        let base = self.natural_block_at(x, y, z, height);
        if base != m.stone {
            return base;
        }
        if y < IRON_MAX_Y && ore_hash(self.seed, x, y, z, IRON_SALT).is_multiple_of(IRON_ODDS) {
            return m.iron_ore;
        }
        if y < COAL_MAX_Y && ore_hash(self.seed, x, y, z, COAL_SALT).is_multiple_of(COAL_ODDS) {
            return m.coal_ore;
        }
        base
    }

    /// 单点地形查询,与 Go `TerrainBlockAt` 一致:Y 界外为 air,高度截断到 MaxY−1。
    pub(crate) fn terrain_block_at(&self, x: i32, y: i32, z: i32) -> u16 {
        if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
            return self.materials.air;
        }
        let mut height = self.height_at(x, z);
        if height >= WORLD_MAX_Y {
            height = WORLD_MAX_Y - 1;
        }
        self.generated_block_at(x, y, z, height)
    }

    /// 单点基础方块查询:地形非空优先,空气处叠加橡树,仍为空气时叠加海水,
    /// 最终仍是空气的格才查询自然短草。
    ///
    /// 层叠顺序与 `generate_chunk` 冻结一致:地形 → 橡树 → 海水 → 短草。
    /// 树或海水在当前格产生非空气即早返回,只有最终空气才进入短草判定,
    /// 因此短草绝不可能覆盖既有内容;`TerrainBlockAt` 与 `HeightAt` 不经
    /// 本函数,天然忽略装饰层。
    pub(crate) fn base_block_at(&self, x: i32, y: i32, z: i32) -> u16 {
        let base = self.terrain_block_at(x, y, z);
        if base != self.materials.air {
            return base;
        }
        let tree = self.tree_block_at(x, y, z);
        if tree != self.materials.air {
            return tree;
        }
        let sea = self.sea_block_at(y);
        if sea != self.materials.air {
            return sea;
        }
        self.short_grass_block_at(x, y, z)
    }

    /// 海平面注水的单点形式:世界高度范围内且 `y <= SEA_LEVEL_Y` 时为 water,
    /// 否则为 air。Y 界外不注水,与 `generate_chunk` 只写 `[MIN_Y, MAX_Y)` 一致。
    fn sea_block_at(&self, y: i32) -> u16 {
        if (WORLD_MIN_Y..=SEA_LEVEL_Y).contains(&y) {
            self.materials.water
        } else {
            self.materials.air
        }
    }

    /// 列 (wx,wz) 的截断地表高度:与 `generate_chunk` 的写入高度一致,
    /// 高度图越上界时截到 `WORLD_MAX_Y - 1`。
    fn truncated_surface(&self, wx: i32, wz: i32) -> i32 {
        let mut height = self.height_at(wx, wz);
        if height >= WORLD_MAX_Y {
            height = WORLD_MAX_Y - 1;
        }
        height
    }

    /// 自然短草的单点形式:调用方已确认地形/树/海水层都返回空气。
    ///
    /// 判定条件与 `apply_short_grass` 冻结一致:目标格是截断地表的 +1、
    /// 地表方块是 `grass`、`ore_hash(seed, wx, 0, wz, salt) & 3 == 0`。
    /// 树结构只写在 surface+1 及以上、海水只改写空气格,因此这里的
    /// `generated_block_at(surface) == grass` 与整块路径里对 dense 数组
    /// 的最终值检查逐格等价;传截断高度作 `height` 与 `generate_chunk`
    /// 的写入参数同源,避免越上界地层的分叉。
    fn short_grass_block_at(&self, x: i32, y: i32, z: i32) -> u16 {
        if ore_hash(self.seed, x, 0, z, SHORT_GRASS_GENERATION_SALT) & 3 != 0 {
            return self.materials.air;
        }
        let surface = self.truncated_surface(x, z);
        if y != surface + 1 || !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&surface) {
            return self.materials.air;
        }
        if self.generated_block_at(x, surface, z, surface) != self.materials.grass {
            return self.materials.air;
        }
        self.materials.short_grass
    }

    /// 返回固定候选格中的有效橡树,与 Go `oakTreeForCell` 一致。
    ///
    /// 同一 `OAK_TREE_SALT` 哈希的不交位域各自确定一项特征,不引入新噪声:
    /// 0 位是生成门槛(偶数生成)、1..3 位是根 X 格内偏移、4..6 位是根 Z
    /// 格内偏移、7..13 位是普通树高(7 比特 `% 3` 给 43/43/42,近似均匀取
    /// 满 5..7)、14..21 位是珍异门槛(`< 21`,21/256≈8.2%)、第 22 位是冠形
    /// 档(置位为蓬松)、23..25 位是珍异树高(`8 + 位域 % 5`,取 8..12)、
    /// 第 26 位是分杈条数(置位为两条)、27..30 位是两条分杈方向。位域两两
    /// 不交,树高、冠形、珍异判定相互独立确定。
    ///
    /// 有效性校验使用未截断的 surface 高度,顺序:根格必须是草、树冠不越界
    /// (树冠最高到顶上第二层)、树干路径必须全空。分杈不参与有效性校验:
    /// 分杈只替换原始空气,撞上固体的格在落笔时跳过(见 `apply_oak_trees`
    /// 与 `base_block_at` 的层叠顺序),整棵树不因此作废。
    fn oak_tree_for_cell(&self, cell_x: i32, cell_z: i32) -> Option<OakTree> {
        let hash = ore_hash(self.seed, cell_x, 0, cell_z, OAK_TREE_SALT);
        if hash & 1 != 0 {
            return None;
        }
        let x = (cell_x << OAK_TREE_CELL_SHIFT).wrapping_add(((hash >> 1) & 7) as i32);
        let z = (cell_z << OAK_TREE_CELL_SHIFT).wrapping_add(((hash >> 4) & 7) as i32);
        let rare = ((hash >> 14) & 0xFF) < 21;
        let height = if rare {
            (8 + ((hash >> 23) & 7) % 5) as i32
        } else {
            (5 + ((hash >> 7) & 0x7F) % 3) as i32
        };
        let fluffy = !rare && (hash >> 22) & 1 == 1;
        let branch_count = if rare {
            (1 + ((hash >> 26) & 1)) as u8
        } else {
            0
        };
        let branch_dir = [((hash >> 27) & 3) as u8, ((hash >> 29) & 3) as u8];
        let surface = self.height_at(x, z);
        let root_y = surface + 1;
        if self.generated_block_at(x, surface, z, surface) != self.materials.grass
            || root_y + height + 1 >= WORLD_MAX_Y
        {
            return None;
        }
        for y in root_y..root_y + height {
            if self.generated_block_at(x, y, z, surface) != self.materials.air {
                return None;
            }
        }
        Some(OakTree {
            root_x: x,
            root_y,
            root_z: z,
            height,
            fluffy,
            rare,
            branch_count,
            branch_dir,
        })
    }

    /// 单点橡树查询:合并全部可能覆盖 (x,y,z) 的候选树,原木优先,与 Go
    /// `treeBlockAt` 的 cellZ 外层、cellX 内层遍历顺序一致。
    ///
    /// 邻域半径取 3:珍异大冠旁侧突出主干 3 格、分杈横向伸 3 格,任一候选
    /// 的影响都落在此半径内;半径不足会让跨界树在单点与整块之间分叉。
    fn tree_block_at(&self, x: i32, y: i32, z: i32) -> u16 {
        let m = self.materials;
        let mut leaf = false;
        let cell_z_min = z.wrapping_sub(3) >> OAK_TREE_CELL_SHIFT;
        let cell_z_max = z.wrapping_add(3) >> OAK_TREE_CELL_SHIFT;
        let cell_x_min = x.wrapping_sub(3) >> OAK_TREE_CELL_SHIFT;
        let cell_x_max = x.wrapping_add(3) >> OAK_TREE_CELL_SHIFT;
        for cell_z in cell_z_min..=cell_z_max {
            for cell_x in cell_x_min..=cell_x_max {
                let Some(tree) = self.oak_tree_for_cell(cell_x, cell_z) else {
                    continue;
                };
                let block = oak_tree_block_at(&tree, &self.materials, x, y, z);
                if block == m.oak_log {
                    return m.oak_log;
                }
                if block == m.leaves {
                    leaf = true;
                }
            }
        }
        if leaf { m.leaves } else { m.air }
    }

    /// 生成整区块 dense 数组,布局 `[y−min_y][lz][lx]`,与 Go `GenerateChunk`
    /// 的写入集合逐位一致:地形只写到截断后的地表高度,其余保持 air。
    ///
    /// 生成顺序冻结为:terrain/ores → `apply_oak_trees` → `flood_sea_level`
    /// → `apply_short_grass`。短草层排在最后,只把"树与海水结算后仍是空气"
    /// 的命中草地列改为 `short_grass`,不触碰任何既有非空气方块。
    pub(crate) fn generate_chunk(&self, chunk_x: i32, chunk_z: i32, dense: &mut [u16]) {
        debug_assert_eq!(dense.len(), CHUNK_VOLUME);
        dense.fill(self.materials.air);
        let base_x = chunk_x.wrapping_shl(SECTION_SHIFT);
        let base_z = chunk_z.wrapping_shl(SECTION_SHIFT);

        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let wx = base_x.wrapping_add(lx);
                let wz = base_z.wrapping_add(lz);
                let h = self.truncated_surface(wx, wz);
                for y in WORLD_MIN_Y..=h {
                    dense[dense_index(lx, y, lz)] = self.generated_block_at(wx, y, wz, h);
                }
            }
        }
        self.apply_oak_trees(chunk_x, chunk_z, dense);
        self.flood_sea_level(dense);
        self.apply_short_grass(chunk_x, chunk_z, dense);
    }

    /// 自然短草装饰层:恰好遍历区块 16×16 = 256 个世界列,每列一次常数
    /// 哈希判定。命中(`hash & 3 == 0`)且地表是最终 `grass`、目标格仍是
    /// 空气时,在 surface+1 写 `short_grass`。
    ///
    /// 判定只依赖世界种子与世界坐标(世界 X/Z、固定 Y=0),不用区块内坐标
    /// 或邻区块状态,因此负坐标、区块边界与生成顺序下结果恒定;与
    /// `short_grass_block_at` 的单点路径逐格一致。
    fn apply_short_grass(&self, chunk_x: i32, chunk_z: i32, dense: &mut [u16]) {
        let m = self.materials;
        let base_x = chunk_x.wrapping_shl(SECTION_SHIFT);
        let base_z = chunk_z.wrapping_shl(SECTION_SHIFT);
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let wx = base_x.wrapping_add(lx);
                let wz = base_z.wrapping_add(lz);
                let surface = self.truncated_surface(wx, wz);
                if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&surface) {
                    continue;
                }
                // 地表最终方块必须是 grass:树只写在 surface+1 及以上、
                // 海水只改写空气格,该格在树/水之后不可能再变化。
                if dense[dense_index(lx, surface, lz)] != m.grass {
                    continue;
                }
                let target = surface + 1;
                if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&target)
                    || dense[dense_index(lx, target, lz)] != m.air
                {
                    continue;
                }
                if ore_hash(self.seed, wx, 0, wz, SHORT_GRASS_GENERATION_SALT) & 3 == 0 {
                    dense[dense_index(lx, target, lz)] = m.short_grass;
                }
            }
        }
    }

    /// 海平面注水:把海平面及以下**仍为空气**的格改写为 `materials.water`。
    ///
    /// 必须排在 `apply_oak_trees` 之后:注水只填最终空气格,不参与任何分层、
    /// 矿石或树木判定,因此这三者的生成结果逐位不变。
    ///
    /// 无门控分支:Go 侧关闭 `fluidEnabled` 时 `materials.water == materials.air`,
    /// 本步逐格把空气写回空气,输出与未引入流体的基线逐位一致(design D6)。
    fn flood_sea_level(&self, dense: &mut [u16]) {
        let m = self.materials;
        const LAYER_CELLS: usize = (SECTION_SIZE as usize) * (SECTION_SIZE as usize);
        for y in WORLD_MIN_Y..=SEA_LEVEL_Y {
            let layer = (y - WORLD_MIN_Y) as usize * LAYER_CELLS;
            for cell in &mut dense[layer..layer + LAYER_CELLS] {
                if *cell == m.air {
                    *cell = m.water;
                }
            }
        }
    }

    /// 把覆盖当前区块的有效候选树写入 dense 数组,与 Go `applyOakTrees` 一致:
    /// 树按 cellZ 外层、cellX 内层顺序应用;单棵树按 y/z/x 顺序写入;
    /// 原木可覆盖空气与树叶,树叶仅覆盖空气。
    ///
    /// 落笔盒取根 ±3、顶上两层:珍异大冠与分杈的最大水平伸展都是 3 格,
    /// 蓬松顶与大冠最高到顶上第二层;盒外不可能有本树的方块,盒内越界 Y
    /// 逐格跳过。原木(含分杈)只覆盖空气与树叶、树叶只覆盖空气,因此固体
    /// 地形永远不被改写,分杈撞上固体时自然截断。
    fn apply_oak_trees(&self, chunk_x: i32, chunk_z: i32, dense: &mut [u16]) {
        let m = self.materials;
        let base_x = chunk_x.wrapping_shl(SECTION_SHIFT);
        let base_z = chunk_z.wrapping_shl(SECTION_SHIFT);
        let cell_z_min = base_z.wrapping_sub(3) >> OAK_TREE_CELL_SHIFT;
        let cell_z_max = base_z.wrapping_add(SECTION_SIZE + 2) >> OAK_TREE_CELL_SHIFT;
        let cell_x_min = base_x.wrapping_sub(3) >> OAK_TREE_CELL_SHIFT;
        let cell_x_max = base_x.wrapping_add(SECTION_SIZE + 2) >> OAK_TREE_CELL_SHIFT;
        for cell_z in cell_z_min..=cell_z_max {
            for cell_x in cell_x_min..=cell_x_max {
                let Some(tree) = self.oak_tree_for_cell(cell_x, cell_z) else {
                    continue;
                };
                for y in tree.root_y..=tree.root_y + tree.height + 1 {
                    for z in tree.root_z.wrapping_sub(3)..=tree.root_z.wrapping_add(3) {
                        for x in tree.root_x.wrapping_sub(3)..=tree.root_x.wrapping_add(3) {
                            // 与 Go `pos.Chunk() != chunk.Pos` 判定等价:
                            // 世界坐标算术右移 4 即 floor 除 16。
                            if (x >> SECTION_SHIFT) != chunk_x
                                || (z >> SECTION_SHIFT) != chunk_z
                                || !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y)
                            {
                                continue;
                            }
                            let block = oak_tree_block_at(&tree, &m, x, y, z);
                            if block == m.air {
                                continue;
                            }
                            let index =
                                dense_index(x & (SECTION_SIZE - 1), y, z & (SECTION_SIZE - 1));
                            let current = dense[index];
                            if block == m.oak_log && (current == m.air || current == m.leaves) {
                                dense[index] = block;
                            }
                            if block == m.leaves && current == m.air {
                                dense[index] = block;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 候选橡树:根方块世界坐标、树干高度与哈希确定的形态特征。
///
/// `fluffy` 只对普通树有意义(珍异树恒为球状大冠);`branch_count` 为 0
/// 表示无分杈(全部普通树),珍异树取 1..2;`branch_dir` 的两个方向编码按
/// `branch_offset` 解释,第二条分杈不存在时其方向位被忽略。
struct OakTree {
    root_x: i32,
    root_y: i32,
    root_z: i32,
    height: i32,
    fluffy: bool,
    rare: bool,
    branch_count: u8,
    branch_dir: [u8; 2],
}

/// 分杈方向编码的水平偏移:0/+X、1/−X、2/+Z、3/−Z。
///
/// 位域外的取值不可能出现(调用方只传入哈希低两位);`& 3` 是防御性收敛,
/// 保证越界输入仍映射到合法水平方向而不是 panic。
fn branch_offset(dir: u8) -> (i32, i32) {
    match dir & 3 {
        0 => (1, 0),
        1 => (-1, 0),
        2 => (0, 1),
        _ => (0, -1),
    }
}

/// 树形在指定世界坐标的方块,树干优先于树叶,与 Go `oakTreeBlockAt` 一致。
///
/// 普通树冠是冻结的四层:顶下两层去角 5×5、顶层满 3×3、顶上一层十字;
/// 蓬松档在顶上再加一层去角 3×3(3×3 去角后恰为十字,形状与顶上层同)。
/// 珍异大冠是以树干顶为中心的多层去角方形叠加:底宽层去角 7×7、中层满
/// 5×5、上层满 3×3、顶十字,旁侧突出主干 3 格。分杈是横向原木段,第 `i`
/// 条长在顶下 `4 + i` 层、从主干向 `branch_dir[i]` 方向伸 3 格;分杈与树干
/// 同为原木优先级,但是否落笔由调用方按原始空气过滤(见 `apply_oak_trees`
/// 与 `base_block_at`),本函数只做形状判定。
fn oak_tree_block_at(tree: &OakTree, m: &Materials, x: i32, y: i32, z: i32) -> u16 {
    if tree.root_y < WORLD_MIN_Y || tree.root_y + tree.height + 1 >= WORLD_MAX_Y {
        return m.air;
    }
    let top_y = tree.root_y + tree.height - 1;
    if x == tree.root_x && z == tree.root_z && (tree.root_y..=top_y).contains(&y) {
        return m.oak_log;
    }
    for i in 0..tree.branch_count {
        // 第 `i` 条分杈长在顶下 `4 + i` 层:顺轴投影 1..=3、侧偏为零才命中,
        // 对角格天然被排除,分杈是严格的横向单列。
        let (dx, dz) = branch_offset(tree.branch_dir[i as usize]);
        if y != top_y - 4 - i32::from(i) {
            continue;
        }
        let along = x
            .wrapping_sub(tree.root_x)
            .wrapping_mul(dx)
            .wrapping_add(z.wrapping_sub(tree.root_z).wrapping_mul(dz));
        let side = z
            .wrapping_sub(tree.root_z)
            .wrapping_mul(dx)
            .wrapping_sub(x.wrapping_sub(tree.root_x).wrapping_mul(dz));
        if (1..=3).contains(&along) && side == 0 {
            return m.oak_log;
        }
    }
    let dx = x.wrapping_sub(tree.root_x).abs();
    let dz = z.wrapping_sub(tree.root_z).abs();
    // Pointwise probe queries may hand in a Y outside the world range before
    // any layer guard runs; wrapping subtraction keeps debug overflow checks
    // quiet and mirrors Go int32 wrap semantics, so extreme Y simply misses
    // every crown layer and resolves to air.
    if tree.rare {
        return match y.wrapping_sub(top_y) {
            -3 if dx <= 2 && dz <= 2 && !(dx == 2 && dz == 2) => m.leaves,
            -2 | -1 if dx <= 3 && dz <= 3 && !(dx == 3 && dz == 3) => m.leaves,
            0 if dx <= 2 && dz <= 2 => m.leaves,
            1 if dx <= 1 && dz <= 1 => m.leaves,
            2 if dx + dz <= 1 => m.leaves,
            _ => m.air,
        };
    }
    match y.wrapping_sub(top_y) {
        -2 | -1 if dx <= 2 && dz <= 2 && !(dx == 2 && dz == 2) => m.leaves,
        0 if dx <= 1 && dz <= 1 => m.leaves,
        1 if dx + dz <= 1 => m.leaves,
        2 if tree.fluffy && dx + dz <= 1 => m.leaves,
        _ => m.air,
    }
}

/// 用世界种子、三维坐标和 salt 生成稳定 64 位混合值,与 Go `oreHash` 一致。
///
/// Go 侧表达式 `hash ^= uint64(v) + K + hash<<6 + hash>>2` 为一串 uint64
/// 回绕加法后再异或,此处逐项用 wrapping_add 镜像。
fn ore_hash(seed: i64, x: i32, y: i32, z: i32, salt: u64) -> u64 {
    let mut hash = (seed as u64) ^ salt;
    for value in [i64::from(x), i64::from(y), i64::from(z)] {
        hash ^= (value as u64)
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(hash << 6)
            .wrapping_add(hash >> 2);
        hash = hash.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        hash ^= hash >> 33;
    }
    hash = hash.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    hash ^= hash >> 33;
    hash
}

/// dense 数组下标:`[y−min_y][lz][lx]` 布局,y 在外层便于 Go 顺序回写。
pub(crate) fn dense_index(lx: i32, y: i32, lz: i32) -> usize {
    let layer = (y - WORLD_MIN_Y) as usize;
    layer * (SECTION_SIZE as usize) * (SECTION_SIZE as usize)
        + (lz as usize) * (SECTION_SIZE as usize)
        + lx as usize
}

// ---- 运行时树形几何(树苗长成橡树) ----
//
// `mornlea_tree_blocks` 的带内契约:输入 28 字节 `MTB1` magic(4) +
// layout u32 LE(4,必须 1) + 世界种子 i64 LE(8) + 根坐标 x/y/z i32 LE(12);
// 输出 `count u32` LE + 每条 8 字节 `dx i8 | dy i8 | dz i8 | reserved u8 |
// block u16 LE | reserved u16`。记录是相对根格的偏移,根格自身(偏移全零)
// 是树干底。布局与世界生成共用的 `MGW1` header 无关:本入口只吃种子与根
// 坐标,不读 perm、材料表或区块坐标。

/// 运行时树形几何的输入字节数。
pub(crate) const TREE_BLOCKS_INPUT_BYTES: usize = 28;
/// 输入布局版本,唯一合法值。
const TREE_BLOCKS_LAYOUT: u32 = 1;
/// 单条记录字节数。
pub(crate) const TREE_BLOCKS_RECORD_BYTES: usize = 8;
/// 输出头部字节数:`count u32`。
pub(crate) const TREE_BLOCKS_COUNT_BYTES: usize = 4;
/// 输出静态最大字节数:头部 + 记录上限 × 单条长度。
pub(crate) const TREE_BLOCKS_MAX_OUTPUT_BYTES: usize =
    TREE_BLOCKS_COUNT_BYTES + TREE_BLOCKS_MAX_RECORDS * TREE_BLOCKS_RECORD_BYTES;
/// 根坐标 Y 的上界(含)。最坏普通橡树高 7、顶格在 `root_y + 8`,因此根格
/// 必须低到让最坏几何完整落在 `[WORLD_MIN_Y, WORLD_MAX_Y)` 内;不满足即
/// 按输入越界拒绝,而不是返回被截断的几何。
const TREE_BLOCKS_MAX_ROOT_Y: i32 = WORLD_MAX_Y - 9;

/// 运行时树形几何请求:世界种子与树苗根坐标。
pub(crate) struct TreeBlocksRequest {
    pub seed: i64,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// 运行时树形几何的一条记录:相对根坐标的偏移与方块编号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TreeBlock {
    pub dx: i8,
    pub dy: i8,
    pub dz: i8,
    pub block: u16,
}

/// 解析运行时树形几何输入;任何违约返回 None(FFI 层转为 StatusInput)。
///
/// 校验项:长度精确等于 28、magic `MTB1`、layout 等于 1、根坐标 Y 落在
/// `[WORLD_MIN_Y, TREE_BLOCKS_MAX_ROOT_Y]`,以及 X/Z 的 ±2 邻域不越出 i32
/// 值域(否则几何坐标加法会回绕)。X/Z 本身无世界边界约束——它们只参与
/// 哈希,几何偏移与绝对坐标无关。
pub(crate) fn parse_tree_blocks_input(bytes: &[u8]) -> Option<TreeBlocksRequest> {
    if bytes.len() != TREE_BLOCKS_INPUT_BYTES
        || &bytes[0..4] != b"MTB1"
        || read_u32(bytes, 4) != TREE_BLOCKS_LAYOUT
    {
        return None;
    }
    let request = TreeBlocksRequest {
        seed: read_i64(bytes, 8),
        x: read_i32(bytes, 16),
        y: read_i32(bytes, 20),
        z: read_i32(bytes, 24),
    };
    // 几何要取根 ±2 的水平邻域;坐标贴近 i32 边界时加法会回绕,回绕后的
    // 坐标虽然仍是合法 i32,却已经不是调用方给的那棵树,按越界坐标拒绝。
    let neighborhood_fits = request.x.checked_add(2).is_some()
        && request.x.checked_sub(2).is_some()
        && request.z.checked_add(2).is_some()
        && request.z.checked_sub(2).is_some();
    let fits_world_height = (WORLD_MIN_Y..=TREE_BLOCKS_MAX_ROOT_Y).contains(&request.y);
    (neighborhood_fits && fits_world_height).then_some(request)
}

/// 由 `TREE_BLOCKS_SALT` 从 (世界种子, 根坐标) 派生普通橡树参数。
///
/// 高度取 `5 + hash % 3`(5..7),蓬松位取另一段位域;珍异与分杈恒关,
/// 因此层形只走 `oak_tree_block_at` 的普通档。
fn runtime_oak_tree(request: &TreeBlocksRequest) -> OakTree {
    let hash = ore_hash(
        request.seed,
        request.x,
        request.y,
        request.z,
        TREE_BLOCKS_SALT,
    );
    OakTree {
        root_x: request.x,
        root_y: request.y,
        root_z: request.z,
        height: (5 + hash % 3) as i32,
        fluffy: (hash >> 3) & 1 == 1,
        rare: false,
        branch_count: 0,
        branch_dir: [0, 0],
    }
}

/// 计算运行时树形几何:相对根坐标的方块偏移列表,根格自身为树干底。
///
/// 层形复用 `oak_tree_block_at` 的普通档(`rare = false`、`branch_count = 0`),
/// 与世界生成的树冠共用同一份实现,不产生珍异巨树或分杈。遍历序固定为
/// dy 外层、dz 中层、dx 内层,记录顺序因此完全由输入决定。
///
/// Traverse runtime tree block geometry and emit each block in dy/dz/dx order.
///
/// Returns None if the emitter returns false or the 128-record bound is exceeded.
pub(crate) fn visit_tree_blocks(
    request: &TreeBlocksRequest,
    mut emit: impl FnMut(TreeBlock) -> bool,
) -> Option<usize> {
    let tree = runtime_oak_tree(request);
    let materials = Materials::RUNTIME_TREE;
    let mut count = 0;
    for dy in 0..=tree.height + 1 {
        for dz in -2..=2 {
            for dx in -2..=2 {
                let block = oak_tree_block_at(
                    &tree,
                    &materials,
                    request.x + dx,
                    request.y + dy,
                    request.z + dz,
                );
                if block == materials.air {
                    continue;
                }
                if count == TREE_BLOCKS_MAX_RECORDS {
                    return None;
                }
                let item = TreeBlock {
                    dx: dx as i8,
                    dy: dy as i8,
                    dz: dz as i8,
                    block,
                };
                if !emit(item) {
                    return None;
                }
                count += 1;
            }
        }
    }
    Some(count)
}

/// Compute runtime tree block geometry: offsets relative to the root coordinate.
pub(crate) fn tree_blocks(request: &TreeBlocksRequest) -> Option<Vec<TreeBlock>> {
    let mut records = Vec::with_capacity(TREE_BLOCKS_MAX_RECORDS);
    let count = visit_tree_blocks(request, |record| {
        records.push(record);
        true
    })?;
    debug_assert_eq!(records.len(), count);
    Some(records)
}

/// 把树形几何编码为输出布局:`count u32` LE 加每条 8 字节记录。
///
/// 保留字节恒写 0,保证输出字节完全由输入决定。
pub(crate) fn encode_tree_blocks(records: &[TreeBlock], out: &mut [u8]) {
    debug_assert_eq!(
        out.len(),
        TREE_BLOCKS_COUNT_BYTES + records.len() * TREE_BLOCKS_RECORD_BYTES
    );
    out[0..4].copy_from_slice(&(records.len() as u32).to_le_bytes());
    for (index, record) in records.iter().enumerate() {
        let offset = TREE_BLOCKS_COUNT_BYTES + index * TREE_BLOCKS_RECORD_BYTES;
        out[offset] = record.dx as u8;
        out[offset + 1] = record.dy as u8;
        out[offset + 2] = record.dz as u8;
        out[offset + 3] = 0;
        out[offset + 4..offset + 6].copy_from_slice(&record.block.to_le_bytes());
        out[offset + 6..offset + 8].fill(0);
    }
}

// ---- ABI 编码常量与解析 ----
//
// 两个 worldgen 入口共用 magic `MGW1` 的 566 字节 header:
// magic(4) + layout version(4) + seed(8) + min_y(4) + max_y(4) +
// 材料表 15×u16(30) + perm 512×u8(512)。
//
// engine ABI v4 把材料表从 13 项扩到 14 项(末项 water),当时新增的 u16
// 正当占用 v3 预留的 reserved 槽(偏移 50),header 总长不变,但布局语义
// 确实变了,因此 layout version 1 → 2——它是独立于 ABI 版本号的带内第二道
// 混装防线。engine ABI v10 再把材料表从 14 项扩到 15 项(末项 short_grass,
// 位于偏移 52,perm 后移到偏移 54),header 564 → 566 字节,layout 2 → 3。
//
// **不再保留空槽是刻意选择,不是漏了**:新增一个 reserved 槽本身就要把 perm
// 往后挪,而 reserved 的意义是推迟这个代价、不是提前支付;何况下一次扩字段
// 必然同样改动材料表布局、必然升 ABI 版本,而 ABI 版本号每次调用都校验,
// 混装在那一步就被挡住。空槽在一个本来就不兼容的版本里买不到兼容性。
// chunk 入口追加 chunk_x/chunk_z(8);probe 入口追加 record_count(4) 与
// 每条 16 字节的查询记录(mode + wx/wy/wz)。

/// 共用 header 字节数。
pub(crate) const WORLDGEN_HEADER_BYTES: usize = 566;
/// chunk 入口输入总字节数:header + chunk_x/chunk_z。
pub(crate) const WORLDGEN_CHUNK_INPUT_BYTES: usize = WORLDGEN_HEADER_BYTES + 8;
/// chunk 入口输出字节数:98304 个 u16 LE。
pub(crate) const WORLDGEN_CHUNK_OUTPUT_BYTES: usize = CHUNK_VOLUME * 2;
/// probe 入口单批最大记录数,沿用 raycast 的 64-record batch 约定。
pub(crate) const WORLDGEN_PROBE_MAX_RECORDS: usize = 64;
/// probe 输入记录字节数:mode(4) + wx/wy/wz(12)。
pub(crate) const WORLDGEN_PROBE_RECORD_BYTES: usize = 16;
/// probe 输出记录字节数:height(4) + block(2) + reserved(2)。
pub(crate) const WORLDGEN_PROBE_OUTPUT_RECORD_BYTES: usize = 8;

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// 从字节流读取小端 u32;lod 模块解析 tile 输入时共用(避免第二份解码)。
pub(crate) fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// 从字节流读取小端 i32;lod 模块解析 tile 坐标与列数时共用。
pub(crate) fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_i64(bytes: &[u8], offset: usize) -> i64 {
    i64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// 解析并校验共用 header;任何违约返回 None(FFI 层转为 StatusInput)。
///
/// 校验项:magic/layout 精确匹配、Y 范围必须与内核常量一致(防止 Go/Rust
/// 世界高度漂移)、材料表 15 项两两互异(air 是哨兵,重复 ID 会破坏与 Go
/// 语义的对应关系)。perm 为 u8,取值域即合法域。
///
/// v3 的 `reserved != 0` 校验随字段一起消失:偏移 50 已被 water 正当占用,
/// 校验对象不复存在。混装由 ABI 版本号与 layout version 两道拦截。
///
/// 互异性的**唯一豁免**是 `water == air`:这是 Go 侧 `fluidEnabled` 关闭时
/// 的门控编码(design D6),water 只被写入、从不参与等值比较,取 air 编号
/// 即让注水退化为空操作。water 与其余 13 项重复仍然拒绝——那只可能是
/// Go/Rust 材料表漂移。`short_grass` 参与装饰写入,与任何材料(含 air 和
/// water)重复都不豁免。
pub(crate) fn parse_header(bytes: &[u8]) -> Option<WorldgenParams> {
    if bytes.len() < WORLDGEN_HEADER_BYTES
        || &bytes[0..4] != b"MGW1"
        || read_u32(bytes, 4) != 3
        || read_i32(bytes, 16) != WORLD_MIN_Y
        || read_i32(bytes, 20) != WORLD_MAX_Y
    {
        return None;
    }
    let seed = read_i64(bytes, 8);
    let materials = Materials {
        air: read_u16(bytes, 24),
        stone: read_u16(bytes, 26),
        dirt: read_u16(bytes, 28),
        grass: read_u16(bytes, 30),
        bedrock: read_u16(bytes, 32),
        snow: read_u16(bytes, 34),
        sand: read_u16(bytes, 36),
        clay: read_u16(bytes, 38),
        gravel: read_u16(bytes, 40),
        iron_ore: read_u16(bytes, 42),
        coal_ore: read_u16(bytes, 44),
        oak_log: read_u16(bytes, 46),
        leaves: read_u16(bytes, 48),
        water: read_u16(bytes, 50),
        short_grass: read_u16(bytes, 52),
    };
    let ids = materials.as_array();
    // as_array 的顺序:0 = air,13 = water,14 = short_grass。
    // (0, 13) 这一对是门控豁免;short_grass 不参与任何豁免。
    const AIR_INDEX: usize = 0;
    const WATER_INDEX: usize = 13;
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            if ids[i] == ids[j] && !(i == AIR_INDEX && j == WATER_INDEX) {
                return None;
            }
        }
    }
    let mut perm = [0u8; 512];
    perm.copy_from_slice(&bytes[54..WORLDGEN_HEADER_BYTES]);
    Some(WorldgenParams {
        seed,
        materials,
        perm,
    })
}

/// 解析 chunk 入口输入,返回参数与区块坐标。
pub(crate) fn parse_chunk_input(bytes: &[u8]) -> Option<(WorldgenParams, i32, i32)> {
    if bytes.len() != WORLDGEN_CHUNK_INPUT_BYTES {
        return None;
    }
    let params = parse_header(bytes)?;
    let chunk_x = read_i32(bytes, WORLDGEN_HEADER_BYTES);
    let chunk_z = read_i32(bytes, WORLDGEN_HEADER_BYTES + 4);
    Some((params, chunk_x, chunk_z))
}

/// 单条 probe 查询记录。mode:0=HeightAt,1=TerrainBlockAt,2=BaseBlockAt。
pub(crate) struct ProbeRecord {
    pub mode: u32,
    pub wx: i32,
    pub wy: i32,
    pub wz: i32,
}

/// 解析 probe 入口输入,返回参数与查询记录;record_count 必须在 1..=64,
/// 长度必须与记录数精确匹配,mode 越界拒绝。
pub(crate) fn parse_probe_input(bytes: &[u8]) -> Option<(WorldgenParams, Vec<ProbeRecord>)> {
    if bytes.len() < WORLDGEN_HEADER_BYTES + 4 {
        return None;
    }
    let count = read_u32(bytes, WORLDGEN_HEADER_BYTES) as usize;
    if count == 0
        || count > WORLDGEN_PROBE_MAX_RECORDS
        || bytes.len() != WORLDGEN_HEADER_BYTES + 4 + count * WORLDGEN_PROBE_RECORD_BYTES
    {
        return None;
    }
    let params = parse_header(bytes)?;
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let offset = WORLDGEN_HEADER_BYTES + 4 + index * WORLDGEN_PROBE_RECORD_BYTES;
        let mode = read_u32(bytes, offset);
        if mode > 2 {
            return None;
        }
        records.push(ProbeRecord {
            mode,
            wx: read_i32(bytes, offset + 4),
            wy: read_i32(bytes, offset + 8),
            wz: read_i32(bytes, offset + 12),
        });
    }
    Some((params, records))
}

/// 执行一批 probe 查询,把结果按输出布局写入 out(每条 8 字节)。
///
/// mode 0 写 height 字段,mode 1/2 写 block 字段;未使用字段保持零,
/// 保证输出字节完全由输入决定。
pub(crate) fn run_probe(params: &WorldgenParams, records: &[ProbeRecord], out: &mut [u8]) {
    debug_assert_eq!(
        out.len(),
        records.len() * WORLDGEN_PROBE_OUTPUT_RECORD_BYTES
    );
    for (index, record) in records.iter().enumerate() {
        let offset = index * WORLDGEN_PROBE_OUTPUT_RECORD_BYTES;
        let mut height = 0i32;
        let mut block = 0u16;
        match record.mode {
            0 => height = params.height_at(record.wx, record.wz),
            1 => block = params.terrain_block_at(record.wx, record.wy, record.wz),
            _ => block = params.base_block_at(record.wx, record.wy, record.wz),
        }
        out[offset..offset + 4].copy_from_slice(&height.to_le_bytes());
        out[offset + 4..offset + 6].copy_from_slice(&block.to_le_bytes());
        out[offset + 6] = 0;
        out[offset + 7] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试材料表:取值互异即可,具体数值不影响结构断言。
    /// water 取 13(与 air 不同)代表门控开启态,short_grass 取 14。
    fn materials() -> Materials {
        materials_with_water(13)
    }

    /// 指定 water 编号的测试材料表:传 0(= air)即门控关闭态。
    fn materials_with_water(water: u16) -> Materials {
        Materials {
            air: 0,
            stone: 1,
            dirt: 2,
            grass: 3,
            bedrock: 4,
            snow: 5,
            sand: 6,
            clay: 7,
            gravel: 8,
            iron_ore: 9,
            coal_ore: 10,
            oak_log: 11,
            leaves: 12,
            water,
            short_grass: 14,
        }
    }

    /// 恒等 perm 表足以驱动结构性断言(确定性、分层、树形)。
    fn params(seed: i64) -> WorldgenParams {
        let mut perm = [0u8; 512];
        for (i, entry) in perm.iter_mut().enumerate() {
            *entry = (i & 255) as u8;
        }
        WorldgenParams {
            seed,
            materials: materials(),
            perm,
        }
    }

    /// 打乱的 perm 表。恒等 perm 在整数格点噪声恒为 0,地形近乎平面、
    /// 海平面以下没有空气格,注水断言会退化成空断言;这里用确定性的
    /// LCG 洗牌造出真实起伏的地形。
    fn shuffled_perm(seed: u64) -> [u8; 512] {
        let mut base: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut state = seed | 1;
        for i in (1..256usize).rev() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let j = (state >> 33) as usize % (i + 1);
            base.swap(i, j);
        }
        std::array::from_fn(|i| base[i & 255])
    }

    /// 起伏地形 + 指定 water 编号的参数;注水相关测试统一用它。
    fn params_water(seed: i64, water: u16) -> WorldgenParams {
        WorldgenParams {
            seed,
            materials: materials_with_water(water),
            perm: shuffled_perm(seed as u64),
        }
    }

    #[test]
    fn perlin_is_zero_at_lattice_points() {
        let p = params(1);
        for i in -8..8 {
            assert_eq!(p.noise_at(f64::from(i), f64::from(i * 3)), 0.0);
        }
    }

    #[test]
    fn generate_chunk_is_deterministic() {
        let p = params(42);
        let mut a = vec![0u16; CHUNK_VOLUME];
        let mut b = vec![0u16; CHUNK_VOLUME];
        p.generate_chunk(-1, 2, &mut a);
        p.generate_chunk(-1, 2, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn chunk_matches_pointwise_base_block() {
        let p = params(7);
        let mut dense = vec![0u16; CHUNK_VOLUME];
        p.generate_chunk(1, -1, &mut dense);
        for y in WORLD_MIN_Y..WORLD_MAX_Y {
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    let wx = (1 << SECTION_SHIFT) + lx;
                    let wz = (-1 << SECTION_SHIFT) + lz;
                    assert_eq!(
                        dense[dense_index(lx, y, lz)],
                        p.base_block_at(wx, y, wz),
                        "({wx},{y},{wz})"
                    );
                }
            }
        }
    }

    #[test]
    fn terrain_layers_follow_go_rules() {
        let p = params(3);
        // 底层是基岩,地表是草或雪/沙系,高度之上是空气。
        assert_eq!(p.terrain_layer(WORLD_MIN_Y, 80), p.materials.bedrock);
        assert_eq!(p.terrain_layer(90, 80), p.materials.air);
        assert_eq!(p.terrain_layer(80, 80), p.materials.grass);
        assert_eq!(p.terrain_layer(78, 80), p.materials.dirt);
        assert_eq!(p.terrain_layer(60, 80), p.materials.stone);
    }

    #[test]
    fn ore_hash_is_stable_and_salt_sensitive() {
        let a = ore_hash(42, 1, 2, 3, COAL_SALT);
        assert_eq!(a, ore_hash(42, 1, 2, 3, COAL_SALT));
        assert_ne!(a, ore_hash(42, 1, 2, 3, IRON_SALT));
        assert_ne!(a, ore_hash(43, 1, 2, 3, COAL_SALT));
    }

    #[test]
    fn tree_canopy_shape_is_log_priority() {
        let tree = OakTree {
            root_x: 0,
            root_y: 100,
            root_z: 0,
            height: 4,
            fluffy: false,
            rare: false,
            branch_count: 0,
            branch_dir: [0, 0],
        };
        let m = materials();
        // 树干整列是原木,冠顶十字是树叶,冠层角落空缺。
        assert_eq!(oak_tree_block_at(&tree, &m, 0, 100, 0), m.oak_log);
        assert_eq!(oak_tree_block_at(&tree, &m, 0, 103, 0), m.oak_log);
        assert_eq!(oak_tree_block_at(&tree, &m, 1, 104, 0), m.leaves);
        assert_eq!(oak_tree_block_at(&tree, &m, 1, 104, 1), m.air);
        assert_eq!(oak_tree_block_at(&tree, &m, 2, 102, 2), m.air);
        assert_eq!(oak_tree_block_at(&tree, &m, 2, 102, 1), m.leaves);
    }
    #[test]
    fn sea_level_constants_agree() {
        // SEA_LEVEL_Y 是 SEA_LEVEL 的整数副本,漂移会让注水高度与地形高度脱节。
        assert_eq!(f64::from(SEA_LEVEL_Y), SEA_LEVEL);
    }

    /// 生成"干"(water = air,门控关闭)与"湿"(water = 13,门控开启)两份
    /// 同种子同区块 dense,并返回被注水改写的格数。
    fn dry_and_wet(seed: i64, cx: i32, cz: i32) -> (Vec<u16>, Vec<u16>, usize) {
        let mut dry = vec![0u16; CHUNK_VOLUME];
        let mut wet = vec![0u16; CHUNK_VOLUME];
        params_water(seed, 0).generate_chunk(cx, cz, &mut dry);
        params_water(seed, 13).generate_chunk(cx, cz, &mut wet);
        let changed = dry.iter().zip(&wet).filter(|(a, b)| a != b).count();
        (dry, wet, changed)
    }

    #[test]
    fn flooding_only_replaces_air_at_or_below_sea_level() {
        let (dry, wet, changed) = dry_and_wet(42, 3, -5);
        // 夹具前提:这个区块必须真的有海平面以下的空气格,否则下面全是空断言。
        assert!(changed > 0, "夹具失效:该区块没有可注水的格");
        for y in WORLD_MIN_Y..WORLD_MAX_Y {
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    let index = dense_index(lx, y, lz);
                    // 自然短草层的唯一两态分歧:门控关闭时海水步写空气,
                    // surface == 63 的命中列在 y == 64(海平面)装饰短草;
                    // 开启态该格先被海水占据,短草让位。其余仍按旧规则。
                    let expected = if y <= SEA_LEVEL_Y && (dry[index] == 0 || dry[index] == 14) {
                        13
                    } else {
                        dry[index]
                    };
                    assert_eq!(wet[index], expected, "({lx},{y},{lz})");
                }
            }
        }
    }

    #[test]
    fn flooding_preserves_terrain_ore_and_trees() {
        let (dry, wet, changed) = dry_and_wet(42, 3, -5);
        assert!(changed > 0, "夹具失效:该区块没有可注水的格");
        // 分层(石/土/草/基岩/雪/沙/黏土/砂砾)、矿石与树木的每一格都必须原样保留。
        // 短草(14)例外:它在关闭态可出现在海平面格,开启态该格属海水。
        let mut seen_ore = false;
        let mut seen_tree = false;
        for (index, &block) in dry.iter().enumerate() {
            if block == 0 || block == 14 {
                continue;
            }
            assert_eq!(wet[index], block, "注水改写了非空气格 index={index}");
            seen_ore |= block == 9 || block == 10;
            seen_tree |= block == 11 || block == 12;
        }
        // 夹具前提:该区块必须真的含矿石与树木,否则"不受影响"是空断言。
        assert!(seen_ore, "夹具失效:该区块没有矿石");
        assert!(seen_tree, "夹具失效:该区块没有树木");
    }

    #[test]
    fn gate_off_leaves_every_floodable_cell_as_air() {
        // 门控关闭(water = air)时,注水必须整体退化为空操作:开启态被注水的
        // **每一格**在关闭态都必须仍是空气或(海平面格的)自然短草,且输出里
        // 不允许出现 13 号方块。
        //
        // 这条断言的对象是"内核是否老老实实用 materials.water 写入":一旦
        // flood_sea_level 绕过材料表硬编码水的编号,Go 侧的门控(water 填 air)
        // 就被架空,关闭态会长出水,本测试立刻变红。短草例外与
        // flooding_only_replaces_air_at_or_below_sea_level 同源:关闭态海水步
        // 写空气,surface == 63 的命中列在海平面装饰短草。
        let (dry, wet, changed) = dry_and_wet(42, 3, -5);
        // 先断言"关闭态没有水",再断言夹具非空:顺序如此是为了让内核硬编码
        // 水编号这类真实故障报出"关闭态出现了水",而不是被后面的夹具守卫
        // 抢先报成"夹具失效"(硬编码会让两态输出相同,changed 归零)。
        assert!(!dry.contains(&13), "门控关闭时输出里出现了水");
        assert!(changed > 0, "夹具失效:该区块没有可注水的格");
        let mut checked = 0;
        for (index, &block) in wet.iter().enumerate() {
            if block == 13 {
                assert!(
                    dry[index] == 0 || dry[index] == 14,
                    "门控关闭时 index={index} 本应仍是空气或短草"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, changed, "两态差异应当恰好是被注水的格");
    }

    #[test]
    fn chunk_and_pointwise_agree_on_water() {
        let p = params_water(7, 13);
        let mut dense = vec![0u16; CHUNK_VOLUME];
        p.generate_chunk(1, -1, &mut dense);
        let mut water_cells = 0;
        for y in WORLD_MIN_Y..WORLD_MAX_Y {
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    let wx = (1 << SECTION_SHIFT) + lx;
                    let wz = (-1 << SECTION_SHIFT) + lz;
                    let block = dense[dense_index(lx, y, lz)];
                    assert_eq!(block, p.base_block_at(wx, y, wz), "({wx},{y},{wz})");
                    if block == 13 {
                        water_cells += 1;
                        assert!(y <= SEA_LEVEL_Y, "海平面以上出现水 ({wx},{y},{wz})");
                    }
                }
            }
        }
        assert!(water_cells > 0, "夹具失效:该区块没有水");
    }

    #[test]
    fn header_allows_water_equal_to_air_but_rejects_other_duplicates() {
        // 门控关闭时 Go 侧把 water 填成 air 编号,header 必须接受。
        assert!(parse_header(&layout_three_header(42, 1, 14)).is_some());
        // water = 15:门控开启态,必须通过。
        assert!(parse_header(&layout_three_header(42, 15, 14)).is_some());
        // water = stone(2):这只可能是材料表漂移,必须拒绝。
        assert!(parse_header(&layout_three_header(42, 2, 14)).is_none());
    }

    // ---- 自然短草层(natural-grass-seeds 变更)的契约测试 ----
    //
    // 以下测试全部以字节级 header 驱动(不构造 `Materials` 字面量),
    // 保证在 framing 未实现时以可观察的解析失败(RED)而不是编译失败暴露。

    /// 短草生成判定的冻结 salt,与 design 决策 3 逐字一致。测试侧以字面量
    /// 钉住:实现侧常量一旦漂移,哈希命中集合随之改变,密度与逐列断言变红。
    const SHORT_GRASS_SALT_FOR_TEST: u64 = 0x5348_4F52_5447_5253;

    /// 写入 header 材料表第 index 项(15 项布局)。
    fn put_material(bytes: &mut [u8], index: usize, id: u16) {
        bytes[24 + index * 2..26 + index * 2].copy_from_slice(&id.to_le_bytes());
    }

    /// 构造 layout 3 的 566 字节 `MGW1` header:材料表 15 项(0..=12 取
    /// 1..=13,water/short_grass 由参数给定)、洗牌 perm 从偏移 54 开始。
    /// 材料 id 刻意避开 0,便于区分"输出缓冲原样"与"生成的空气"。
    fn layout_three_header(seed: i64, water: u16, short_grass: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 566];
        bytes[0..4].copy_from_slice(b"MGW1");
        bytes[4..8].copy_from_slice(&3u32.to_le_bytes());
        bytes[8..16].copy_from_slice(&seed.to_le_bytes());
        bytes[16..20].copy_from_slice(&WORLD_MIN_Y.to_le_bytes());
        bytes[20..24].copy_from_slice(&WORLD_MAX_Y.to_le_bytes());
        for index in 0..13usize {
            put_material(&mut bytes, index, index as u16 + 1);
        }
        put_material(&mut bytes, 13, water);
        put_material(&mut bytes, 14, short_grass);
        bytes[54..566].copy_from_slice(&shuffled_perm(seed as u64));
        bytes
    }

    /// 用 layout 3 header 解析参数;framing 未实现时 `parse_header` 返回
    /// None,expect 失败即 RED 的直接证据。
    fn grass_params(seed: i64, water: u16, short_grass: u16) -> WorldgenParams {
        parse_header(&layout_three_header(seed, water, short_grass))
            .expect("layout 3 header 必须可解析")
    }

    #[test]
    fn mgw1_layout_three_framing_is_frozen() {
        // layout 3 / header 566 / chunk input 574 / probe input 570+16N 是
        // 冻结的 ABI 帧契约;layout version 是独立于 ABI 版本号的带内混装防线。
        assert_eq!(WORLDGEN_HEADER_BYTES, 566);
        assert_eq!(WORLDGEN_CHUNK_INPUT_BYTES, 574);
        let header = layout_three_header(42, 15, 14);
        assert!(
            parse_header(&header).is_some(),
            "layout 3 + 15 项互异材料必须被接受"
        );

        // 旧 layout 2 的 564 字节 header 必须整体拒绝。
        let mut legacy = header[..564].to_vec();
        legacy[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert!(
            parse_header(&legacy).is_none(),
            "旧 layout 2 header 必须被拒绝"
        );

        // 旧 chunk 入口总长 572 必须拒绝。
        let mut legacy_chunk = legacy.clone();
        legacy_chunk.extend_from_slice(&0i32.to_le_bytes());
        legacy_chunk.extend_from_slice(&0i32.to_le_bytes());
        assert_eq!(legacy_chunk.len(), 572);
        assert!(
            parse_chunk_input(&legacy_chunk).is_none(),
            "旧 572 字节 chunk 输入必须被拒绝"
        );

        // 新 chunk/probe 帧被精确接受:probe 总长 570 + 16×N。
        let mut chunk_input = header.clone();
        chunk_input.extend_from_slice(&0i32.to_le_bytes());
        chunk_input.extend_from_slice(&0i32.to_le_bytes());
        assert_eq!(chunk_input.len(), 574);
        assert!(parse_chunk_input(&chunk_input).is_some());

        let mut probe_input = header;
        probe_input.extend_from_slice(&1u32.to_le_bytes());
        probe_input.extend_from_slice(&2u32.to_le_bytes());
        probe_input.extend_from_slice(&0i32.to_le_bytes());
        probe_input.extend_from_slice(&0i32.to_le_bytes());
        probe_input.extend_from_slice(&0i32.to_le_bytes());
        assert_eq!(probe_input.len(), 586);
        assert!(
            parse_probe_input(&probe_input).is_some(),
            "570+16N probe 帧必须被接受"
        );

        // 旧 probe 帧(564 header + count + 记录)必须拒绝。
        let mut legacy_probe = legacy;
        legacy_probe.extend_from_slice(&1u32.to_le_bytes());
        legacy_probe.extend_from_slice(&[0u8; 16]);
        assert_eq!(legacy_probe.len(), 584);
        assert!(
            parse_probe_input(&legacy_probe).is_none(),
            "旧 564 帧probe输入必须被拒绝"
        );
    }

    #[test]
    fn mgw1_short_grass_is_not_exempt_from_uniqueness() {
        // 门控关闭态的唯一豁免仍是 water == air;short_grass 与任何材料
        // (含 air 与 water)重复都必须拒绝——它参与写入,不在豁免集合内。
        assert!(
            parse_header(&layout_three_header(42, 1, 14)).is_some(),
            "water == air 的门控豁免必须保留"
        );
        assert!(
            parse_header(&layout_three_header(42, 15, 1)).is_none(),
            "short_grass == air 不在豁免内"
        );
        assert!(
            parse_header(&layout_three_header(42, 1, 1)).is_none(),
            "short_grass 与 water 同为 air 编号仍构成重复"
        );
        assert!(
            parse_header(&layout_three_header(42, 15, 2)).is_none(),
            "short_grass == stone 是材料表漂移"
        );
        assert!(
            parse_header(&layout_three_header(42, 15, 15)).is_none(),
            "short_grass == water(门控开启)也必须拒绝"
        );
    }

    /// 遍历区块全部 256 列,按冻结规则逐列核对短草判定:
    /// 装饰格必须满足"草地表面 + hash 命中",未装饰的空气目标格必须未命中。
    /// short_grass 编号由调用方给定(header 构造时写入),不读 `Materials`
    /// 字段,保证 framing 未实现时测试以运行期断言失败(RED)暴露。
    fn audit_short_grass(
        params: &WorldgenParams,
        chunk_x: i32,
        chunk_z: i32,
        dense: &[u16],
        short_grass: u16,
    ) -> (usize, usize) {
        let air = params.materials.air;
        let grass = params.materials.grass;
        let mut grass_columns = 0;
        let mut decorated = 0;
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let wx = (chunk_x << SECTION_SHIFT) + lx;
                let wz = (chunk_z << SECTION_SHIFT) + lz;
                let mut surface = params.height_at(wx, wz);
                if surface >= WORLD_MAX_Y {
                    surface = WORLD_MAX_Y - 1;
                }
                let target = dense[dense_index(lx, surface + 1, lz)];
                let hash_hit = ore_hash(params.seed, wx, 0, wz, SHORT_GRASS_SALT_FOR_TEST) & 3 == 0;
                if target == short_grass {
                    // 装饰格:正下方必须是草地表面,且该列 hash 必然命中。
                    assert_eq!(
                        dense[dense_index(lx, surface, lz)],
                        grass,
                        "({wx},{wz}) 短草下方不是草地表面"
                    );
                    assert!(hash_hit, "({wx},{wz}) 未命中列出现短草");
                    decorated += 1;
                } else if target == air {
                    if dense[dense_index(lx, surface, lz)] == grass {
                        grass_columns += 1;
                        assert!(!hash_hit, "({wx},{wz}) 命中的空草地列未被装饰");
                    }
                } else {
                    // 树/海水等既有内容占据目标格:短草必须让位。
                    assert_ne!(target, short_grass);
                }
            }
        }
        (grass_columns, decorated)
    }

    #[test]
    fn short_grass_decorates_qualifying_columns_with_gaps() {
        // 门控关闭(water == air)的湿语义:dry 世界海平面以下没有水,
        // 表面为草的列照常参与判定。
        let params = grass_params(42, 1, 14);
        let mut dense = vec![0u16; CHUNK_VOLUME];
        params.generate_chunk(3, -5, &mut dense);
        let (gaps, decorated) = audit_short_grass(&params, 3, -5, &dense, 14);
        assert!(decorated > 0, "夹具失效:该区块没有任何短草");
        assert!(gaps > 0, "夹具失效:该区块没有空隙列,无法证明稀疏分布");

        // 材料表驱动的编号:换一个 short_grass 编号重生成,装饰格必须随之改变,
        // 证明内核使用请求材料表而不是硬编码编号。
        let other = grass_params(42, 1, 20);
        let mut dense_other = vec![0u16; CHUNK_VOLUME];
        other.generate_chunk(3, -5, &mut dense_other);
        let cells = dense
            .iter()
            .zip(&dense_other)
            .filter(|&(a, b)| a != b)
            .count();
        assert_eq!(cells, decorated, "两份输出差异格数必须恰为装饰格数");
        assert!(
            dense_other.contains(&20),
            "装饰格必须使用请求的 short_grass 编号"
        );
    }

    #[test]
    fn short_grass_density_is_quarter_over_corpus() {
        // 多区块(含负坐标)语料上命中比例必须落在 1/4 邻域:过密或过疏
        // 都意味着判定偏离 hash & 3 == 0 的冻结规则。
        let params = grass_params(42, 1, 14);
        let mut grass_columns = 0usize;
        let mut decorated = 0usize;
        for (cx, cz) in [(3, -5), (0, 0), (-1, -1), (37, -104)] {
            let mut dense = vec![0u16; CHUNK_VOLUME];
            params.generate_chunk(cx, cz, &mut dense);
            let (gaps, hits) = audit_short_grass(&params, cx, cz, &dense, 14);
            grass_columns += gaps + hits;
            decorated += hits;
        }
        assert!(decorated > 0, "夹具失效:语料没有任何短草");
        let ratio = decorated as f64 / grass_columns as f64;
        assert!(
            (0.15..0.35).contains(&ratio),
            "短草密度 {ratio:.3} 偏离 1/4 邻域(装饰={decorated}, 草地列={grass_columns})"
        );
    }

    #[test]
    fn short_grass_yields_to_trees_and_sea() {
        // 湿世界(注水开启,water=15):海平面及以下的目标格已被水占据,
        // 短草绝不允许出现在 y <= SEA_LEVEL_Y。
        let wet = grass_params(42, 15, 14);
        let corpus = [(3, -5), (0, 0), (1, 1), (-1, -1), (37, -104)];
        let short_grass = 14u16;
        let water = wet.materials.water;
        let mut wet_cells = 0;
        let mut yielded = 0;
        for (cx, cz) in corpus {
            let mut dense = vec![0u16; CHUNK_VOLUME];
            wet.generate_chunk(cx, cz, &mut dense);
            for y in WORLD_MIN_Y..=SEA_LEVEL_Y {
                let layer =
                    (y - WORLD_MIN_Y) as usize * (SECTION_SIZE as usize) * (SECTION_SIZE as usize);
                for cell in &dense[layer..layer + (SECTION_SIZE as usize) * (SECTION_SIZE as usize)]
                {
                    assert_ne!(*cell, short_grass, "y={y} 出现短草,海水优先被破坏");
                    if *cell == water {
                        wet_cells += 1;
                    }
                }
            }

            // 树优先:树干列即使 hash 命中,目标格也必须保持原木/树叶。
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    let wx = (cx << SECTION_SHIFT) + lx;
                    let wz = (cz << SECTION_SHIFT) + lz;
                    let mut surface = wet.height_at(wx, wz);
                    if surface >= WORLD_MAX_Y {
                        surface = WORLD_MAX_Y - 1;
                    }
                    let tree = wet.tree_block_at(wx, surface + 1, wz);
                    if tree == wet.materials.air {
                        continue;
                    }
                    let hash_hit =
                        ore_hash(wet.seed, wx, 0, wz, SHORT_GRASS_SALT_FOR_TEST) & 3 == 0;
                    let target = dense[dense_index(lx, surface + 1, lz)];
                    if hash_hit {
                        assert_eq!(target, tree, "({wx},{wz}) 命中列的树被短草覆盖");
                        yielded += 1;
                    }
                }
            }
        }
        assert!(wet_cells > 0, "夹具失效:湿语料没有水");
        assert!(
            yielded > 0,
            "夹具失效:语料没有 hash 命中的树列,树优先是空断言"
        );
    }

    #[test]
    fn short_grass_chunk_and_pointwise_parity_spans_boundaries() {
        // 整块与单点两条生产出口必须逐格一致,语料覆盖正/负坐标与区块边界。
        let params = grass_params(7, 1, 14);
        let mut total = 0;
        for (cx, cz) in [(0, 0), (1, 0), (-1, -1), (37, -104)] {
            let mut dense = vec![0u16; CHUNK_VOLUME];
            params.generate_chunk(cx, cz, &mut dense);
            for y in WORLD_MIN_Y..WORLD_MAX_Y {
                for lz in 0..SECTION_SIZE {
                    for lx in 0..SECTION_SIZE {
                        let wx = (cx << SECTION_SHIFT) + lx;
                        let wz = (cz << SECTION_SHIFT) + lz;
                        assert_eq!(
                            dense[dense_index(lx, y, lz)],
                            params.base_block_at(wx, y, wz),
                            "({wx},{y},{wz})"
                        );
                    }
                }
            }
            total += dense.iter().filter(|&&b| b == 14).count();
        }
        assert!(total > 0, "夹具失效:语料没有任何短草");
    }

    #[test]
    fn height_and_terrain_queries_ignore_short_grass() {
        // 短草是纯装饰:HeightAt 仍指草地表面,TerrainBlockAt 在装饰格
        // 仍是 air,装饰格上方也仍是 air(单格,不向上生长)。
        let params = grass_params(42, 1, 14);
        let mut dense = vec![0u16; CHUNK_VOLUME];
        params.generate_chunk(3, -5, &mut dense);
        let mut checked = 0;
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let wx = (3 << SECTION_SHIFT) + lx;
                let wz = (-5 << SECTION_SHIFT) + lz;
                let mut surface = params.height_at(wx, wz);
                if surface >= WORLD_MAX_Y {
                    surface = WORLD_MAX_Y - 1;
                }
                if dense[dense_index(lx, surface + 1, lz)] != 14 {
                    continue;
                }
                assert_eq!(params.height_at(wx, wz), surface, "短草不得抬高高度图");
                assert_eq!(
                    params.terrain_block_at(wx, surface + 1, wz),
                    params.materials.air,
                    "TerrainBlockAt 必须忽略装饰短草"
                );
                assert_eq!(
                    params.base_block_at(wx, surface + 2, wz),
                    params.materials.air,
                    "短草必须只有单格"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "夹具失效:该区块没有短草");
    }

    #[test]
    fn short_grass_is_independent_of_generation_order() {
        // 短草判定只依赖世界种子与世界坐标:同一批区块按不同顺序生成,
        // 输出必须逐位一致(无邻区块状态、无进程 RNG)。
        let params = grass_params(42, 1, 14);
        let chunks = [(0, 0), (-1, -1), (1, 0), (37, -104)];
        let mut forward = Vec::new();
        for (cx, cz) in chunks {
            let mut dense = vec![0u16; CHUNK_VOLUME];
            params.generate_chunk(cx, cz, &mut dense);
            forward.push(dense);
        }
        let mut backward = Vec::new();
        for &(cx, cz) in chunks.iter().rev() {
            let mut dense = vec![0u16; CHUNK_VOLUME];
            params.generate_chunk(cx, cz, &mut dense);
            backward.push(dense);
        }
        backward.reverse();
        assert_eq!(forward, backward);
    }

    // ---- 橡树高度/冠形/珍异多样性（确定性树扩展）的契约测试 ----

    /// 在给定种子下收集一片候选格的全部有效橡树。
    fn collect_oaks(seed: i64, cell_range: i32) -> Vec<OakTree> {
        let p = params(seed);
        let mut out = Vec::new();
        for cz in -cell_range..=cell_range {
            for cx in -cell_range..=cell_range {
                if let Some(tree) = p.oak_tree_for_cell(cx, cz) {
                    out.push(tree);
                }
            }
        }
        out
    }

    #[test]
    fn normal_oak_heights_cover_five_to_seven() {
        // 普通橡树树高必须在 5..7 内均匀取满三档,冠形标准/蓬松两档都必须出现。
        let oaks = collect_oaks(11, 30);
        assert!(!oaks.is_empty(), "夹具失效:语料没有任何橡树");
        let mut heights = [0usize; 3];
        let mut standard = false;
        let mut fluffy = false;
        let mut normals = 0;
        for tree in &oaks {
            if tree.rare {
                continue;
            }
            normals += 1;
            assert!(
                (5..=7).contains(&tree.height),
                "普通树高 {} 越界",
                tree.height
            );
            heights[(tree.height - 5) as usize] += 1;
            fluffy |= tree.fluffy;
            standard |= !tree.fluffy;
        }
        assert!(normals > 0, "夹具失效:语料没有任何普通橡树");
        assert!(
            heights.iter().all(|&c| c > 0),
            "普通树高未取满 5/6/7:{heights:?}"
        );
        // 均匀分布:7 比特位域 %3 给 43/43/42,三档都应落在 1/3 邻域;
        // 3 比特位域的 3/8、3/8、2/8 偏置会在这里变红。
        for (grade, count) in heights.iter().enumerate() {
            let ratio = *count as f64 / normals as f64;
            assert!(
                (0.28..0.39).contains(&ratio),
                "树高 {} 档比例 {ratio:.3} 偏离均匀分布",
                grade + 5,
            );
        }
        assert!(
            standard && fluffy,
            "冠形两档必须都出现(标准={standard}, 蓬松={fluffy})"
        );
    }

    #[test]
    fn rare_big_trees_appear_at_small_rate() {
        // 珍异大树必须以小概率出现,主干高度在 8..12 内,分杈 1..2 条;
        // 普通树不带分杈。
        let oaks = collect_oaks(11, 30);
        assert!(!oaks.is_empty(), "夹具失效:语料没有任何橡树");
        let mut rare = 0;
        for tree in &oaks {
            if !tree.rare {
                assert_eq!(tree.branch_count, 0, "普通树不应带分杈");
                continue;
            }
            rare += 1;
            assert!(
                (8..=12).contains(&tree.height),
                "珍异树高 {} 越界",
                tree.height
            );
            assert!(
                (1..=2).contains(&tree.branch_count),
                "珍异分杈条数 {} 越界",
                tree.branch_count
            );
            for dir in tree.branch_dir {
                assert!(dir < 4, "分杈方向 {dir} 越界");
            }
        }
        assert!(rare > 0, "语料没有任何珍异大树");
        let ratio = rare as f64 / oaks.len() as f64;
        // 固定种子语料是确定性的:门槛设计为 21/256≈8.2%,断言收紧到
        // 5%..12%,3% 或 18% 的实现会在这里变红。
        assert!(
            (0.05..0.12).contains(&ratio),
            "珍异比例 {ratio:.3} 偏离约 8% 的小概率"
        );
    }

    #[test]
    fn fluffy_crown_has_extra_top_layer() {
        // 蓬松档必须在标准四层冠之上多一层树叶顶。
        let oaks = collect_oaks(11, 30);
        assert!(!oaks.is_empty(), "夹具失效:语料没有任何橡树");
        let m = materials();
        let fluffy = oaks
            .iter()
            .filter(|t| {
                oak_tree_block_at(t, &m, t.root_x, t.root_y + t.height + 1, t.root_z) == m.leaves
            })
            .count();
        assert!(fluffy > 0, "语料没有任何带顶层的蓬松橡树");
    }

    #[test]
    fn oak_candidate_gate_and_layout_are_hash_determined() {
        // 候选门槛、根偏移、树高、冠形、珍异与分杈全部由同一哈希的不交位域
        // 确定;奇数哈希必不生成;重复查询逐字段一致,与遍历顺序、时间无关。
        let p = params(11);
        let mut generated = 0;
        let mut odd_seen = 0;
        for cz in -8..=8 {
            for cx in -8..=8 {
                let hash = ore_hash(11, cx, 0, cz, OAK_TREE_SALT);
                let first = p.oak_tree_for_cell(cx, cz);
                let second = p.oak_tree_for_cell(cx, cz);
                match (&first, &second) {
                    (Some(a), Some(b)) => assert_eq!(
                        (
                            a.root_x,
                            a.root_y,
                            a.root_z,
                            a.height,
                            a.fluffy,
                            a.rare,
                            a.branch_count,
                            a.branch_dir,
                        ),
                        (
                            b.root_x,
                            b.root_y,
                            b.root_z,
                            b.height,
                            b.fluffy,
                            b.rare,
                            b.branch_count,
                            b.branch_dir,
                        ),
                        "候选格 ({cx},{cz}) 两次查询不一致",
                    ),
                    (None, None) => {}
                    _ => panic!("候选格 ({cx},{cz}) 两次查询不一致"),
                }
                if hash & 1 == 1 {
                    odd_seen += 1;
                    assert!(first.is_none(), "奇数哈希候选格 ({cx},{cz}) 必须不生成");
                    continue;
                }
                let Some(tree) = first else { continue };
                generated += 1;
                assert_eq!(
                    tree.root_x,
                    (cx << OAK_TREE_CELL_SHIFT).wrapping_add(((hash >> 1) & 7) as i32),
                    "根 X 偏移必须取哈希 1..3 位",
                );
                assert_eq!(
                    tree.root_z,
                    (cz << OAK_TREE_CELL_SHIFT).wrapping_add(((hash >> 4) & 7) as i32),
                    "根 Z 偏移必须取哈希 4..6 位",
                );
                let rare = ((hash >> 14) & 0xFF) < 21;
                assert_eq!(tree.rare, rare, "珍异判定必须取哈希 14..21 位约 8% 门槛");
                if rare {
                    assert_eq!(
                        tree.height,
                        (8 + ((hash >> 23) & 7) % 5) as i32,
                        "珍异树高必须取 8..12",
                    );
                    assert_eq!(
                        tree.branch_count,
                        (1 + ((hash >> 26) & 1)) as u8,
                        "分杈条数必须取 1..2",
                    );
                    assert_eq!(
                        tree.branch_dir,
                        [((hash >> 27) & 3) as u8, ((hash >> 29) & 3) as u8],
                        "分杈方向必须取哈希高位",
                    );
                } else {
                    assert_eq!(
                        tree.height,
                        (5 + ((hash >> 7) & 0x7F) % 3) as i32,
                        "普通树高必须取 5..7",
                    );
                    assert_eq!(
                        tree.fluffy,
                        (hash >> 22) & 1 == 1,
                        "冠形档必须由哈希第 22 位独立确定",
                    );
                    assert_eq!(tree.branch_count, 0, "普通树不应带分杈");
                }
            }
        }
        assert!(generated > 0, "夹具失效:语料没有任何橡树");
        assert!(odd_seen > 0, "夹具失效:语料没有任何奇数哈希候选格");
    }

    /// 冠形对比夹具:标准/蓬松只差顶层标志,其余字段一致。
    fn crown_fixture(fluffy: bool) -> OakTree {
        OakTree {
            root_x: 0,
            root_y: 100,
            root_z: 0,
            height: 6,
            fluffy,
            rare: false,
            branch_count: 0,
            branch_dir: [0, 0],
        }
    }

    #[test]
    fn fluffy_crown_adds_decornered_top_layer() {
        // 蓬松档 = 标准四层冠 + 去角 3×3 顶;3×3 去角后恰为十字五格。
        let m = materials();
        let std = crown_fixture(false);
        let lush = crown_fixture(true);
        let top = 100 + 6 - 1;
        // 标准四层冠逐层钉住:下两层去角 5×5、顶层满 3×3、顶上十字。
        assert_eq!(oak_tree_block_at(&std, &m, 2, top - 1, 1), m.leaves);
        assert_eq!(oak_tree_block_at(&std, &m, 2, top - 1, 2), m.air);
        assert_eq!(oak_tree_block_at(&std, &m, 1, top, 1), m.leaves);
        assert_eq!(oak_tree_block_at(&std, &m, 1, top + 1, 0), m.leaves);
        assert_eq!(oak_tree_block_at(&std, &m, 1, top + 1, 1), m.air);
        // 顶上第二层:标准档为空,蓬松档为十字树叶。
        assert_eq!(oak_tree_block_at(&std, &m, 0, top + 2, 0), m.air);
        assert_eq!(oak_tree_block_at(&lush, &m, 0, top + 2, 0), m.leaves);
        assert_eq!(oak_tree_block_at(&lush, &m, 1, top + 2, 0), m.leaves);
        assert_eq!(oak_tree_block_at(&lush, &m, 0, top + 2, 1), m.leaves);
        assert_eq!(oak_tree_block_at(&lush, &m, 1, top + 2, 1), m.air);
        // 其余层两档必须一致。
        for (dx, dz) in [(2, 1), (1, 1), (1, 0)] {
            for dy in -2..=1 {
                assert_eq!(
                    oak_tree_block_at(&std, &m, dx, top + dy, dz),
                    oak_tree_block_at(&lush, &m, dx, top + dy, dz),
                    "({dx},{dy},{dz})",
                );
            }
        }
    }

    #[test]
    fn rare_crown_reaches_three_with_air_only_branches() {
        // 珍异球状大冠旁侧突出主干 3 格并去角;分杈为横向原木、定长 3 格。
        let m = materials();
        let tree = OakTree {
            root_x: 0,
            root_y: 100,
            root_z: 0,
            height: 10,
            fluffy: false,
            rare: true,
            branch_count: 2,
            branch_dir: [0, 2],
        };
        let top = 100 + 10 - 1;
        // 主干穿过树冠处仍是原木(原木优先于树叶)。
        for y in 100..=top {
            assert_eq!(oak_tree_block_at(&tree, &m, 0, y, 0), m.oak_log);
        }
        // 大冠:宽层去角 7×7、中层满 5×5、上层满 3×3、顶十字。
        assert_eq!(oak_tree_block_at(&tree, &m, 3, top - 2, 0), m.leaves);
        assert_eq!(oak_tree_block_at(&tree, &m, 0, top - 2, 3), m.leaves);
        assert_eq!(oak_tree_block_at(&tree, &m, 3, top - 2, 3), m.air);
        assert_eq!(oak_tree_block_at(&tree, &m, 3, top - 1, 1), m.leaves);
        assert_eq!(oak_tree_block_at(&tree, &m, 2, top, 2), m.leaves);
        assert_eq!(oak_tree_block_at(&tree, &m, 1, top + 1, 1), m.leaves);
        assert_eq!(oak_tree_block_at(&tree, &m, 0, top + 2, 0), m.leaves);
        assert_eq!(oak_tree_block_at(&tree, &m, 1, top + 2, 1), m.air);
        // 分杈:0 号向 +X 长在 top-4,1 号向 +Z 长在 top-5,各伸 3 格。
        for step in 1..=3 {
            assert_eq!(oak_tree_block_at(&tree, &m, step, top - 4, 0), m.oak_log);
            assert_eq!(oak_tree_block_at(&tree, &m, 0, top - 5, step), m.oak_log);
        }
        assert_eq!(oak_tree_block_at(&tree, &m, 4, top - 4, 0), m.air);
        assert_eq!(oak_tree_block_at(&tree, &m, 0, top - 5, 4), m.air);
    }

    /// 在固定语料里找第一棵珍异树(种子固定,结果确定)。
    fn first_rare_tree() -> (WorldgenParams, OakTree) {
        for seed in 1..=60 {
            let p = params(seed);
            for cz in -8..=8 {
                for cx in -8..=8 {
                    if let Some(tree) = p.oak_tree_for_cell(cx, cz)
                        && tree.rare
                    {
                        return (p, tree);
                    }
                }
            }
        }
        panic!("夹具失效:语料里找不到珍异树");
    }

    #[test]
    fn tree_writes_never_replace_solid() {
        // 树(含分杈)只替换原始空气:全石头 dense 落笔后必须逐位不变;
        // 全空气 dense 里该珍异树的分杈格必须全部落为原木。
        let (p, tree) = first_rare_tree();
        assert!((8..=12).contains(&tree.height));
        assert!((1..=2).contains(&tree.branch_count));
        let cx = tree.root_x >> SECTION_SHIFT;
        let cz = tree.root_z >> SECTION_SHIFT;
        let stone = p.materials.stone;
        let mut dense = vec![stone; CHUNK_VOLUME];
        p.apply_oak_trees(cx, cz, &mut dense);
        assert!(
            dense.iter().all(|&b| b == stone),
            "落笔改写了固体格(含分杈只替换空气语义被破坏)"
        );
        // 空气底上,分杈几何经真实落笔路径仍然成立。
        let mut air = vec![p.materials.air; CHUNK_VOLUME];
        p.apply_oak_trees(cx, cz, &mut air);
        let top = tree.root_y + tree.height - 1;
        for i in 0..tree.branch_count {
            let (dx, dz) = branch_offset(tree.branch_dir[i as usize]);
            let by = top - 4 - i32::from(i);
            for step in 1..=3 {
                let (wx, wz) = (tree.root_x + dx * step, tree.root_z + dz * step);
                if (wx >> SECTION_SHIFT) != cx || (wz >> SECTION_SHIFT) != cz {
                    continue;
                }
                assert_eq!(
                    air[dense_index(wx & (SECTION_SIZE - 1), by, wz & (SECTION_SIZE - 1))],
                    p.materials.oak_log,
                    "分杈格 ({wx},{by},{wz}) 未落为原木",
                );
            }
        }
    }

    /// 找一棵树冠/分杈跨越区块边界的珍异树(种子固定,结果确定)。
    fn boundary_crossing_rare_tree() -> (WorldgenParams, OakTree) {
        for seed in 1..=60 {
            let p = params(seed);
            for cz in -16..=16 {
                for cx in -16..=16 {
                    if let Some(tree) = p.oak_tree_for_cell(cx, cz) {
                        if !tree.rare {
                            continue;
                        }
                        if (tree.root_x - 3) >> SECTION_SHIFT != (tree.root_x + 3) >> SECTION_SHIFT
                            || (tree.root_z - 3) >> SECTION_SHIFT
                                != (tree.root_z + 3) >> SECTION_SHIFT
                        {
                            return (p, tree);
                        }
                    }
                }
            }
        }
        panic!("夹具失效:语料里找不到跨界珍异树");
    }

    #[test]
    fn extended_reach_tree_matches_pointwise_across_boundary() {
        // 半径 3 的树冠/分杈跨越区块边界时,整块与单点必须逐格一致:
        // 任一侧漏扫(邻域半径不足)都会在这里变红。
        let (p, tree) = boundary_crossing_rare_tree();
        let top = tree.root_y + tree.height - 1;
        let mut touched = Vec::new();
        for cz in ((tree.root_z - 3) >> SECTION_SHIFT)..=((tree.root_z + 3) >> SECTION_SHIFT) {
            for cx in ((tree.root_x - 3) >> SECTION_SHIFT)..=((tree.root_x + 3) >> SECTION_SHIFT) {
                touched.push((cx, cz));
            }
        }
        assert!(touched.len() > 1, "夹具失效:该树未真正跨界");
        for (cx, cz) in touched {
            let mut dense = vec![p.materials.air; CHUNK_VOLUME];
            p.generate_chunk(cx, cz, &mut dense);
            for y in tree.root_y..=(top + 2) {
                for z in tree.root_z - 3..=tree.root_z + 3 {
                    for x in tree.root_x - 3..=tree.root_x + 3 {
                        if (x >> SECTION_SHIFT) != cx || (z >> SECTION_SHIFT) != cz {
                            continue;
                        }
                        assert_eq!(
                            dense[dense_index(x & (SECTION_SIZE - 1), y, z & (SECTION_SIZE - 1))],
                            p.base_block_at(x, y, z),
                            "({x},{y},{z})",
                        );
                    }
                }
            }
        }
    }

    /// 珍异树形的最大伸展包络:全部非空气格落在根 ±3、树干底到顶上两层内,
    /// 包络外一圈全是空气;四个分杈方向各有一棵夹具,分杈与大冠的 3 格伸展
    /// 都被打满(回归非空)。
    ///
    /// 这是覆盖半径的形状侧:单点邻域与整块落笔盒都只取 ±3,形状若伸出包络,
    /// 两条路径会同时漏掉同一格——本测试让这种缺口先在这里变红,而不是等
    /// 跨界一致性测试用坏运气去撞边界对齐。
    #[test]
    fn rare_shape_fits_radius_three_box() {
        let m = materials();
        let mut reached_axial = [false; 4];
        let mut reached_crown = false;
        for dir in 0..4u8 {
            let tree = OakTree {
                root_x: 0,
                root_y: 100,
                root_z: 0,
                height: 12,
                fluffy: false,
                rare: true,
                branch_count: 1,
                branch_dir: [dir, 0],
            };
            let top = 100 + 12 - 1;
            let (step_x, step_z) = branch_offset(dir);
            for y in 99..=(top + 3) {
                for z in -4..=4 {
                    for x in -4..=4 {
                        let block = oak_tree_block_at(&tree, &m, x, y, z);
                        if block == m.air {
                            continue;
                        }
                        let dx = x.abs();
                        let dz = z.abs();
                        assert!(dx <= 3 && dz <= 3, "({x},{y},{z}) 水平伸出 ±3");
                        assert!((100..=(top + 2)).contains(&y), "({x},{y},{z}) 竖直伸出包络");
                        if block == m.oak_log
                            && y == top - 4
                            && x * step_x + z * step_z == 3
                            && (z * step_x - x * step_z) == 0
                        {
                            reached_axial[usize::from(dir)] = true;
                        }
                        if block == m.leaves && dx.max(dz) == 3 {
                            reached_crown = true;
                        }
                    }
                }
            }
        }
        assert!(
            reached_axial.iter().all(|&hit| hit),
            "四个方向的分杈都必须打满 3 格:{reached_axial:?}"
        );
        assert!(reached_crown, "大冠旁侧必须有 3 格伸展");
    }

    /// 冠顶上界守卫:根加树高加冠顶两层触及上界时整树形状为空,低一格时冠
    /// 顶照常生成。守卫对档位一视同仁——标准树顶上第二层本就是空气,仍被
    /// 整体丢弃:保守但两侧路径共用同一守卫,不可能分叉。
    #[test]
    fn crown_top_guard_rejects_at_upper_bound() {
        let m = materials();
        // 珍异 12 格:根 306 时顶上第二层 319 在界内,根 307 时触界整树为空。
        let fitting = OakTree {
            root_x: 0,
            root_y: 306,
            root_z: 0,
            height: 12,
            fluffy: false,
            rare: true,
            branch_count: 2,
            branch_dir: [0, 2],
        };
        let top = 306 + 12 - 1;
        assert_eq!(oak_tree_block_at(&fitting, &m, 0, top + 2, 0), m.leaves);
        let touching = OakTree {
            root_x: 0,
            root_y: 307,
            root_z: 0,
            height: 12,
            fluffy: false,
            rare: true,
            branch_count: 2,
            branch_dir: [0, 2],
        };
        for y in 307..WORLD_MAX_Y {
            for z in -3..=3 {
                for x in -3..=3 {
                    assert_eq!(
                        oak_tree_block_at(&touching, &m, x, y, z),
                        m.air,
                        "触界珍异树 ({x},{y},{z}) 必须为空"
                    );
                }
            }
        }
        // 普通标准 7 格同理:根 311 冠顶正常(顶上第二层回到空气),根 312
        // 整树为空。
        let std_fitting = OakTree {
            root_x: 0,
            root_y: 311,
            root_z: 0,
            height: 7,
            fluffy: false,
            rare: false,
            branch_count: 0,
            branch_dir: [0, 0],
        };
        let std_top = 311 + 7 - 1;
        assert_eq!(
            oak_tree_block_at(&std_fitting, &m, 0, std_top + 1, 0),
            m.leaves
        );
        assert_eq!(
            oak_tree_block_at(&std_fitting, &m, 0, std_top + 2, 0),
            m.air
        );
        let std_touching = OakTree {
            root_x: 0,
            root_y: 312,
            root_z: 0,
            height: 7,
            fluffy: false,
            rare: false,
            branch_count: 0,
            branch_dir: [0, 0],
        };
        for y in 312..WORLD_MAX_Y {
            for z in -3..=3 {
                for x in -3..=3 {
                    assert_eq!(
                        oak_tree_block_at(&std_touching, &m, x, y, z),
                        m.air,
                        "触界普通树 ({x},{y},{z}) 必须为空"
                    );
                }
            }
        }
    }

    /// 找一棵根在负坐标、冠幅跨越区块边界的珍异树(种子固定,结果确定)。
    fn negative_boundary_crossing_rare_tree() -> (WorldgenParams, OakTree) {
        for seed in 1..=60 {
            let p = params(seed);
            for cz in -16..=16 {
                for cx in -16..=16 {
                    if let Some(tree) = p.oak_tree_for_cell(cx, cz) {
                        if !tree.rare {
                            continue;
                        }
                        if tree.root_x >= 0 && tree.root_z >= 0 {
                            continue;
                        }
                        if (tree.root_x - 3) >> SECTION_SHIFT != (tree.root_x + 3) >> SECTION_SHIFT
                            || (tree.root_z - 3) >> SECTION_SHIFT
                                != (tree.root_z + 3) >> SECTION_SHIFT
                        {
                            return (p, tree);
                        }
                    }
                }
            }
        }
        panic!("夹具失效:语料里找不到负坐标跨界珍异树");
    }

    #[test]
    fn negative_extended_reach_tree_matches_pointwise_across_boundary() {
        // 负坐标跨界珍异树:整块与单点逐格一致。算术右移即 floor 除法,负坐
        // 标候选格划分与正坐标同一规则;半径不足会在跨界侧先漏掉一格。
        let (p, tree) = negative_boundary_crossing_rare_tree();
        assert!(
            tree.root_x < 0 || tree.root_z < 0,
            "夹具失效:该树不在负坐标"
        );
        let top = tree.root_y + tree.height - 1;
        let mut touched = Vec::new();
        for cz in ((tree.root_z - 3) >> SECTION_SHIFT)..=((tree.root_z + 3) >> SECTION_SHIFT) {
            for cx in ((tree.root_x - 3) >> SECTION_SHIFT)..=((tree.root_x + 3) >> SECTION_SHIFT) {
                touched.push((cx, cz));
            }
        }
        assert!(touched.len() > 1, "夹具失效:该树未真正跨界");
        for (cx, cz) in touched {
            let mut dense = vec![p.materials.air; CHUNK_VOLUME];
            p.generate_chunk(cx, cz, &mut dense);
            for y in tree.root_y..=(top + 2) {
                for z in tree.root_z - 3..=tree.root_z + 3 {
                    for x in tree.root_x - 3..=tree.root_x + 3 {
                        if (x >> SECTION_SHIFT) != cx || (z >> SECTION_SHIFT) != cz {
                            continue;
                        }
                        assert_eq!(
                            dense[dense_index(x & (SECTION_SIZE - 1), y, z & (SECTION_SIZE - 1))],
                            p.base_block_at(x, y, z),
                            "({x},{y},{z})",
                        );
                    }
                }
            }
        }
    }
    /// Pointwise probe queries can hand the tree layer a Y coordinate outside
    /// the world range before any guard runs. Debug overflow checks must not
    /// panic on the crown-layer Y comparison, and wrapped comparison keeps
    /// out-of-reach layers air instead of matching a wrapped offset.
    #[test]
    fn extreme_probe_y_stays_air_without_overflow() {
        let (p, tree) = first_rare_tree();
        let m = &p.materials;
        assert_eq!(
            oak_tree_block_at(&tree, m, tree.root_x, i32::MIN, tree.root_z),
            m.air
        );
        assert_eq!(p.base_block_at(tree.root_x, i32::MIN, tree.root_z), m.air);
        assert_eq!(p.base_block_at(tree.root_x, i32::MAX, tree.root_z), m.air);
    }
}

/// 运行时树形几何(`tree_blocks`)的主题测试。
///
/// 该入口与世界生成的 8×8 候选格橡树是两条独立路径:参数由 `TREE_BLOCKS_SALT`
/// 从 (世界种子, 根坐标) 派生,因此这里既不断言世界生成行为,也不复用
/// `oak_tree_for_cell` 的语料;层形复用由「记录集合逐层等于普通档树冠层」
/// 这一可观察断言覆盖。
#[cfg(test)]
mod tree_blocks_tests {
    use super::*;

    /// 构造一条根坐标取世界高度中段(远离上下界)的请求。
    fn request(seed: i64, x: i32, z: i32) -> TreeBlocksRequest {
        TreeBlocksRequest { seed, x, y: 64, z }
    }

    /// 树干高度:根列上的原木条数。运行时几何只把根列写为原木(无分杈),
    /// 因此它同时是树高。
    fn trunk_height(records: &[TreeBlock]) -> i32 {
        records
            .iter()
            .filter(|record| {
                record.dx == 0 && record.dz == 0 && record.block == TREE_BLOCKS_OAK_LOG
            })
            .count() as i32
    }

    /// 指定层上满足条件的树叶条数。
    fn leaves_matching(records: &[TreeBlock], dy: i32, keep: impl Fn(i32, i32) -> bool) -> usize {
        records
            .iter()
            .filter(|record| {
                i32::from(record.dy) == dy
                    && record.block == TREE_BLOCKS_LEAVES
                    && keep(i32::from(record.dx), i32::from(record.dz))
            })
            .count()
    }

    #[test]
    fn geometry_is_deterministic_and_bounded() {
        let mut fluffy_seen = false;
        let mut standard_seen = false;
        for seed in [1i64, 42, -7, 20_260_909] {
            for (x, z) in [
                (0i32, 0i32),
                (-137, 902),
                (15, -16),
                (1_000_003, -2_000_004),
            ] {
                let first = tree_blocks(&request(seed, x, z)).expect("合法请求必须成功");
                let second = tree_blocks(&request(seed, x, z)).expect("合法请求必须成功");
                assert_eq!(first, second, "同输入必须逐记录一致");

                assert!(first.len() <= TREE_BLOCKS_MAX_RECORDS);
                // 根格自身是第一条记录,且恒为树干底。
                assert_eq!(
                    (first[0].dx, first[0].dy, first[0].dz, first[0].block),
                    (0, 0, 0, TREE_BLOCKS_OAK_LOG)
                );
                let height = trunk_height(&first);
                assert!((5..=7).contains(&height), "树高 {height} 超出 5..7");
                for record in &first {
                    assert!(
                        record.dx.abs() <= 2 && record.dz.abs() <= 2,
                        "水平半径超出 2"
                    );
                    assert!((0..=height + 1).contains(&i32::from(record.dy)), "dy 越界");
                    assert!(
                        record.block == TREE_BLOCKS_OAK_LOG || record.block == TREE_BLOCKS_LEAVES,
                        "几何只允许原木与树叶"
                    );
                    if record.block == TREE_BLOCKS_OAK_LOG {
                        // 无分杈:原木只出现在根列。
                        assert_eq!((record.dx, record.dz), (0, 0), "原木偏离根列");
                    }
                }
                if first
                    .iter()
                    .any(|record| i32::from(record.dy) == height + 1)
                {
                    fluffy_seen = true;
                } else {
                    standard_seen = true;
                }
            }
        }
        assert!(fluffy_seen && standard_seen, "样本必须覆盖蓬松与标准两档");
    }

    #[test]
    fn geometry_reuses_normal_crown_tiers() {
        let mut fluffy_checked = false;
        let mut standard_checked = false;
        for seed in 0..64i64 {
            let records = tree_blocks(&request(seed, 8, -8)).expect("合法请求必须成功");
            let height = trunk_height(&records);
            let fluffy = records
                .iter()
                .any(|record| i32::from(record.dy) == height + 1);
            if fluffy && fluffy_checked || !fluffy && standard_checked {
                continue;
            }
            // 顶下两层:去角 5×5 各 21 格,中心被树干占用,故树叶 20 条。
            for dy in [height - 3, height - 2] {
                assert_eq!(
                    leaves_matching(&records, dy, |dx, dz| !(dx.abs() == 2 && dz.abs() == 2)),
                    20,
                    "层 dy={dy} 不是去角 5×5"
                );
            }
            // 顶下层:3×3 去掉中心树干,树叶 8 条。
            assert_eq!(
                leaves_matching(&records, height - 1, |dx, dz| dx.abs() <= 1
                    && dz.abs() <= 1),
                8,
                "顶下层不是 3×3"
            );
            // 顶层:十字 5 条。
            assert_eq!(
                leaves_matching(&records, height, |dx, dz| dx.abs() + dz.abs() <= 1),
                5,
                "顶层不是十字"
            );
            // 蓬松档在顶上再加一层同形十字;标准档顶上第二层为空。
            assert_eq!(
                leaves_matching(&records, height + 1, |dx, dz| dx.abs() + dz.abs() <= 1),
                if fluffy { 5 } else { 0 },
                "蓬松层不符"
            );
            // 总条数 = 树干 + 两层去角 5×5 + 3×3 + 十字 + 可选蓬松十字。
            let expected = height as usize + 20 + 20 + 8 + 5 + if fluffy { 5 } else { 0 };
            assert_eq!(records.len(), expected, "记录条数与普通档层形不符");
            if fluffy {
                fluffy_checked = true;
            } else {
                standard_checked = true;
            }
            if fluffy_checked && standard_checked {
                return;
            }
        }
        panic!("样本未覆盖蓬松与标准两档");
    }

    #[test]
    fn geometry_is_independent_of_worldgen_and_short_grass_salts() {
        // 独立冻结 salt 是「生长几何与世界生成结果无关」的根因;一旦复用
        // 任一既有 salt,同一坐标的生长几何会与世界生成树形产生耦合。
        assert_ne!(TREE_BLOCKS_SALT, OAK_TREE_SALT);
        assert_ne!(TREE_BLOCKS_SALT, SHORT_GRASS_GENERATION_SALT);
        // 同一坐标在世界生成候选格 salt 与运行时 salt 下必须给出不同的哈希。
        assert_ne!(
            ore_hash(42, 3, 64, -5, TREE_BLOCKS_SALT),
            ore_hash(42, 3, 64, -5, OAK_TREE_SALT)
        );
    }

    #[test]
    fn parse_rejects_bad_input() {
        let valid = {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"MTB1");
            bytes.extend_from_slice(&TREE_BLOCKS_LAYOUT.to_le_bytes());
            bytes.extend_from_slice(&42i64.to_le_bytes());
            bytes.extend_from_slice(&7i32.to_le_bytes());
            bytes.extend_from_slice(&64i32.to_le_bytes());
            bytes.extend_from_slice(&(-9i32).to_le_bytes());
            bytes
        };
        assert_eq!(valid.len(), TREE_BLOCKS_INPUT_BYTES);
        let parsed = parse_tree_blocks_input(&valid).expect("合法输入必须解析成功");
        assert_eq!((parsed.seed, parsed.x, parsed.y, parsed.z), (42, 7, 64, -9));

        let bad_magic = {
            let mut bytes = valid.clone();
            bytes[0] = b'X';
            bytes
        };
        let bad_layout = {
            let mut bytes = valid.clone();
            bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
            bytes
        };
        let mut too_short = valid.clone();
        too_short.pop();
        let mut too_long = valid.clone();
        too_long.push(0);
        // 根坐标越界:低于世界下界、以及高到最矮的普通橡树也放不下。
        let below_world = {
            let mut bytes = valid.clone();
            bytes[20..24].copy_from_slice(&(WORLD_MIN_Y - 1).to_le_bytes());
            bytes
        };
        let above_world = {
            let mut bytes = valid.clone();
            bytes[20..24].copy_from_slice(&(WORLD_MAX_Y - 8).to_le_bytes());
            bytes
        };
        // 水平邻域越出 i32 值域:坐标本身合法,但根 ±2 会回绕,必须拒绝。
        let x_at_max = {
            let mut bytes = valid.clone();
            bytes[16..20].copy_from_slice(&i32::MAX.to_le_bytes());
            bytes
        };
        let z_at_min = {
            let mut bytes = valid.clone();
            bytes[24..28].copy_from_slice(&i32::MIN.to_le_bytes());
            bytes
        };
        for (name, bytes) in [
            ("magic", &bad_magic),
            ("layout", &bad_layout),
            ("too_short", &too_short),
            ("too_long", &too_long),
            ("below_world", &below_world),
            ("above_world", &above_world),
            ("x_at_max", &x_at_max),
            ("z_at_min", &z_at_min),
        ] {
            assert!(
                parse_tree_blocks_input(bytes).is_none(),
                "{name} 必须被拒绝"
            );
        }
        // i32 值域内最靠边的合法根坐标仍必须接受:邻域恰好不越界。
        let mut edge = valid.clone();
        edge[16..20].copy_from_slice(&(i32::MAX - 2).to_le_bytes());
        edge[24..28].copy_from_slice(&(i32::MIN + 2).to_le_bytes());
        parse_tree_blocks_input(&edge).expect("i32 边界内 2 格的根坐标必须被接受");
        // 最高合法根坐标:最坏普通橡树(高 7)恰好落在世界上界内。
        let mut highest = valid.clone();
        highest[20..24].copy_from_slice(&(WORLD_MAX_Y - 9).to_le_bytes());
        let parsed = parse_tree_blocks_input(&highest).expect("最高合法根坐标必须被接受");
        let records = tree_blocks(&parsed).expect("最高合法根坐标必须能放下");
        let top = records
            .iter()
            .map(|record| parsed.y + i32::from(record.dy))
            .max()
            .expect("几何非空");
        assert!(top < WORLD_MAX_Y, "几何越出世界上界: {top}");
    }

    #[test]
    fn encode_writes_count_and_fixed_records() {
        let records = vec![
            TreeBlock {
                dx: 0,
                dy: 0,
                dz: 0,
                block: TREE_BLOCKS_OAK_LOG,
            },
            TreeBlock {
                dx: -2,
                dy: 3,
                dz: 2,
                block: TREE_BLOCKS_LEAVES,
            },
        ];
        let mut out =
            vec![0xAAu8; TREE_BLOCKS_COUNT_BYTES + records.len() * TREE_BLOCKS_RECORD_BYTES];
        encode_tree_blocks(&records, &mut out);
        assert_eq!(&out[0..4], &2u32.to_le_bytes());
        assert_eq!(&out[4..12], &[0, 0, 0, 0, 17, 0, 0, 0]);
        assert_eq!(&out[12..20], &[0xFE, 3, 2, 0, 19, 0, 0, 0]);
    }

    #[test]
    fn runtime_block_ids_match_go_core() {
        // 运行时几何写入的方块编号是协议稳定值,与 Go `core` 的 `AirID`(0)、
        // `OakLogID`(17)、`LeavesID`(19)逐一对应;重排即破坏跨语言契约。
        assert_eq!(Materials::RUNTIME_TREE.air, 0);
        assert_eq!(TREE_BLOCKS_OAK_LOG, 17);
        assert_eq!(TREE_BLOCKS_LEAVES, 19);
    }

    #[test]
    fn visit_tree_blocks_matches_legacy_tree_blocks_and_handles_refusal() {
        for seed in [1i64, 42, -7, 20_260_909] {
            for (x, z) in [(0i32, 0i32), (-137, 902), (15, -16)] {
                let req = request(seed, x, z);
                let expected = tree_blocks(&req).expect("valid request succeeds");
                let mut collected = Vec::with_capacity(TREE_BLOCKS_MAX_RECORDS);
                let count = visit_tree_blocks(&req, |record| {
                    collected.push(record);
                    true
                });
                assert_eq!(count, Some(expected.len()));
                assert_eq!(collected, expected);

                let mut early_stopped = Vec::new();
                let mut emit_count = 0;
                let stopped_res = visit_tree_blocks(&req, |record| {
                    early_stopped.push(record);
                    emit_count += 1;
                    emit_count < 2
                });
                assert_eq!(stopped_res, None);
                assert_eq!(early_stopped.len(), 2);
            }
        }
    }
}
