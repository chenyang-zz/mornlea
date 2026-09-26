use crate::native::contracts::{KernelError, MeshRegistry, MeshRegistryEntry, MeshView};

const BLOCKS_BYTES: usize = 27 * 4096 * 2;
const HEIGHTS_PRESENT_BYTES: usize = 9;
const HEIGHTS_BYTES: usize = 9 * 256 * 2;
/// 单条 registry 条目的字节数。
///
/// 布局（小端）：`id: u16` | `opaque: u8` | `emission: u8` | `material: [u16; 6]`
/// | `fluid_height: u8` | `light_attenuation: u8` | `block_top_raw: u8`
/// | `model: u8`，共 20 字节。
///
/// 后四个字节与 `emission` 同形状——每方块一个字节、由 Go 侧
/// `internal/mesh.BlockProperties` 烘焙、`encodeNativeInput` 按同一顺序写出：
///
/// - `fluid_height`：该格**孤立时**的 4-bit 高度原值 `h_raw`（实际高度 `(h_raw+1)/16`）。
///   `0` 是「非流体」哨兵：`h_raw = 14 - level` 且 `level <= 7`，所以真流体的 `h_raw`
///   恒在 `7..=14`，`0` 永远不会是合法的流体高度，于是不必再额外花一个标志位。
///   Rust 侧只消费这个数，**不知道也不需要知道流体等级**——等级→高度的映射是 Go 的
///   单一真值源（`internal/assets.Registry.FluidHeight`）。
/// - `light_attenuation`：派生天空光穿过该方块时的额外衰减，由 `light::build_sky`
///   消费（每格扣减 = 1 + 本值）；竖直直射路径上的空气与植物保持 15，不走这条扣减。
///   合法域只有 `0..=1`，上界来自 `build_sky` 的分桶证明而不是天空光值域，见
///   `RegistryView::validate`。方块光不读它。
/// - `block_top_raw`：非满格方块的 4-bit 顶面高度原值（实际高度 `(h_raw+1)/16`），
///   由 mesher 的常量角高度路径消费。`0` 是「满格方块」哨兵：绝大多数方块是整格
///   立方体，取 0 让既有条目零改动，与 `fluid_height` 的「0=非流体」同构；
///   `1..=14` 表示全部可见面的上缘按该高度下沉（首个消费者是干/湿耕地的 14，
///   即 15/16，恰等于物理碰撞高度）；`15` 无从表达任何合法几何——满格必须写
///   哨兵 0，「非零即短方块」才能保持单一判定。本字段与 `fluid_height` 互斥：
///   流体的角高度由 mesher 邻域平均现算、短方块由本字段常量驱动，两条几何路径
///   不得叠加在同一条目上（见 `RegistryView::validate`）。
/// - `model`：有限模型 tag 的封闭集合。`0` 是「默认」——无模型覆写，满格、短
///   方块、流体与植物继续走既有判定（植物仍按 material 区间识别），火把是
///   model tag 的第一个消费者，故 0 取「默认」而非「cube」，避免为既有四条
///   几何路径重复造 tag；`1..=5` 是火把五种形态（1=落地、2..=5=墙面
///   +X/−X/+Z/−Z，与火把方块编号 72..75 同序）；`6` 是床（床尾/床头 × 4 朝向
///   八形态共用的半高板几何，朝向差异由逐形态材质层表达）；其余值未知拒绝。
///   这是固定 tag 而不是数据驱动格式——见 `RegistryView::validate` 与 greedy
///   的 model dispatcher。
const REGISTRY_ENTRY_BYTES: usize = 20;
/// registry 条目表的容量上限。
///
/// 96 是**上限**而不是当前条目数：Go 侧 `internal/assets.NewRegistry()` 把
/// `core.AirID..core.BlockIDMax-1` 的全部已注册方块烘焙进 mesh registry snapshot，今天
/// 是 85 条（27 个基础材料 + 8 个流体 + 27 个农业编号含工作台 + 9 个门编号 +
/// 5 个火把形态 + 8 个床形态 + 1 个短草）。
/// 留出余量是为了避免每次追加方块编号都要做一次跨语言的常量同步。
///
/// 本常量此前是 35，即"恰好等于当时的条目数"；那种写法会在 Go 侧追加编号时
/// 静默分叉，因为 Go 侧的对应常量当时是从末位方块编号推导出来的、会自己长大。
/// Go 侧对应的 `internal/mesh.nativeMaxRegistryEntries` 必须与本常量一起改，
/// 两侧各自独立定义、没有共享常量或生成步骤，只能人手同步（一次跨语言
/// engine ABI 变更）。Go 侧的 `TestNativeAcceptsRegistryAtGoCapacity` 会真的
/// 喂满一次调用，两侧不同步即变红。
///
/// **注意**：本文件开头 `BLOCKS_BYTES = 27 * 4096 * 2` 里的 27 是 3×3×3 邻域的
/// 区段数，与本常量只是数字撞了，两者语义无关，改一个绝不能牵动另一个。
const MAX_REGISTRY_ENTRIES: usize = 96;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InputError {
    Input,
    Registry,
    Emission,
}

impl From<InputError> for KernelError {
    /// Maps the byte parser's rejection categories onto the typed kernel
    /// error the native lanes surface to callers.
    fn from(error: InputError) -> Self {
        match error {
            InputError::Input => Self::InvalidInput,
            InputError::Registry => Self::InvalidRegistry,
            InputError::Emission => Self::EmissionOutOfRange,
        }
    }
}

/// Structural registry rule shared by the byte parser and the typed lane:
/// `1..=96` entries whose visibility table holds exactly `R * ceil(R / 64)`
/// words — one bit per ordered (id, adjacent id) pair.
///
/// Returns the required total word count. The upper bound is a cross-language
/// hand-synchronized capacity; Go keeps the matching
/// `nativeMaxRegistryEntries` and the capacity sync test feeds a full table
/// through this check.
pub(crate) fn check_registry_shape(count: usize) -> Result<usize, InputError> {
    if count == 0 || count > MAX_REGISTRY_ENTRIES {
        return Err(InputError::Registry);
    }
    Ok(count * count.div_ceil(64))
}

/// One registry entry's semantic fields, decoded from either lane's storage.
///
/// The byte lane decodes the 20-byte record and the typed lane reads the
/// contract fields; both then run the single range check below, so the two
/// lanes cannot drift apart.
pub(crate) struct RegistryEntrySpec {
    pub(crate) id: u16,
    pub(crate) opaque: u8,
    pub(crate) emission: u8,
    pub(crate) fluid_height: u8,
    pub(crate) light_attenuation: u8,
    pub(crate) block_top_raw: u8,
    pub(crate) model: u8,
}

/// Validates one entry in table order plus every semantic range.
///
/// `previous` is the preceding accepted id: ids must strictly increase, which
/// the binary-search lookups rely on. `reject_overbright` is only cleared by
/// the light unit tests' over-bright fixture; production callers keep it set.
///
/// Field domains, in the order they are checked:
///
/// - `opaque` is a two-value flag (`0`/`1`).
/// - `emission` is a 4-bit light level; `16` is out of range.
/// - `fluid_height` is a 4-bit height raw value plus the `0` "not fluid"
///   sentinel, so the legal domain is `0..=14`. `15` is reserved for the
///   mesher's "fluid above too" full-cell case and must never be baked into an
///   entry, which would otherwise paint a full-cell surface from a wrong entry.
/// - `light_attenuation` is the extra sky-light cost per cell, legal `0..=1`.
///   The `1` bound is the premise of the `light::build_sky` bucket proof (the
///   per-cell step is only ever 1 or 2), not the sky-light value range. A `>= 2`
///   attenuation would give 1/2/3 steps, let two brightness values share one
///   bucket and break the "each cell enters the queue at most once" invariant,
///   overflowing the exactly `LIGHT_VOLUME` queue on the render hot path.
///   Supporting it is a separate change that generalizes the buckets.
/// - `block_top_raw` is a 4-bit top-height raw value with the `0` "full cube"
///   sentinel; the legal domain is `0..=14` and `15` cannot express any legal
///   geometry (full cubes must write the sentinel).
/// - `fluid_height` and `block_top_raw` are mutually exclusive: fluid corner
///   heights are computed from the neighborhood while short blocks are driven
///   by the constant field, so one entry must not carry both meanings.
/// - `model` is the closed tag set `0..=6` (default, the five torch forms and
///   the bed); unknown tags would silently fall back to default geometry.
pub(crate) fn check_registry_entry(
    previous: Option<u16>,
    entry: RegistryEntrySpec,
    reject_overbright: bool,
) -> Result<(), InputError> {
    if previous.is_some_and(|previous| previous >= entry.id) || entry.opaque > 1 {
        return Err(InputError::Registry);
    }
    if reject_overbright && entry.emission > 15 {
        return Err(InputError::Emission);
    }
    if entry.fluid_height > 14 {
        return Err(InputError::Registry);
    }
    if entry.light_attenuation > 1 {
        return Err(InputError::Registry);
    }
    if entry.block_top_raw > 14 {
        return Err(InputError::Registry);
    }
    if entry.fluid_height != 0 && entry.block_top_raw != 0 {
        return Err(InputError::Registry);
    }
    if entry.model > 6 {
        return Err(InputError::Registry);
    }
    Ok(())
}

/// Validates a typed registry snapshot fully, closing the ranges the frozen
/// contract constructor leaves open.
///
/// `MeshRegistry::try_new` already checks the entry count, strict id order,
/// visibility word count, emission, fluid height and model. This second pass
/// applies the same shared entry check the byte lane runs, plus the sentinel
/// rules the typed lane must not skip: air and barrier must be distinct and
/// both present, because the view falls back to the barrier id outside the
/// owned neighborhood and the block-light pass falls back to the air id.
pub(crate) fn validate_typed_registry(registry: &MeshRegistry) -> Result<(), InputError> {
    let entries = registry.entries();
    let required_words = check_registry_shape(entries.len())?;
    if registry.visibility().len() != required_words {
        return Err(InputError::Registry);
    }
    if registry.air() == registry.barrier() {
        return Err(InputError::Registry);
    }
    let mut previous = None;
    let mut has_air = false;
    let mut has_barrier = false;
    for entry in entries {
        check_registry_entry(
            previous,
            RegistryEntrySpec {
                id: entry.id,
                opaque: u8::from(entry.opaque),
                emission: entry.emission,
                fluid_height: entry.fluid_height,
                light_attenuation: entry.light_attenuation,
                block_top_raw: entry.block_top_raw,
                model: entry.model as u8,
            },
            true,
        )?;
        has_air |= entry.id == registry.air();
        has_barrier |= entry.id == registry.barrier();
        previous = Some(entry.id);
    }
    if !has_air || !has_barrier {
        return Err(InputError::Registry);
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct MeshInput<'a> {
    pub section_origin_y: i32,
    pub air_id: u16,
    pub barrier_id: u16,
    pub blocks: &'a [u8],
    pub heights_present: &'a [u8],
    pub heights: &'a [u8],
    pub registry: RegistryView<'a>,
}

impl<'a> MeshInput<'a> {
    pub(crate) fn parse(input: &'a [u8]) -> Result<Self, InputError> {
        let parsed = Self::parse_structural(input)?;
        parsed.validate_registry(true)?;
        Ok(parsed)
    }

    #[cfg(test)]
    pub(crate) fn parse_allowing_overbright(input: &'a [u8]) -> Result<Self, InputError> {
        let parsed = Self::parse_structural(input)?;
        parsed.validate_registry(false)?;
        Ok(parsed)
    }

    pub(crate) fn parse_structural(input: &'a [u8]) -> Result<Self, InputError> {
        if input.len() < 16 || &input[0..4] != b"MGM1" {
            return Err(InputError::Input);
        }
        let registry_count = usize::from(read_u16(input, 8));
        let words_per_row = usize::from(read_u16(input, 10));
        // After the shape check `registry_count <= 96`, so the wire table length
        // `count * words_per_row` cannot overflow this multiplication.
        let required_words = check_registry_shape(registry_count)?;
        if registry_count * words_per_row != required_words {
            return Err(InputError::Registry);
        }
        let registry_bytes = registry_count
            .checked_mul(REGISTRY_ENTRY_BYTES)
            .ok_or(InputError::Input)?;
        let visibility_bytes = registry_count
            .checked_mul(words_per_row)
            .and_then(|words| words.checked_mul(8))
            .ok_or(InputError::Input)?;
        let expected = 16usize
            .checked_add(BLOCKS_BYTES)
            .and_then(|n| n.checked_add(HEIGHTS_PRESENT_BYTES))
            .and_then(|n| n.checked_add(HEIGHTS_BYTES))
            .and_then(|n| n.checked_add(registry_bytes))
            .and_then(|n| n.checked_add(visibility_bytes))
            .ok_or(InputError::Input)?;
        if input.len() != expected {
            return Err(InputError::Input);
        }

        let blocks_end = 16 + BLOCKS_BYTES;
        let present_end = blocks_end + HEIGHTS_PRESENT_BYTES;
        let heights_end = present_end + HEIGHTS_BYTES;
        if input[blocks_end..present_end]
            .iter()
            .any(|&present| present > 1)
        {
            return Err(InputError::Input);
        }
        let entries_end = heights_end + registry_bytes;
        let registry = RegistryView {
            entries: &input[heights_end..entries_end],
            visibility: &input[entries_end..],
            count: registry_count,
            words_per_row,
        };
        let air_id = read_u16(input, 12);
        let barrier_id = read_u16(input, 14);

        Ok(Self {
            section_origin_y: read_i32(input, 4),
            air_id,
            barrier_id,
            blocks: &input[16..blocks_end],
            heights_present: &input[blocks_end..present_end],
            heights: &input[present_end..heights_end],
            registry,
        })
    }

    pub(crate) fn validate_registry(&self, reject_overbright: bool) -> Result<(), InputError> {
        if self.air_id == self.barrier_id {
            return Err(InputError::Registry);
        }
        self.registry
            .validate(self.air_id, self.barrier_id, reject_overbright)
    }

    pub(crate) fn block(&self, x: i32, y: i32, z: i32) -> u16 {
        match typed_cell_index(x, y, z) {
            Some(index) => read_u16(self.blocks, index * 2),
            None => self.barrier_id,
        }
    }

    pub(crate) fn sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        let Some((column, cell)) = typed_height_slot(x, z) else {
            return 0;
        };
        if neighbor_cell(y).is_none() || self.heights_present[column] == 0 {
            return 0;
        }
        let highest = read_i16(self.heights, (column * 256 + cell) * 2);
        u8::from(self.section_origin_y + y > i32::from(highest)) * 15
    }
}

#[derive(Debug)]
pub(crate) struct RegistryView<'a> {
    entries: &'a [u8],
    visibility: &'a [u8],
    count: usize,
    words_per_row: usize,
}

impl RegistryView<'_> {
    fn validate(
        &self,
        air_id: u16,
        barrier_id: u16,
        reject_overbright: bool,
    ) -> Result<(), InputError> {
        let mut previous = None;
        let mut has_air = false;
        let mut has_barrier = false;
        for index in 0..self.count {
            let offset = index * REGISTRY_ENTRY_BYTES;
            let id = read_u16(self.entries, offset);
            // Every per-field range lives in `check_registry_entry` so the byte
            // lane and the typed lane reject exactly the same tables.
            check_registry_entry(
                previous,
                RegistryEntrySpec {
                    id,
                    opaque: self.entries[offset + 2],
                    emission: self.entries[offset + 3],
                    fluid_height: self.entries[offset + 16],
                    light_attenuation: self.entries[offset + 17],
                    block_top_raw: self.entries[offset + 18],
                    model: self.entries[offset + 19],
                },
                reject_overbright,
            )?;
            has_air |= id == air_id;
            has_barrier |= id == barrier_id;
            previous = Some(id);
        }
        if !has_air || !has_barrier {
            return Err(InputError::Registry);
        }
        Ok(())
    }

    fn index(&self, id: u16) -> Option<usize> {
        let mut low = 0;
        let mut high = self.count;
        while low < high {
            let middle = low + (high - low) / 2;
            match read_u16(self.entries, middle * REGISTRY_ENTRY_BYTES).cmp(&id) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }

    pub(crate) fn opaque(&self, id: u16) -> bool {
        self.index(id)
            .is_some_and(|index| self.entries[index * REGISTRY_ENTRY_BYTES + 2] != 0)
    }

    pub(crate) fn emission(&self, id: u16) -> u8 {
        self.index(id)
            .map_or(0, |index| self.entries[index * REGISTRY_ENTRY_BYTES + 3])
    }

    /// fluid_height 返回该方块**孤立时**的 4-bit 高度原值 `h_raw`，非流体返回 `None`。
    ///
    /// 见 `REGISTRY_ENTRY_BYTES` 的布局说明：`0` 是非流体哨兵，真流体恒在 `7..=14`。
    /// 未登记的方块编号同样返回 `None`（与 `opaque`/`emission` 的缺省口径一致）。
    pub(crate) fn fluid_height(&self, id: u16) -> Option<u8> {
        let index = self.index(id)?;
        match self.entries[index * REGISTRY_ENTRY_BYTES + 16] {
            0 => None,
            raw => Some(raw),
        }
    }

    /// light_attenuation 返回天空光穿过该方块时的额外衰减。
    ///
    /// 天空光 BFS（`light::build_sky`）消费它：派生传播每格扣减 = 固定的 1 + 本值；
    /// 竖直直射路径上的空气与植物保持 15。
    /// 方块光**不**读它——方块光只经 `AirID` 或植物传播，其他非空气方块一律阻断。
    pub(crate) fn light_attenuation(&self, id: u16) -> u8 {
        self.index(id)
            .map_or(0, |index| self.entries[index * REGISTRY_ENTRY_BYTES + 17])
    }

    /// block_top_raw 返回该方块非满格时的 4-bit 顶面高度原值，满格返回 `None`。
    ///
    /// 见 `REGISTRY_ENTRY_BYTES` 的布局说明：`0` 是满格哨兵，mesher 的常量角
    /// 高度路径只对 `Some(raw)` 的方块下沉（首个消费者是干/湿耕地的 14，即
    /// 15/16）。未登记的方块编号同样返回 `None`（与 `opaque`/`emission` 的
    /// 缺省口径一致）。
    pub(crate) fn block_top_raw(&self, id: u16) -> Option<u8> {
        let index = self.index(id)?;
        match self.entries[index * REGISTRY_ENTRY_BYTES + 18] {
            0 => None,
            raw => Some(raw),
        }
    }

    /// model 返回该方块的有限模型 tag。
    ///
    /// 封闭集合见 `REGISTRY_ENTRY_BYTES` 的布局说明：`0` 是默认（无模型覆写），
    /// `1..=5` 是火把五形态、`6` 是床；7 起的未知值在 `validate` 就被拒绝，
    /// 走不到这里。未登记的方块编号返回默认 0（与 `opaque`/`emission` 的缺省
    /// 口径一致）。
    pub(crate) fn model(&self, id: u16) -> u8 {
        self.index(id)
            .map_or(0, |index| self.entries[index * REGISTRY_ENTRY_BYTES + 19])
    }

    pub(crate) fn material(&self, id: u16, face: usize) -> Option<u16> {
        if face >= 6 {
            return None;
        }
        let index = self.index(id)?;
        Some(read_u16(
            self.entries,
            index * REGISTRY_ENTRY_BYTES + 4 + face * 2,
        ))
    }

    /// `contains` 报告方块编号是否有显式 registry 条目；光照用它让未知编号关闭。
    pub(crate) fn contains(&self, id: u16) -> bool {
        self.index(id).is_some()
    }

    pub(crate) fn face_visible(&self, id: u16, adjacent: u16) -> bool {
        let Some(row) = self.index(id) else {
            return false;
        };
        let Some(column) = self.index(adjacent) else {
            return false;
        };
        let offset = (row * self.words_per_row + column / 64) * 8;
        read_u64(self.visibility, offset) & (1 << (column % 64)) != 0
    }
}

/// Resolves a neighborhood coordinate onto the 27-section cell index.
///
/// The byte lane and the typed lane share this rule: sections are ordered
/// `(cx * 3 + cy) * 3 + cz` and a section cell packs `(ly << 8) | (lz << 4) | lx`
/// with the low nibble per axis relative to its section. `None` means the
/// coordinate lies outside the owned `-16..=31` neighborhood.
pub(crate) fn typed_cell_index(x: i32, y: i32, z: i32) -> Option<usize> {
    let (cx, lx) = neighbor_cell(x)?;
    let (cy, ly) = neighbor_cell(y)?;
    let (cz, lz) = neighbor_cell(z)?;
    Some(((cx * 3 + cy) * 3 + cz) * 4096 + ((ly << 8) | (lz << 4) | lx))
}

/// Resolves a column coordinate onto `(column, cell)` of the 9x256 height
/// tables; `None` means the coordinate lies outside the owned neighborhood.
pub(crate) fn typed_height_slot(x: i32, z: i32) -> Option<(usize, usize)> {
    let (cx, lx) = neighbor_cell(x)?;
    let (cz, lz) = neighbor_cell(z)?;
    Some((cx * 3 + cz, (lz << 4) | lx))
}

/// Read accessor over one validated typed registry snapshot.
///
/// Mirrors the query surface of `RegistryView` so both lanes answer the same
/// questions for the light and mesh rules: ids are unique and sorted, absent
/// ids fall back to the same neutral answers, and a face outside `0..6` has no
/// material.
pub(crate) struct TypedRegistryView<'a> {
    entries: &'a [MeshRegistryEntry],
}

impl<'a> TypedRegistryView<'a> {
    pub(crate) fn new(registry: &'a MeshRegistry) -> Self {
        Self {
            entries: registry.entries(),
        }
    }

    fn index(&self, id: u16) -> Option<usize> {
        self.entries
            .binary_search_by(|entry| entry.id.cmp(&id))
            .ok()
    }

    /// `contains` reports whether the id has an explicit entry; light uses it
    /// to close unknown ids.
    pub(crate) fn contains(&self, id: u16) -> bool {
        self.index(id).is_some()
    }

    pub(crate) fn opaque(&self, id: u16) -> bool {
        self.index(id)
            .is_some_and(|index| self.entries[index].opaque)
    }

    pub(crate) fn emission(&self, id: u16) -> u8 {
        self.index(id)
            .map_or(0, |index| self.entries[index].emission)
    }

    pub(crate) fn light_attenuation(&self, id: u16) -> u8 {
        self.index(id)
            .map_or(0, |index| self.entries[index].light_attenuation)
    }

    pub(crate) fn material(&self, id: u16, face: usize) -> Option<u16> {
        if face >= 6 {
            return None;
        }
        self.index(id)
            .map(|index| self.entries[index].material[face])
    }
}

/// Read accessor over one validated typed section view.
///
/// Ownership: the accessor copies the view (its references are `Copy`) and owns
/// no cells. Coordinates resolve through the same `typed_cell_index` /
/// `typed_height_slot` rules as the byte lane, and a coordinate outside the
/// owned 3x3x3 neighborhood answers the barrier id exactly like `MeshInput`.
pub(crate) struct MeshViewAccess<'a> {
    view: MeshView<'a>,
    registry: TypedRegistryView<'a>,
}

impl<'a> MeshViewAccess<'a> {
    pub(crate) fn new(view: &MeshView<'a>) -> Self {
        Self {
            view: *view,
            registry: TypedRegistryView::new(view.registry),
        }
    }

    pub(crate) fn block(&self, x: i32, y: i32, z: i32) -> u16 {
        match typed_cell_index(x, y, z) {
            Some(index) => self.view.blocks[index],
            None => self.view.registry.barrier(),
        }
    }

    pub(crate) fn sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        let Some((column, cell)) = typed_height_slot(x, z) else {
            return 0;
        };
        if neighbor_cell(y).is_none() || !self.view.heights_present[column] {
            return 0;
        }
        u8::from(self.view.section_origin_y + y > i32::from(self.view.heights[column][cell])) * 15
    }

    pub(crate) fn air_id(&self) -> u16 {
        self.view.registry.air()
    }

    pub(crate) fn contains(&self, id: u16) -> bool {
        self.registry.contains(id)
    }

    pub(crate) fn opaque(&self, id: u16) -> bool {
        self.registry.opaque(id)
    }

    pub(crate) fn emission(&self, id: u16) -> u8 {
        self.registry.emission(id)
    }

    pub(crate) fn light_attenuation(&self, id: u16) -> u8 {
        self.registry.light_attenuation(id)
    }

    pub(crate) fn material(&self, id: u16, face: usize) -> Option<u16> {
        self.registry.material(id, face)
    }
}

fn neighbor_cell(value: i32) -> Option<(usize, usize)> {
    if !(-16..=31).contains(&value) {
        return None;
    }
    let shifted = value + 16;
    Some(((shifted >> 4) as usize, (shifted & 15) as usize))
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_i16(bytes: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{InputError, MAX_REGISTRY_ENTRIES, MeshInput};

    const BLOCKS_OFFSET: usize = 16;
    const HEIGHTS_PRESENT_OFFSET: usize = BLOCKS_OFFSET + 27 * 4096 * 2;
    const HEIGHTS_OFFSET: usize = HEIGHTS_PRESENT_OFFSET + 9;
    const REGISTRY_OFFSET: usize = HEIGHTS_OFFSET + 9 * 256 * 2;
    /// 测试夹具复用生产常量，避免条目布局再扩容时夹具默默错位。
    pub(crate) const ENTRY_BYTES: usize = super::REGISTRY_ENTRY_BYTES;

    pub(crate) fn valid_input() -> Vec<u8> {
        let mut input = vec![0; REGISTRY_OFFSET + 3 * ENTRY_BYTES + 3 * 8];
        input[0..4].copy_from_slice(b"MGM1");
        input[4..8].copy_from_slice(&(-32_i32).to_le_bytes());
        input[8..10].copy_from_slice(&3_u16.to_le_bytes());
        input[10..12].copy_from_slice(&1_u16.to_le_bytes());
        input[12..14].copy_from_slice(&0_u16.to_le_bytes());
        input[14..16].copy_from_slice(&1_u16.to_le_bytes());

        let center_cell = BLOCKS_OFFSET + (13 * 4096 + ((2 << 8) | (3 << 4) | 1)) * 2;
        input[center_cell..center_cell + 2].copy_from_slice(&0x1234_u16.to_le_bytes());
        input[HEIGHTS_PRESENT_OFFSET + 4] = 1;
        let height = HEIGHTS_OFFSET + (4 * 256 + (5 << 4) + 3) * 2;
        input[height..height + 2].copy_from_slice(&(-33_i16).to_le_bytes());

        for (index, id) in [0_u16, 1, 40000].into_iter().enumerate() {
            let entry = REGISTRY_OFFSET + index * ENTRY_BYTES;
            input[entry..entry + 2].copy_from_slice(&id.to_le_bytes());
            input[entry + 2] = u8::from(id == 1);
            input[entry + 3] = if id == 40000 { 7 } else { 0 };
            for face in 0..6 {
                let material = id.wrapping_add(face as u16);
                input[entry + 4 + face * 2..entry + 6 + face * 2]
                    .copy_from_slice(&material.to_le_bytes());
            }
            // id=40000 冒充一格流体：h_raw=9、额外衰减 1，用来证明这两个新字节
            // 真的跨过了 ABI 边界（0 是非流体哨兵，若编码丢失就会读回 None/0）。
            input[entry + 16] = if id == 40000 { 9 } else { 0 };
            input[entry + 17] = u8::from(id == 40000);
        }
        for (index, word) in [2_u64, 5, 1].into_iter().enumerate() {
            let offset = REGISTRY_OFFSET + 3 * ENTRY_BYTES + index * 8;
            input[offset..offset + 8].copy_from_slice(&word.to_le_bytes());
        }
        input
    }

    /// input_with_registry_entries 造一份除条目数外一切合法的输入,用来把
    /// MAX_REGISTRY_ENTRIES 这个纯数字常量钉在可执行断言上——否则它被改动时
    /// 没有任何测试会变红。
    fn input_with_registry_entries(count: usize) -> Vec<u8> {
        let words_per_row = count.div_ceil(64);
        let mut input = vec![0; REGISTRY_OFFSET + count * ENTRY_BYTES + count * words_per_row * 8];
        input[0..4].copy_from_slice(b"MGM1");
        input[8..10].copy_from_slice(&(count as u16).to_le_bytes());
        input[10..12].copy_from_slice(&(words_per_row as u16).to_le_bytes());
        // air=0、barrier=1,条目 id 取 0..count 保证严格递增。
        input[12..14].copy_from_slice(&0_u16.to_le_bytes());
        input[14..16].copy_from_slice(&1_u16.to_le_bytes());
        for index in 0..count {
            let entry = REGISTRY_OFFSET + index * ENTRY_BYTES;
            input[entry..entry + 2].copy_from_slice(&(index as u16).to_le_bytes());
        }
        input
    }

    /// registry 条目上限必须正好是 MAX_REGISTRY_ENTRIES:恰好装满要被接受,多
    /// 一条要被拒绝。上限少于 Go 侧烘焙的条目数,整批 mesh 调用会被拒绝(水与
    /// 作物直接画不出来);多于 Go 侧上限,两侧对输入长度的期望就会分叉。
    ///
    /// 断言直接引用常量而不是把 48 抄成字面量:抄字面量的话,常量被改动时本用例
    /// 会跟着一起"正确",变成恒真的空转。真正把数字钉住的是 Go 侧
    /// TestNativeAcceptsRegistryAtGoCapacity——它拿 Go 的上限喂进本解析器。
    #[test]
    fn accepts_exactly_max_registry_entries() {
        assert!(MeshInput::parse(&input_with_registry_entries(MAX_REGISTRY_ENTRIES)).is_ok());
        assert_eq!(
            MeshInput::parse(&input_with_registry_entries(MAX_REGISTRY_ENTRIES + 1)).unwrap_err(),
            InputError::Registry
        );
    }

    #[test]
    fn parses_unaligned_little_endian_input_without_typed_casts() {
        let input = valid_input();
        let mut unaligned = vec![0xff];
        unaligned.extend_from_slice(&input);
        let parsed = MeshInput::parse(&unaligned[1..]).unwrap();

        assert_eq!(parsed.section_origin_y, -32);
        assert_eq!(parsed.air_id, 0);
        assert_eq!(parsed.barrier_id, 1);
        assert_eq!(parsed.block(1, 2, 3), 0x1234);
        assert_eq!(parsed.block(-17, 0, 0), 1);
        assert_eq!(parsed.sky_light(3, 0, 5), 15);
        assert_eq!(parsed.sky_light(3, 0, -17), 0);
        assert!(parsed.registry.opaque(1));
        assert!(!parsed.registry.opaque(30000));
        assert_eq!(parsed.registry.emission(40000), 7);
        assert_eq!(parsed.registry.emission(30000), 0);
        assert_eq!(parsed.registry.material(40000, 5), Some(40005));
        assert_eq!(parsed.registry.material(40000, 6), None);
        assert_eq!(parsed.registry.fluid_height(40000), Some(9));
        assert_eq!(parsed.registry.fluid_height(1), None);
        assert_eq!(parsed.registry.fluid_height(30000), None);
        assert_eq!(parsed.registry.light_attenuation(40000), 1);
        assert_eq!(parsed.registry.light_attenuation(1), 0);
        assert_eq!(parsed.registry.light_attenuation(30000), 0);
        assert!(parsed.registry.face_visible(1, 40000));
        assert!(!parsed.registry.face_visible(30000, 0));
    }

    /// model tag 的域读回：第 20 字节（offset 19）真的跨过 ABI 边界。
    ///
    /// 夹具刻意不改 `valid_input` 本身——它被 greedy/light/ffi 多个主题共享，
    /// 而是把石头（id=1）的 model 改写成 3（火把墙 −X）做局部验证：若编码
    /// 丢失会读回 0（默认），与未登记编号的缺省口径一致。
    #[test]
    fn model_byte_is_parsed_from_offset_nineteen() {
        let mut tagged = valid_input();
        tagged[REGISTRY_OFFSET + ENTRY_BYTES + 19] = 3;
        let parsed = MeshInput::parse(&tagged).unwrap();
        assert_eq!(parsed.registry.model(1), 3);
        assert_eq!(parsed.registry.model(0), 0, "空气条目保持默认 model 0");
        assert_eq!(parsed.registry.model(30000), 0, "未登记编号缺省为默认 0");

        // 边界对照：六个 tag（1..=5 火把五形态与 6=床）全部合法，逐个改写
        // 都要放行。
        for tag in 1_u8..=6 {
            let mut bytes = valid_input();
            bytes[REGISTRY_OFFSET + ENTRY_BYTES + 19] = tag;
            assert!(
                MeshInput::parse(&bytes).is_ok(),
                "火把/床 model tag={tag} 必须合法"
            );
        }
    }

    /// model tag 的封闭集合：7 起的未知值必须在条目校验就被拒绝（返回
    /// Registry 拒绝）——放行会让 mesher 静默回退到默认几何。拒绝发生在
    /// parse 期、任何几何产出之前。床 tag 6 已由床功能行消费（见上一条
    /// 边界对照），不再属于拒绝集合。
    #[test]
    fn unknown_model_tags_are_rejected() {
        for tag in [7_u8, 8, 255] {
            let mut bytes = valid_input();
            bytes[REGISTRY_OFFSET + ENTRY_BYTES + 19] = tag;
            assert_eq!(
                MeshInput::parse(&bytes).unwrap_err(),
                InputError::Registry,
                "model tag={tag} 未被拒绝"
            );
        }
    }

    /// block_top_raw 的域读回、fluid 互斥与哨兵语义。
    ///
    /// 夹具刻意不改 `valid_input` 本身——它被 greedy/light/ffi 多个主题共享，
    /// 把其中的石头改成短方块会让那些主题的期望集体失真——而是在本用例内
    /// 做局部改写。
    #[test]
    fn block_top_raw_readback_mutex_and_sentinel() {
        // 非流体条目（id=1 的石头）携带 14 合法，且能原样读回：证明第 19 字节
        // 真的跨过了 ABI 边界（若编码丢失会读回 None）。0 是满格哨兵、未登记
        // 编码同样返回 None，与 opaque/emission 的缺省口径一致。
        let mut sinkable = valid_input();
        sinkable[REGISTRY_OFFSET + ENTRY_BYTES + 18] = 14;
        let parsed = MeshInput::parse(&sinkable).unwrap();
        assert_eq!(parsed.registry.block_top_raw(1), Some(14));
        assert_eq!(parsed.registry.block_top_raw(0), None);
        assert_eq!(parsed.registry.block_top_raw(40000), None);
        assert_eq!(parsed.registry.block_top_raw(30000), None);

        // 与 fluid_height 互斥：id=40000 冒充流体（h_raw=9），再塞非零顶面
        // 高度必须整体拒绝——流体的角高度由 mesher 邻域平均现算，两条几何
        // 路径不得叠加在同一条目上。
        let mut conflict = valid_input();
        conflict[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 18] = 1;
        assert_eq!(
            MeshInput::parse(&conflict).unwrap_err(),
            InputError::Registry
        );

        // 边界对照：同一条目把顶面高度写回哨兵 0 后恢复合法，证明拒绝确实
        // 来自互斥规则而不是别的字段。
        let mut fluid_plain = valid_input();
        fluid_plain[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 18] = 0;
        assert!(MeshInput::parse(&fluid_plain).is_ok());
    }

    #[test]
    fn rejects_wrong_length_and_magic_as_input_errors() {
        let input = valid_input();
        assert_eq!(
            MeshInput::parse(&input[..input.len() - 1]).unwrap_err(),
            InputError::Input
        );
        let mut long = input.clone();
        long.push(0);
        assert_eq!(MeshInput::parse(&long).unwrap_err(), InputError::Input);
        let mut magic = input;
        magic[0] = b'X';
        assert_eq!(MeshInput::parse(&magic).unwrap_err(), InputError::Input);
    }

    #[test]
    fn rejects_malformed_registry_and_overbright_emission() {
        let mut unsorted = valid_input();
        unsorted[REGISTRY_OFFSET..REGISTRY_OFFSET + 2].copy_from_slice(&1_u16.to_le_bytes());
        assert_eq!(
            MeshInput::parse(&unsorted).unwrap_err(),
            InputError::Registry
        );

        let mut duplicate = valid_input();
        duplicate[REGISTRY_OFFSET + ENTRY_BYTES..REGISTRY_OFFSET + ENTRY_BYTES + 2]
            .copy_from_slice(&0_u16.to_le_bytes());
        assert_eq!(
            MeshInput::parse(&duplicate).unwrap_err(),
            InputError::Registry
        );

        let mut bad_opaque = valid_input();
        bad_opaque[REGISTRY_OFFSET + 2] = 2;
        assert_eq!(
            MeshInput::parse(&bad_opaque).unwrap_err(),
            InputError::Registry
        );

        let mut emission = valid_input();
        emission[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 3] = 16;
        assert_eq!(
            MeshInput::parse(&emission).unwrap_err(),
            InputError::Emission
        );

        // fluid_height 的合法域是 0..=14：15 被保留给「上方也是流体」的满格情形，
        // 只能由 mesher 现算，出现在条目里就是编码方写错了。
        let mut fluid_height = valid_input();
        fluid_height[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 16] = 15;
        assert_eq!(
            MeshInput::parse(&fluid_height).unwrap_err(),
            InputError::Registry
        );

        // light_attenuation 的合法域是 0..=1，2 就要被拒——这是 light::build_sky 分桶
        // 证明的前提（桶宽 = 1），不是天空光值域。挡在校验层，越界条目根本进不了 BFS。
        let mut attenuation = valid_input();
        attenuation[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 17] = 2;
        assert_eq!(
            MeshInput::parse(&attenuation).unwrap_err(),
            InputError::Registry
        );
        // 1 仍然合法：上面那条不是把整个字段一起否掉。
        let mut attenuation_one = valid_input();
        attenuation_one[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 17] = 1;
        assert!(MeshInput::parse(&attenuation_one).is_ok());

        // block_top_raw 的合法域是 0..=14：0 是「满格」哨兵，非零即短方块；
        // 15 无从表达任何合法几何（满格必须写哨兵 0），放行会破坏 mesher
        // 「非零即短」的单一判定。
        let mut block_top_raw = valid_input();
        block_top_raw[REGISTRY_OFFSET + ENTRY_BYTES + 18] = 15;
        assert_eq!(
            MeshInput::parse(&block_top_raw).unwrap_err(),
            InputError::Registry
        );

        let mut same_air_and_barrier = valid_input();
        same_air_and_barrier[14..16].copy_from_slice(&0_u16.to_le_bytes());
        assert_eq!(
            MeshInput::parse(&same_air_and_barrier).unwrap_err(),
            InputError::Registry
        );
    }
}
