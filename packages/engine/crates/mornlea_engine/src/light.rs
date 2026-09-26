use crate::input::{MeshInput, MeshViewAccess, RegistryView};
use crate::native::contracts::{KernelError, MeshScratch, MeshView};
use crate::quad::{Face, plant_material};

pub(crate) const LIGHT_MIN: i32 = -16;
pub(crate) const LIGHT_SIDE: usize = 48;
pub(crate) const LIGHT_VOLUME: usize = LIGHT_SIDE * LIGHT_SIDE * LIGHT_SIDE;
const SKY_MASK: u8 = 0xf0;
const BLOCK_MASK: u8 = 0x0f;
const DIRECTIONS: [(i32, i32, i32); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MeshError {
    EmissionOutOfRange,
    QueueOverflow,
}

pub(crate) struct LightScratch<'a> {
    levels: &'a mut [u8],
    queue: &'a mut [u32],
    tail: usize,
}

impl<'a> LightScratch<'a> {
    pub(crate) fn new(levels: &'a mut [u8], queue: &'a mut [u32]) -> Self {
        Self {
            levels,
            queue,
            tail: 0,
        }
    }

    pub(crate) fn at(&self, x: i32, y: i32, z: i32) -> u8 {
        if !inside(x, y, z) {
            return 0;
        }
        self.levels[light_index(x, y, z)]
    }

    fn enqueue(&mut self, index: usize) -> Result<(), MeshError> {
        if self.tail == self.queue.len() {
            return Err(MeshError::QueueOverflow);
        }
        self.queue[self.tail] = index as u32;
        self.tail += 1;
        Ok(())
    }

    fn reset_queue(&mut self) {
        self.tail = 0;
    }

    #[cfg(test)]
    fn tail(&self) -> usize {
        self.tail
    }
}

/// Read capability the shared light solver needs from one registry lane.
///
/// The byte ABI lane (`MeshInput` plus `RegistryView`) and the typed lane
/// (`MeshView` plus its contract registry) answer the same questions, so the
/// solver is generic over this trait and both lanes run exactly one
/// implementation of the sky and block light rules.
pub(crate) trait LightAccess {
    fn block(&self, x: i32, y: i32, z: i32) -> u16;
    fn sky_light(&self, x: i32, y: i32, z: i32) -> u8;
    fn air_id(&self) -> u16;
    fn contains(&self, id: u16) -> bool;
    fn opaque(&self, id: u16) -> bool;
    fn emission(&self, id: u16) -> u8;
    fn light_attenuation(&self, id: u16) -> u8;
    fn plant_block(&self, id: u16) -> bool;
}

/// Byte-lane access over the raw parser's borrowed input and registry
/// snapshot; both references stay owned by the ABI call frame.
struct RawLightAccess<'view, 'data> {
    input: &'view MeshInput<'data>,
    registry: &'view RegistryView<'data>,
}

impl LightAccess for RawLightAccess<'_, '_> {
    fn block(&self, x: i32, y: i32, z: i32) -> u16 {
        self.input.block(x, y, z)
    }

    fn sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        self.input.sky_light(x, y, z)
    }

    fn air_id(&self) -> u16 {
        self.input.air_id
    }

    fn contains(&self, id: u16) -> bool {
        self.registry.contains(id)
    }

    fn opaque(&self, id: u16) -> bool {
        self.registry.opaque(id)
    }

    fn emission(&self, id: u16) -> u8 {
        self.registry.emission(id)
    }

    fn light_attenuation(&self, id: u16) -> u8 {
        self.registry.light_attenuation(id)
    }

    fn plant_block(&self, id: u16) -> bool {
        self.registry
            .material(id, Face::NegX as usize)
            .is_some_and(plant_material)
    }
}

impl LightAccess for MeshViewAccess<'_> {
    fn block(&self, x: i32, y: i32, z: i32) -> u16 {
        MeshViewAccess::block(self, x, y, z)
    }

    fn sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        MeshViewAccess::sky_light(self, x, y, z)
    }

    fn air_id(&self) -> u16 {
        MeshViewAccess::air_id(self)
    }

    fn contains(&self, id: u16) -> bool {
        MeshViewAccess::contains(self, id)
    }

    fn opaque(&self, id: u16) -> bool {
        MeshViewAccess::opaque(self, id)
    }

    fn emission(&self, id: u16) -> u8 {
        MeshViewAccess::emission(self, id)
    }

    fn light_attenuation(&self, id: u16) -> u8 {
        MeshViewAccess::light_attenuation(self, id)
    }

    fn plant_block(&self, id: u16) -> bool {
        MeshViewAccess::material(self, id, Face::NegX as usize).is_some_and(plant_material)
    }
}

/// Raw byte-lane entry used by the engine ABI adapter; its behavior is pinned
/// by the ABI tests.
pub(crate) fn build_light(
    input: &MeshInput<'_>,
    registry: &RegistryView<'_>,
    scratch: &mut LightScratch<'_>,
) -> Result<(), MeshError> {
    build_light_core(&RawLightAccess { input, registry }, scratch)
}

/// Typed native entry: builds sky and block light for one validated view into
/// the typed scratch.
///
/// The caller (`native::mesh`) validates the view's registry first. Every build
/// clears the complete level volume and empties the queue before each pass, so
/// the caller may reuse one `MeshScratch` across sections without retained
/// light or queue state.
pub(crate) fn build_light_view(
    view: &MeshView<'_>,
    scratch: &mut MeshScratch,
) -> Result<(), KernelError> {
    let access = MeshViewAccess::new(view);
    let mut light = LightScratch::new(&mut scratch.levels[..], &mut scratch.queue[..]);
    build_light_core(&access, &mut light).map_err(|error| match error {
        MeshError::EmissionOutOfRange => KernelError::EmissionOutOfRange,
        MeshError::QueueOverflow => KernelError::QueueOverflow,
    })
}

/// Runs the two light passes over one lane, resetting all owned scratch state
/// first so a reused scratch can never carry levels or queue entries across
/// builds.
fn build_light_core(
    access: &impl LightAccess,
    scratch: &mut LightScratch<'_>,
) -> Result<(), MeshError> {
    scratch.levels.fill(0);
    scratch.reset_queue();
    build_sky(access, scratch)?;
    scratch.reset_queue();
    build_block(access, scratch)
}

fn build_sky(access: &impl LightAccess, scratch: &mut LightScratch<'_>) -> Result<(), MeshError> {
    let end = LIGHT_MIN + LIGHT_SIDE as i32;
    for x in LIGHT_MIN..end {
        for z in LIGHT_MIN..end {
            let mut direct = false;
            for y in (LIGHT_MIN..end).rev() {
                if access.sky_light(x, y, z) == 15 {
                    direct = true;
                }
                if !direct {
                    continue;
                }
                let id = access.block(x, y, z);
                if !access.contains(id)
                    || access.opaque(id)
                    || (id != access.air_id() && !access.plant_block(id))
                {
                    direct = false;
                    continue;
                }
                let index = light_index(x, y, z);
                scratch.levels[index] = SKY_MASK;
                scratch.enqueue(index)?;
            }
        }
    }

    // 按亮度降序分桶推进，每个桶再分两相位。逐列扫描先把露天空气以及它正下方
    // 连续的空气或植物写成满亮直射种子；其他透明方块会中断直射，只能由 BFS 进入。
    //
    // **为什么不能再用朴素 FIFO**：每步扣减改成按方块查表的 1 或 2 之后，FIFO 里的值
    // 不再单调不增，同一格可以先被 2 代价路径够到、再被 1 代价路径改进并重复入队。
    // 而队列容量是恰好 LIGHT_VOLUME（见 ffi.rs），溢出会一路变成 Go 侧渲染热路径上的
    // panic。固定扣减 1 的年代「每格至多入队一次」是显然的，这里必须把它挣回来。
    //
    // **前提（必读）**：本证明只在 `light_attenuation <= 1`、即**每格扣减只可能是 1 或 2**
    // 时成立。衰减到 2 就有 1/2/3 三种扣减，`B(L+1)` 产出的 L-2 会和 `A(L)` 产出的 L-1
    // 落进同一个桶段、桶不再单亮度，下面第 2 条随即失效、队列溢出。这个前提由
    // `RegistryView::validate`（以及 Go 侧 mesh.BuildRegistrySnapshot / encodeNativeInput）
    // 的 `> 1` 校验强制，越界条目根本到不了这里。真要支持 >= 2 的衰减，是一次独立变更：
    // 把分桶泛化成每个 step 一个桶，而不是放宽那条校验。
    //
    // **为什么这样就够**：设正在处理亮度 L 的桶。相位 A 只放松零衰减邻居（一律产出
    // L-1），相位 B 只放松有衰减邻居（在前提下一律产出恰好 L-2）。于是：
    //
    //   1. 相位 A 跑完时，所有本轮能拿到 L-1 的格子都已经定在 L-1；相位 B 递上来的
    //      L-2 对它们不是改进，被 `>= candidate` 挡掉，不会造成第二次入队；
    //   2. 只在相位 B 拿到 L-2 的格子，此后再无人能给它更高的值——L-1 只可能由 L 桶的
    //      相位 A 产生，而那一相位已经结束，更低的桶只会产出 <= L-2。
    //
    // 两条合起来：每格恰好在它的**最终**亮度上入队一次，容量 LIGHT_VOLUME 因此够用，
    // 也不存在需要跳过的陈旧队列项。守卫见 sky_queue_enqueues_each_cell_at_most_once_in_mixed_media。
    //
    // 桶在队列里天然连续：写入顺序是 A(L+1)→L、B(L+1)→L-1、A(L)→L-1、B(L)→L-2……
    // 所以 L-1 号桶正好是 B(L+1) 段紧接 A(L) 段，用 start/end 两个下标就能划出来。
    let mut start = 0;
    let mut end = scratch.tail;
    while start < scratch.tail {
        let deferred = spread(access, scratch, start..end, false)?;
        let next_end = scratch.tail;
        // 没有任何有衰减的邻居被推迟时整个相位 B 是空转，直接跳过：这让不含流体的
        // 世界维持与固定扣减时代逐格相同的工作量。
        if deferred {
            spread(access, scratch, start..end, true)?;
        }
        // 空桶会自愈：start 不动、end 前移到当前 tail，下一轮自然落到再下一个桶。
        start = end;
        end = next_end;
    }
    Ok(())
}

/// spread 把 `queue[slots]` 这一个桶里的格子向六邻放松一轮，返回是否**推迟**过邻居。
///
/// `attenuating` 选择本轮处理哪一类邻居：`false` 只处理 `light_attenuation == 0` 的
/// 邻居（扣减恰好 1），`true` 只处理有衰减的邻居（扣减至少 2）。同一个桶跑两遍、
/// 零衰减那遍在先，是「每格至多入队一次」的关键，推导见 `build_sky`。
///
/// 返回值只在 `attenuating == false` 时有意义：为真表示本轮至少跳过了一个有衰减的
/// 邻居，调用方据此决定要不要真的跑相位 B。
fn spread(
    access: &impl LightAccess,
    scratch: &mut LightScratch<'_>,
    slots: std::ops::Range<usize>,
    attenuating: bool,
) -> Result<bool, MeshError> {
    let mut deferred = false;
    for slot in slots {
        let mut index = scratch.queue[slot] as usize;
        let z = (index % LIGHT_SIDE) as i32 + LIGHT_MIN;
        index /= LIGHT_SIDE;
        let y = (index % LIGHT_SIDE) as i32 + LIGHT_MIN;
        let x = (index / LIGHT_SIDE) as i32 + LIGHT_MIN;
        let current = scratch.at(x, y, z) >> 4;
        // 最小可能扣减是 1，所以 current<=1 时任何邻居都拿不到正的光照。
        if current <= 1 {
            continue;
        }
        // best 是本格能给出的**最好**结果（扣减恰好为 1）。先拿它剪枝，已经不低于
        // best 的邻居连查表都不必做——这既省掉热路径上的两次二分查找，也保住了
        // 「稳定输入不再采样已定型邻居」这条既有性质。
        let best = current - 1;
        for (dx, dy, dz) in DIRECTIONS {
            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
            if !inside(nx, ny, nz) {
                continue;
            }
            let next = light_index(nx, ny, nz);
            if scratch.levels[next] >> 4 >= best {
                continue;
            }
            let id = access.block(nx, ny, nz);
            if !access.contains(id) || access.opaque(id) {
                continue;
            }
            let attenuation = access.light_attenuation(id);
            if (attenuation != 0) != attenuating {
                deferred |= !attenuating;
                continue;
            }
            // 每格扣减 = 固定的 1 + 目标方块查表得到的额外衰减。六个方向共用同一个
            // 公式，竖直向下没有任何特例：水的额外衰减是 1，于是每下沉一格扣 2。
            let step = 1 + attenuation;
            if current <= step {
                continue;
            }
            let candidate = current - step;
            if scratch.levels[next] >> 4 >= candidate {
                continue;
            }
            scratch.levels[next] = (scratch.levels[next] & BLOCK_MASK) | (candidate << 4);
            scratch.enqueue(next)?;
        }
    }
    Ok(deferred)
}

fn build_block(access: &impl LightAccess, scratch: &mut LightScratch<'_>) -> Result<(), MeshError> {
    let end = LIGHT_MIN + LIGHT_SIDE as i32;
    let mut source_counts = [0_usize; 16];
    for x in LIGHT_MIN..end {
        for y in LIGHT_MIN..end {
            for z in LIGHT_MIN..end {
                let level = access.emission(access.block(x, y, z));
                if level == 0 {
                    continue;
                }
                if level > 15 {
                    return Err(MeshError::EmissionOutOfRange);
                }
                source_counts[level as usize] += 1;
            }
        }
    }

    // 光源与传播都按亮度从高到低推进。生产 registry 只有 14/15 两档光源，先扫一遍
    // 计数后只为实际存在的档位重扫输入；16 个计数器是唯一新增状态，固定 scratch
    // 仍严格为 48³ levels + 48³ queue。
    //
    // 处理亮度 L 时，所有能生成更高候选值的桶都已结束；同一目标第一次写入的 L-1
    // 因而就是最终值。较低档光源若已被更高路径照亮也不会再次入队。每格最多入队一次，
    // 所以任意合法光源排列的 tail 都不超过 LIGHT_VOLUME。
    let mut start = 0;
    for level in (1_u8..=15).rev() {
        if source_counts[level as usize] != 0 {
            for x in LIGHT_MIN..end {
                for y in LIGHT_MIN..end {
                    for z in LIGHT_MIN..end {
                        if access.emission(access.block(x, y, z)) != level {
                            continue;
                        }
                        let index = light_index(x, y, z);
                        if scratch.levels[index] & BLOCK_MASK >= level {
                            continue;
                        }
                        scratch.levels[index] = (scratch.levels[index] & SKY_MASK) | level;
                        scratch.enqueue(index)?;
                    }
                }
            }
        }

        let bucket_end = scratch.tail;
        for slot in start..bucket_end {
            let mut index = scratch.queue[slot] as usize;
            let z = (index % LIGHT_SIDE) as i32 + LIGHT_MIN;
            index /= LIGHT_SIDE;
            let y = (index % LIGHT_SIDE) as i32 + LIGHT_MIN;
            let x = (index / LIGHT_SIDE) as i32 + LIGHT_MIN;
            debug_assert_eq!(scratch.at(x, y, z) & BLOCK_MASK, level);
            if level <= 1 {
                continue;
            }
            let candidate = level - 1;
            for (dx, dy, dz) in DIRECTIONS {
                let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                if !inside(nx, ny, nz) {
                    continue;
                }
                let next = light_index(nx, ny, nz);
                let id = access.block(nx, ny, nz);
                if scratch.levels[next] & BLOCK_MASK >= candidate
                    || !block_light_destination(access, id)
                {
                    continue;
                }
                scratch.levels[next] = (scratch.levels[next] & SKY_MASK) | candidate;
                scratch.enqueue(next)?;
            }
        }
        start = bucket_end;
    }
    Ok(())
}

// block_light_destination 只放行空气与离散植物材质；未登记编号因无材质而关闭。
fn block_light_destination(access: &impl LightAccess, id: u16) -> bool {
    id == access.air_id() || access.plant_block(id)
}

fn inside(x: i32, y: i32, z: i32) -> bool {
    let end = LIGHT_MIN + LIGHT_SIDE as i32;
    (LIGHT_MIN..end).contains(&x) && (LIGHT_MIN..end).contains(&y) && (LIGHT_MIN..end).contains(&z)
}

fn light_index(x: i32, y: i32, z: i32) -> usize {
    (((x - LIGHT_MIN) as usize * LIGHT_SIDE + (y - LIGHT_MIN) as usize) * LIGHT_SIDE)
        + (z - LIGHT_MIN) as usize
}

#[cfg(test)]
mod tests {
    use super::{
        LIGHT_VOLUME, LightScratch, MeshError, build_light, build_light_core, build_light_view,
    };
    use crate::input::{MeshInput, tests::valid_input};
    use crate::native::contracts::{
        MeshModel, MeshRegistry, MeshRegistryEntry, MeshScratch, MeshView,
    };

    const BLOCKS_OFFSET: usize = 16;
    const BLOCKS_BYTES: usize = 27 * 4096 * 2;
    const HEIGHTS_PRESENT_OFFSET: usize = BLOCKS_OFFSET + BLOCKS_BYTES;
    const HEIGHTS_OFFSET: usize = HEIGHTS_PRESENT_OFFSET + 9;
    const REGISTRY_OFFSET: usize = HEIGHTS_OFFSET + 9 * 256 * 2;
    const ENTRY_BYTES: usize = crate::input::tests::ENTRY_BYTES;
    const LIGHT_ID: u16 = 40000;

    struct InputFixture {
        mesh: MeshInput<'static>,
    }

    struct ScratchFixture {
        light: LightScratch<'static>,
    }

    impl ScratchFixture {
        fn new() -> Self {
            let levels = Box::leak(vec![0; LIGHT_VOLUME].into_boxed_slice());
            let queue = Box::leak(vec![0; LIGHT_VOLUME].into_boxed_slice());
            Self {
                light: LightScratch::new(levels, queue),
            }
        }
    }

    #[test]
    fn single_block_light_reaches_fourteen_next_door() {
        let input = fixture_with_light_block(8, 8, 8);
        let mut storage = ScratchFixture::new();
        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();
        assert_eq!(storage.light.at(8, 8, 8) & 0x0f, 15);
        assert_eq!(storage.light.at(9, 8, 8) & 0x0f, 14);
    }

    #[test]
    fn all_sources_fill_exact_queue_without_overflow() {
        let input = fixture_all_light_blocks();
        let mut storage = ScratchFixture::new();
        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();
        assert_eq!(storage.light.tail(), LIGHT_VOLUME);
    }

    #[test]
    fn one_short_queue_reports_overflow() {
        let input = fixture_all_light_blocks();
        let levels = Box::leak(vec![0; LIGHT_VOLUME].into_boxed_slice());
        let queue = Box::leak(vec![0; LIGHT_VOLUME - 1].into_boxed_slice());
        let mut light = LightScratch::new(levels, queue);

        assert_eq!(
            build_light(&input.mesh, &input.mesh.registry, &mut light),
            Err(MeshError::QueueOverflow),
        );
    }

    #[test]
    fn emission_sixteen_is_rejected() {
        let input = fixture_with_emission(16);
        let mut storage = ScratchFixture::new();
        assert_eq!(
            build_light(&input.mesh, &input.mesh.registry, &mut storage.light),
            Err(MeshError::EmissionOutOfRange),
        );
    }

    #[test]
    fn direct_sky_seed_is_fifteen() {
        let input = fixture_with_sky_column(8, 8, 7);
        let mut storage = ScratchFixture::new();

        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.at(8, 8, 8) >> 4, 15);
    }

    #[test]
    fn sky_light_attenuates_to_fourteen_next_door() {
        let input = fixture_with_sky_column(8, 8, 7);
        let mut storage = ScratchFixture::new();

        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.at(9, 8, 8) >> 4, 14);
    }

    #[test]
    fn opaque_cell_blocks_sky_propagation() {
        let mut bytes = base_input(0);
        set_height(&mut bytes, 8, 8, Some(7));
        set_block(&mut bytes, 9, 8, 8, 1);
        let input = parse_fixture(bytes);
        let mut storage = ScratchFixture::new();

        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.at(9, 8, 8) >> 4, 0);
    }

    #[test]
    fn missing_height_stays_dark() {
        let input = parse_fixture(base_input(0));
        let mut storage = ScratchFixture::new();

        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.at(8, 8, 8) >> 4, 0);
    }

    #[test]
    fn queue_is_reused_between_sky_and_block_passes() {
        let mut bytes = base_input(15);
        for x in -16..32 {
            for z in -16..32 {
                set_height(&mut bytes, x, z, Some(-17));
            }
        }
        set_block(&mut bytes, 8, 8, 8, LIGHT_ID);
        let input = parse_fixture(bytes);
        let mut storage = ScratchFixture::new();

        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.tail(), 4089);
        assert_eq!(storage.light.at(9, 8, 8) & 0x0f, 14);
    }

    #[test]
    fn outside_light_volume_returns_zero() {
        let input = fixture_with_light_block(8, 8, 8);
        let mut storage = ScratchFixture::new();
        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.at(-17, 0, 0), 0);
        assert_eq!(storage.light.at(32, 0, 0), 0);
        assert_eq!(storage.light.at(0, -17, 0), 0);
        assert_eq!(storage.light.at(0, 32, 0), 0);
        assert_eq!(storage.light.at(0, 0, -17), 0);
        assert_eq!(storage.light.at(0, 0, 32), 0);
    }

    #[test]
    fn reused_scratch_clears_old_light_before_a_dark_build() {
        let lit = fixture_with_light_block(8, 8, 8);
        let dark = parse_fixture(base_input(0));
        let mut storage = ScratchFixture::new();

        build_light(&lit.mesh, &lit.mesh.registry, &mut storage.light).unwrap();
        assert_eq!(storage.light.at(8, 8, 8) & 0x0f, 15);
        build_light(&dark.mesh, &dark.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.at(8, 8, 8), 0);
        assert_eq!(storage.light.at(9, 8, 8), 0);
    }

    #[test]
    fn non_plant_non_air_non_opaque_block_stops_block_light() {
        let mut bytes = base_input(15);
        for block in bytes[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].chunks_exact_mut(2) {
            block.copy_from_slice(&1_u16.to_le_bytes());
        }
        bytes[REGISTRY_OFFSET + ENTRY_BYTES + 2] = 0;
        set_block(&mut bytes, 8, 8, 8, LIGHT_ID);
        set_block(&mut bytes, 10, 8, 8, 0);
        let input = parse_fixture(bytes);
        let mut storage = ScratchFixture::new();

        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.at(9, 8, 8) & 0x0f, 0);
        assert_eq!(storage.light.at(10, 8, 8) & 0x0f, 0);
    }

    #[test]
    fn crop_and_short_grass_plant_block_light_steps_down_normally() {
        for material in [31_u16, 68] {
            let mut bytes = base_input(15);
            for block in bytes[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].chunks_exact_mut(2) {
                block.copy_from_slice(&1_u16.to_le_bytes());
            }
            let plant = REGISTRY_OFFSET + ENTRY_BYTES;
            bytes[plant + 2] = 0;
            for face in 0..6 {
                bytes[plant + 4 + face * 2..plant + 6 + face * 2]
                    .copy_from_slice(&material.to_le_bytes());
            }
            set_block(&mut bytes, 8, 8, 8, LIGHT_ID);
            set_block(&mut bytes, 10, 8, 8, 0);
            let input = parse_fixture(bytes);
            let mut storage = ScratchFixture::new();

            build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

            assert_eq!(
                storage.light.at(9, 8, 8) & 0x0f,
                14,
                "植物材质层 {material} 未让方块光进入相邻格"
            );
            assert_eq!(
                storage.light.at(10, 8, 8) & 0x0f,
                13,
                "植物材质层 {material} 后方空气未继续传播"
            );
        }
    }

    #[test]
    fn crop_and_short_grass_plant_direct_sky_stays_fifteen() {
        for material in [31_u16, 68] {
            let mut bytes = base_input(0);
            for x in -16..32 {
                for z in -16..32 {
                    set_height(&mut bytes, x, z, Some(-17));
                }
            }
            let plant = REGISTRY_OFFSET + ENTRY_BYTES;
            bytes[plant + 2] = 0;
            for face in 0..6 {
                bytes[plant + 4 + face * 2..plant + 6 + face * 2]
                    .copy_from_slice(&material.to_le_bytes());
            }
            set_block(&mut bytes, 8, 8, 8, 1);
            set_height(&mut bytes, 8, 8, Some(8));
            let input = parse_fixture(bytes);
            let mut storage = ScratchFixture::new();

            build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

            assert_eq!(
                storage.light.at(8, 8, 8) >> 4,
                15,
                "植物材质层 {material} 的直射天空光不是 15"
            );
            assert_eq!(
                storage.light.at(8, 7, 8) >> 4,
                15,
                "植物材质层 {material} 正下方的直射天空光不是 15"
            );
        }
    }

    #[test]
    fn unknown_block_stops_plant_sky_seed_and_propagation() {
        let mut bytes = base_input(0);
        for block in bytes[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].chunks_exact_mut(2) {
            block.copy_from_slice(&1_u16.to_le_bytes());
        }
        for x in -16..32 {
            for z in -16..32 {
                set_height(&mut bytes, x, z, Some(31));
            }
        }
        for y in 10..32 {
            set_block(&mut bytes, 8, y, 8, 0);
        }
        set_block(&mut bytes, 8, 9, 8, 60000);
        set_block(&mut bytes, 8, 8, 8, 0);
        set_height(&mut bytes, 8, 8, Some(9));
        let input = parse_fixture(bytes);
        let mut storage = ScratchFixture::new();

        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.at(8, 9, 8) >> 4, 0);
        assert_eq!(storage.light.at(8, 8, 8) >> 4, 0);

        let mut stale = base_input(0);
        for block in stale[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].chunks_exact_mut(2) {
            block.copy_from_slice(&1_u16.to_le_bytes());
        }
        for x in -16..32 {
            for z in -16..32 {
                set_height(&mut stale, x, z, Some(31));
            }
        }
        set_block(&mut stale, 8, 9, 8, 60000);
        set_block(&mut stale, 8, 8, 8, 0);
        set_height(&mut stale, 8, 8, Some(8));
        let stale = parse_fixture(stale);
        let mut stale_storage = ScratchFixture::new();

        build_light(&stale.mesh, &stale.mesh.registry, &mut stale_storage.light).unwrap();

        assert_eq!(stale_storage.light.at(8, 9, 8) >> 4, 0);
        assert_eq!(stale_storage.light.at(8, 8, 8) >> 4, 0);
    }

    #[test]
    fn unknown_block_stops_plant_block_light() {
        let mut bytes = base_input(15);
        for block in bytes[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].chunks_exact_mut(2) {
            block.copy_from_slice(&1_u16.to_le_bytes());
        }
        bytes[REGISTRY_OFFSET + ENTRY_BYTES + 2] = 0;
        set_block(&mut bytes, 8, 8, 8, LIGHT_ID);
        set_block(&mut bytes, 9, 8, 8, 60000);
        set_block(&mut bytes, 10, 8, 8, 0);
        let input = parse_fixture(bytes);
        let mut storage = ScratchFixture::new();

        build_light(&input.mesh, &input.mesh.registry, &mut storage.light).unwrap();

        assert_eq!(storage.light.at(9, 8, 8) & 0x0f, 0);
        assert_eq!(storage.light.at(10, 8, 8) & 0x0f, 0);
    }

    #[test]
    fn crop_and_short_grass_plant_block_light_mixed_sources_fit_fixed_queue() {
        for material in [31_u16, 68] {
            let mut bytes = base_input(15);
            for block in bytes[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].chunks_exact_mut(2) {
                block.copy_from_slice(&LIGHT_ID.to_le_bytes());
            }
            let plant = REGISTRY_OFFSET + ENTRY_BYTES;
            bytes[plant + 2] = 0;
            for face in 0..6 {
                bytes[plant + 4 + face * 2..plant + 6 + face * 2]
                    .copy_from_slice(&material.to_le_bytes());
            }
            bytes[REGISTRY_OFFSET + 3] = 14;
            set_block(&mut bytes, -16, -16, -16, 1);
            set_block(&mut bytes, -16, -16, -15, 0);
            let input = parse_fixture(bytes);
            let mut storage = ScratchFixture::new();

            build_light(&input.mesh, &input.mesh.registry, &mut storage.light)
                .expect("混合 14/15 光源让固定方块光队列溢出");

            assert_eq!(storage.light.at(-16, -16, -16) & 0x0f, 14);
            assert_eq!(storage.light.tail(), LIGHT_VOLUME);
        }
    }

    /// 天空光穿过流体时每格额外衰减：固定的 1 加上查表得到的 `light_attenuation`。
    ///
    /// 水的 `light_attenuation = 1`，所以每下沉一格扣 2：15 → 13 → 11 → … → 1 → 0。
    /// **竖直向下不再无损**：流体格即使严格高于列顶也不再是直射起点，光只能从上方的
    /// 空气逐格付费进入。
    #[test]
    fn sky_light_dims_by_two_per_fluid_cell() {
        let water = open_water_fixture(LIGHT_ID);
        // 夹具把 LIGHT_ID 当水用，靠的是 valid_input() 给它写了 light_attenuation=1
        // 这一**隐式**事实（base_input 显式清零了 fluid_height 那个字节，却没碰它）。
        // 显式钉住，免得夹具改动悄悄把「水」变成普通透明方块、整组断言退化。
        assert_eq!(water.mesh.registry.light_attenuation(LIGHT_ID), 1);
        let mut storage = ScratchFixture::new();
        build_light(&water.mesh, &water.mesh.registry, &mut storage.light).unwrap();

        // 水面之上的空气仍然是满亮直射起点。
        assert_eq!(storage.light.at(8, 8, 8) >> 4, 15);
        // 紧邻水面之下必须大于 0，且逐格严格变暗，足够深处到 0。
        let want = [13, 11, 9, 7, 5, 3, 1, 0];
        for (depth, expected) in want.into_iter().enumerate() {
            let y = 7 - depth as i32;
            assert_eq!(
                storage.light.at(8, y, 8) >> 4,
                expected,
                "水下第 {} 格（y={y}）的天空光不符",
                depth + 1,
            );
        }

        // 防空转守卫排在真实断言之后：把水换成空气后同样八格必须全是 15。
        // 若对照组也变暗，说明变暗来自夹具被封顶之类的原因，而不是流体衰减，
        // 上面那串读数就不再证明任何事。
        let air = open_water_fixture(0);
        let mut control = ScratchFixture::new();
        build_light(&air.mesh, &air.mesh.registry, &mut control.light).unwrap();
        for y in 0..8 {
            assert_eq!(
                control.light.at(8, y, 8) >> 4,
                15,
                "空气对照组 y={y} 不是满亮，夹具本身就是暗的"
            );
        }
    }

    /// 流体透光：水下的光来自 BFS 逐格付费，而不是「被流体完全阻断」。
    /// 与 `opaque_cell_blocks_sky_propagation` 成对——不透明格是 0，流体格不是。
    #[test]
    fn fluid_attenuates_instead_of_blocking() {
        let water = open_water_fixture(LIGHT_ID);
        let mut storage = ScratchFixture::new();
        build_light(&water.mesh, &water.mesh.registry, &mut storage.light).unwrap();

        assert!(storage.light.at(8, 7, 8) >> 4 > 0);
        assert!(storage.light.at(8, 7, 8) >> 4 < 15);
    }

    /// mixed_media_fixture 造一份**队列压力最大**的天空光输入：
    ///
    /// - 8 个外围区段列的列顶压到 `-17`（低于整个光照体积），于是它们的每一格都是
    ///   满亮直射种子；中心区段列加一层 `roof` 高的顶盖，那 16×16 区域全靠 BFS 侧向灌入；
    /// - 方块按 4×4 的**水柱／空气柱棋盘**交替铺满全高。
    ///
    /// 这两件事合起来正是重复入队的温床：中心暗区的同一格会先被穿水的 2 代价路径够到、
    /// 再被绕行的 1 代价路径改进一次。而整个 48³ 体积恰好全部有光，队列一格不剩，
    /// **任何一次重复入队都立刻溢出**。
    fn mixed_media_fixture() -> InputFixture {
        let mut bytes = base_input(0);
        for cx in [-16_i32, 0, 16] {
            for cz in [-16_i32, 0, 16] {
                let highest = if cx == 0 && cz == 0 { 31 } else { -17 };
                fill_height_section(&mut bytes, cx, cz, highest);
            }
        }
        for x in -16_i32..32 {
            for z in -16_i32..32 {
                if (x.div_euclid(4) + z.div_euclid(4)).rem_euclid(2) != 0 {
                    continue;
                }
                for y in -16_i32..32 {
                    set_block(&mut bytes, x, y, z, LIGHT_ID);
                }
            }
        }
        parse_fixture(bytes)
    }

    /// 天空光队列容量的承重守卫：**每格至多入队一次**，因此恰好 `LIGHT_VOLUME` 够用。
    ///
    /// 这条不变量在「每步扣减固定为 1」的年代是显然的（BFS 值严格单调不增，一格被改进
    /// 之后不可能再被改进）。扣减改成按方块查表的 1 或 2 之后它**不再显然**：FIFO 里的值
    /// 不再单调，同一格可以先被 2 代价路径够到、再被 1 代价路径改进并重复入队。
    /// `build_sky` 因此改成按亮度降序分桶、每桶先放松零衰减邻居再放松有衰减邻居，
    /// 重新把「至多一次」变成可证的（推导见 `build_sky` 的注释）。
    ///
    /// 队列容量是 `ffi.rs` 里写死的 `LIGHT_VOLUME`，溢出会一路变成 Go 侧
    /// `internal/mesh/native.go` 在渲染热路径上的 panic，所以这条必须钉死。
    /// 本夹具下整个体积恰好全部有光，队列一格不剩：不变量一破就是真溢出，不是余量问题。
    #[test]
    fn sky_queue_enqueues_each_cell_at_most_once_in_mixed_media() {
        let input = mixed_media_fixture();
        let levels = Box::leak(vec![0; LIGHT_VOLUME].into_boxed_slice());
        let queue = Box::leak(vec![0; LIGHT_VOLUME].into_boxed_slice());
        let mut light = LightScratch::new(levels, queue);

        let access = super::RawLightAccess {
            input: &input.mesh,
            registry: &input.mesh.registry,
        };
        super::build_sky(&access, &mut light)
            .expect("天空光队列溢出：每格至多入队一次的不变量已被破坏");

        let mut lit = 0;
        for x in -16..32 {
            for y in -16..32 {
                for z in -16..32 {
                    if light.at(x, y, z) >> 4 > 0 {
                        lit += 1;
                    }
                }
            }
        }
        // 夹具前提守卫排在真实断言之后：整个体积必须真的全部有光，否则队列还有余量，
        // 上面那句 expect 就不再是「一次重复入队即溢出」的严格判据。
        assert_eq!(
            light.tail(),
            lit,
            "入队 {} 次但只有 {lit} 格有光：存在重复入队",
            light.tail(),
        );
        assert_eq!(
            lit, LIGHT_VOLUME,
            "夹具没有把整个光照体积点亮，队列仍有余量"
        );
    }

    fn fixture_with_light_block(x: i32, y: i32, z: i32) -> InputFixture {
        let mut bytes = base_input(15);
        set_block(&mut bytes, x, y, z, LIGHT_ID);
        parse_fixture(bytes)
    }

    fn fixture_all_light_blocks() -> InputFixture {
        let mut bytes = base_input(15);
        for block in bytes[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].chunks_exact_mut(2) {
            block.copy_from_slice(&LIGHT_ID.to_le_bytes());
        }
        parse_fixture(bytes)
    }

    fn fixture_with_emission(emission: u8) -> InputFixture {
        let mut bytes = base_input(emission);
        set_block(&mut bytes, 8, 8, 8, LIGHT_ID);
        let bytes = Box::leak(bytes.into_boxed_slice());
        InputFixture {
            mesh: MeshInput::parse_allowing_overbright(bytes).unwrap(),
        }
    }

    fn fixture_with_sky_column(x: i32, z: i32, highest: i16) -> InputFixture {
        let mut bytes = base_input(0);
        darken_height_section(&mut bytes, x, z);
        set_height(&mut bytes, x, z, Some(highest));
        parse_fixture(bytes)
    }

    fn base_input(emission: u8) -> Vec<u8> {
        let mut bytes = valid_input();
        bytes[4..8].copy_from_slice(&0_i32.to_le_bytes());
        bytes[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].fill(0);
        bytes[HEIGHTS_PRESENT_OFFSET..HEIGHTS_PRESENT_OFFSET + 9].fill(0);
        bytes[HEIGHTS_OFFSET..REGISTRY_OFFSET].fill(0);
        bytes[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 3] = emission;
        bytes[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 16] = 0;
        bytes
    }

    fn set_block(bytes: &mut [u8], x: i32, y: i32, z: i32, id: u16) {
        let (cx, lx) = neighbor_cell(x);
        let (cy, ly) = neighbor_cell(y);
        let (cz, lz) = neighbor_cell(z);
        let section = (cx * 3 + cy) * 3 + cz;
        let cell = (ly << 8) | (lz << 4) | lx;
        let offset = BLOCKS_OFFSET + (section * 4096 + cell) * 2;
        bytes[offset..offset + 2].copy_from_slice(&id.to_le_bytes());
    }

    fn set_height(bytes: &mut [u8], x: i32, z: i32, highest: Option<i16>) {
        let (cx, lx) = neighbor_cell(x);
        let (cz, lz) = neighbor_cell(z);
        let column = cx * 3 + cz;
        bytes[HEIGHTS_PRESENT_OFFSET + column] = u8::from(highest.is_some());
        let offset = HEIGHTS_OFFSET + (column * 256 + (lz << 4) + lx) * 2;
        bytes[offset..offset + 2].copy_from_slice(&highest.unwrap_or_default().to_le_bytes());
    }

    fn darken_height_section(bytes: &mut [u8], x: i32, z: i32) {
        fill_height_section(bytes, x, z, 31);
    }

    /// fill_height_section 把 (x,z) 所在整个 16×16 区段列的列顶统一设成 highest。
    fn fill_height_section(bytes: &mut [u8], x: i32, z: i32, highest: i16) {
        let (cx, _) = neighbor_cell(x);
        let (cz, _) = neighbor_cell(z);
        let column = cx * 3 + cz;
        bytes[HEIGHTS_PRESENT_OFFSET + column] = 1;
        for cell in 0..256 {
            let offset = HEIGHTS_OFFSET + (column * 256 + cell) * 2;
            bytes[offset..offset + 2].copy_from_slice(&highest.to_le_bytes());
        }
    }

    /// open_water_fixture 造一片**露天水体**：`fill` 铺满中心 16×16 区段列的
    /// y=0..=7 八格，其上是空气，没有任何遮挡。
    ///
    /// `fill` 传 `LIGHT_ID` 时就是水：在 `base_input(0)` 下它正好是一块「非不透明、
    /// 不发光、`light_attenuation = 1`」的方块。传 `0`（空气）就是同一夹具的对照组。
    ///
    /// 列顶按 `fill` 派生，与 `world.Chunk` 的 `updateHeight` 同口径：水是非空气方块，
    /// 会把列顶抬到水面 `y=7`（于是水格自身 `sky_light=0`、只能靠 BFS 进光）；空气
    /// 对照组则是空列（-17，低于整个光照体积）。
    ///
    /// 夹具刻意做成整段 16×16 满铺八格深：**单列水柱是测不出衰减的**——旁边的空气
    /// 会以每格 1 的代价把光送到同样深度再横向灌进来，读数被空气路径而不是水路径
    /// 决定。满铺之后中心列 (8,·,8) 的每个横向邻居都是同深度的水，唯一路径是竖直
    /// 穿水。八格深也是刻意的：一格深时「衰减 1」与「衰减 0」会给出同一批读数。
    fn open_water_fixture(fill: u16) -> InputFixture {
        let mut bytes = base_input(0);
        let highest = if fill == 0 { -17 } else { 7 };
        fill_height_section(&mut bytes, 8, 8, highest);
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..8 {
                    set_block(&mut bytes, x, y, z, fill);
                }
            }
        }
        parse_fixture(bytes)
    }

    fn neighbor_cell(value: i32) -> (usize, usize) {
        let shifted = value + 16;
        ((shifted >> 4) as usize, (shifted & 15) as usize)
    }

    fn parse_fixture(bytes: Vec<u8>) -> InputFixture {
        let bytes = Box::leak(bytes.into_boxed_slice());
        InputFixture {
            mesh: MeshInput::parse(bytes).unwrap(),
        }
    }

    /// Typed-lane fixtures: a small registry plus one owned 3x3x3 neighborhood
    /// view so the shared light core can be exercised through the typed view.
    const TYPED_AIR: u16 = 0;
    const TYPED_STONE: u16 = 1;
    const TYPED_LAMP: u16 = 2;
    const TYPED_BLOCKS: usize = 27 * 4096;

    fn typed_entry(id: u16, emission: u8) -> MeshRegistryEntry {
        MeshRegistryEntry {
            id,
            opaque: id == TYPED_STONE,
            emission,
            material: [0; 6],
            fluid_height: 0,
            light_attenuation: 0,
            block_top_raw: 0,
            model: MeshModel::Default,
        }
    }

    /// The lamp entry is the only emitter; `emission` lets a rebuild reuse the
    /// same blocks with a dark registry to expose retained light state.
    fn typed_registry(emission: u8) -> MeshRegistry {
        let entries = [
            typed_entry(TYPED_AIR, 0),
            typed_entry(TYPED_STONE, 0),
            typed_entry(TYPED_LAMP, emission),
        ];
        let words = entries.len() * entries.len().div_ceil(64);
        MeshRegistry::try_new(&entries, &vec![0_u64; words], TYPED_AIR, TYPED_STONE).unwrap()
    }

    struct TypedFixture {
        blocks: Box<[u16; TYPED_BLOCKS]>,
        heights_present: Box<[bool; 9]>,
        heights: Box<[[i16; 256]; 9]>,
    }

    impl TypedFixture {
        fn new() -> Self {
            Self {
                blocks: Box::new([0; TYPED_BLOCKS]),
                heights_present: Box::new([false; 9]),
                heights: Box::new([[0; 256]; 9]),
            }
        }

        fn set_block(&mut self, x: i32, y: i32, z: i32, id: u16) {
            self.blocks[typed_cell(x, y, z)] = id;
        }

        /// Pins one height cell inside its 16x16 section column. Presence is a
        /// per-column flag, so callers must first darken or fill the section
        /// (`set_height_section`) before opening a single cell.
        fn set_height(&mut self, x: i32, z: i32, highest: i16) {
            let (column, cell) = typed_height_slot(x, z);
            self.heights[column][cell] = highest;
        }

        /// Mirrors the byte lane's `fill_height_section`: marks the whole
        /// section column present with one pinned height.
        fn set_height_section(&mut self, x: i32, z: i32, highest: i16) {
            let (cx, _) = neighbor_cell(x);
            let (cz, _) = neighbor_cell(z);
            let column = cx * 3 + cz;
            self.heights_present[column] = true;
            for cell in 0..256 {
                self.heights[column][cell] = highest;
            }
        }

        fn view<'a>(&'a self, registry: &'a MeshRegistry) -> MeshView<'a> {
            MeshView {
                blocks: &self.blocks,
                heights_present: &self.heights_present,
                heights: &self.heights,
                section_origin_y: 0,
                registry,
            }
        }
    }

    /// Test-local mirror of the typed neighborhood indexing, used to place
    /// fixture cells and to read levels back out of the scratch.
    fn typed_cell(x: i32, y: i32, z: i32) -> usize {
        let (cx, lx) = neighbor_cell(x);
        let (cy, ly) = neighbor_cell(y);
        let (cz, lz) = neighbor_cell(z);
        ((cx * 3 + cy) * 3 + cz) * 4096 + ((ly << 8) | (lz << 4) | lx)
    }

    fn typed_height_slot(x: i32, z: i32) -> (usize, usize) {
        let (cx, lx) = neighbor_cell(x);
        let (cz, lz) = neighbor_cell(z);
        (cx * 3 + cz, (lz << 4) | lx)
    }

    /// Reads one cell out of the typed scratch level volume. The volume has
    /// its own index space, so the block-cell index cannot be reused here.
    fn typed_level(scratch: &MeshScratch, x: i32, y: i32, z: i32) -> u8 {
        scratch.levels[super::light_index(x, y, z)]
    }

    #[test]
    fn typed_emitter_lights_fifteen_and_next_door_fourteen() {
        let registry = typed_registry(15);
        let mut fixture = TypedFixture::new();
        fixture.set_block(8, 8, 8, TYPED_LAMP);
        let view = fixture.view(&registry);
        let mut scratch = MeshScratch::try_new().unwrap();

        build_light_view(&view, &mut scratch).unwrap();

        assert_eq!(typed_level(&scratch, 8, 8, 8) & 0x0f, 15);
        assert_eq!(typed_level(&scratch, 9, 8, 8) & 0x0f, 14);
    }

    #[test]
    fn typed_direct_sky_seed_is_fifteen_and_spreads_to_fourteen() {
        let registry = typed_registry(0);
        let mut fixture = TypedFixture::new();
        fixture.set_height_section(8, 8, 31);
        fixture.set_height(8, 8, 7);
        let view = fixture.view(&registry);
        let mut scratch = MeshScratch::try_new().unwrap();

        build_light_view(&view, &mut scratch).unwrap();

        assert_eq!(typed_level(&scratch, 8, 8, 8) >> 4, 15);
        assert_eq!(typed_level(&scratch, 8, 8, 8) & 0x0f, 0);
        assert_eq!(typed_level(&scratch, 9, 8, 8) >> 4, 14);
    }

    #[test]
    fn typed_unknown_block_stops_block_light_propagation() {
        let registry = typed_registry(15);
        let mut fixture = TypedFixture::new();
        // Solid stone everywhere except one lamp, one unknown id and one air
        // cell: an open volume would let light detour around the unknown id.
        fixture.blocks.fill(TYPED_STONE);
        fixture.set_block(8, 8, 8, TYPED_LAMP);
        fixture.set_block(9, 8, 8, 60000);
        fixture.set_block(10, 8, 8, 0);
        let view = fixture.view(&registry);
        let mut scratch = MeshScratch::try_new().unwrap();

        build_light_view(&view, &mut scratch).unwrap();

        assert_eq!(typed_level(&scratch, 9, 8, 8) & 0x0f, 0);
        assert_eq!(typed_level(&scratch, 10, 8, 8) & 0x0f, 0);
    }

    /// Bright-then-dark reuse of one shared scratch: the second build must
    /// start from a cleared level volume and an empty queue. A third rebuild of
    /// the bright section must then match a fresh scratch exactly, so neither
    /// retained levels nor a retained queue tail can leak across calls.
    #[test]
    fn typed_reuse_resets_levels_and_queue_between_bright_and_dark() {
        let registry = typed_registry(15);
        let mut bright_fixture = TypedFixture::new();
        bright_fixture.set_block(8, 8, 8, TYPED_LAMP);
        let dark_fixture = TypedFixture::new();
        let bright = bright_fixture.view(&registry);
        let dark = dark_fixture.view(&registry);

        let mut scratch = MeshScratch::try_new().unwrap();
        build_light_view(&bright, &mut scratch).unwrap();
        assert!(
            scratch.levels.iter().any(|&level| level != 0),
            "bright section must light the level volume"
        );

        build_light_view(&dark, &mut scratch).unwrap();
        assert!(
            scratch.levels.iter().all(|&level| level == 0),
            "dark rebuild retained light from the bright section"
        );

        build_light_view(&bright, &mut scratch).unwrap();
        let mut fresh = MeshScratch::try_new().unwrap();
        build_light_view(&bright, &mut fresh).unwrap();
        assert_eq!(
            &scratch.levels[..],
            &fresh.levels[..],
            "rebuild after reuse diverged from a fresh scratch"
        );
    }

    /// Queue capacity is exactly one enqueue per cell: the lamp registry fills
    /// the whole 48^3 level volume with sources, so the exact-capacity queue
    /// completes and a queue one word short overflows.
    #[test]
    fn typed_queue_exact_fill_completes_and_one_short_overflows() {
        let registry = typed_registry(15);
        let mut fixture = TypedFixture::new();
        fixture.blocks.fill(TYPED_LAMP);
        let view = fixture.view(&registry);
        let mut scratch = MeshScratch::try_new().unwrap();
        let access = crate::input::MeshViewAccess::new(&view);

        let mut light = LightScratch::new(&mut scratch.levels[..], &mut scratch.queue[..]);
        build_light_core(&access, &mut light).unwrap();
        assert_eq!(light.tail(), LIGHT_VOLUME);

        let mut short = MeshScratch::try_new().unwrap();
        let mut light =
            LightScratch::new(&mut short.levels[..], &mut short.queue[..LIGHT_VOLUME - 1]);
        assert_eq!(
            build_light_core(&access, &mut light),
            Err(MeshError::QueueOverflow),
        );
    }
}
