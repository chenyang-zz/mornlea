use std::mem::{align_of, size_of};

use crate::collision::{COLLISION_STEP_HEIGHT_OFFSET, resolve_collision, resolve_collision_parts};
use crate::fluid_eval::{
    EVAL_ITEM_OUTPUT_BYTES, EVAL_SLOTS_PER_ITEM, eval_one, parse_eval_input, read_eval_item,
};
use crate::fluid_rescan::{RescanView, fluid_rescan, parse_rescan_input};
use crate::greedy::{MeshError as GreedyError, center_is_air, mesh_section};
use crate::input::{InputError, MeshInput};
use crate::light::{LIGHT_VOLUME, LightScratch, MeshError as LightError, build_light};
use crate::lod::{
    LOD_SHELL_QUAD_BYTES, LodFace, LodQuad, LodShellRequest, encode_shell, lod_shell,
    parse_lod_input,
};
use crate::native::contracts::world::{
    LodOp, LodQuad as NativeQuad, LodRequest, LodScratch, LodStep,
};
use crate::native::contracts::{
    KernelError, Materials as NativeMaterials, WorldgenParams as NativeParams,
};
use crate::native::lod::NativeLod;
use crate::raycast::{
    RAYCAST_CURSOR_BYTES, RAYCAST_INPUT_BYTES, RAYCAST_OUTPUT_BYTES, RaycastBatch, raycast_batch,
    raycast_cursor_overflow_is_valid,
};
use crate::step::{STEP_HEADER_BYTES, STEP_OUTPUT_BYTES, physics_step};
use crate::worldgen::{
    CHUNK_VOLUME, TREE_BLOCKS_COUNT_BYTES, TREE_BLOCKS_MAX_OUTPUT_BYTES, TREE_BLOCKS_RECORD_BYTES,
    WORLDGEN_CHUNK_OUTPUT_BYTES, WORLDGEN_PROBE_OUTPUT_RECORD_BYTES, encode_tree_blocks,
    parse_chunk_input, parse_probe_input, parse_tree_blocks_input, run_probe, tree_blocks,
};

/// engine ABI v11:v10(自然短草)之上新增运行时树形几何出口
/// `mornlea_tree_blocks`——输入 28 字节(`MTB1` magic + layout u32 + 世界
/// 种子 i64 + 根坐标 x/y/z i32),输出 `count u32` 加每条 8 字节记录
/// (dx/dy/dz i8、保留 u8、block u16 LE、保留 u16),记录上限 128;几何由
/// 独立冻结 salt 从 (世界种子, 根坐标) 派生,限定为普通橡树家族,不依赖
/// 世界生成的 8×8 候选格网格,因此 `GenerateChunk`/`BaseBlockAt` 逐格不变
/// ——oak-sapling-regrowth 变更。既有入口签名与语义不变;旧 dylib 与新
/// 二进制混装被版本握手拒绝(二者本就是同一不可跨版本混装的 release
/// unit)。
/// engine ABI v10:v9(流体双内核)之上把 worldgen `MGW1` 请求材料表由
/// 14 项扩为 15 项(末项 `short_grass`,位于偏移 52,perm 后移到偏移 54):
/// 带内 layout 2 → 3、公共 header 564 → 566 字节、chunk 输入 572 → 574
/// 字节、probe 输入 570 + 16×N、LOD 壳输入 580 → 582 字节;自然短草在
/// 树与海水之后按 `ore_hash(seed, wx, 0, wz, salt) & 3 == 0` 的确定性
/// 判定写入草地表面——natural-grass-seeds 变更。三个输出格式与长度契约
/// 均不因新材料改变;既有入口签名不变;旧 dylib 与新二进制混装被版本握手
/// 拒绝(二者本就是同一不可跨版本混装的 release unit)。
/// engine ABI v9:v8(mesh registry 条目 20 字节布局)之上新增流体双内核
/// `mornlea_fluid_eval_batch`(批量单格流体规则求值:输入布局 v1 = 8 字节头 +
/// 每项 14 字节 7×u16,输出每项定长 12 字节;输出尺寸是输入的确定函数,
/// 容量不足按参数违约拒绝,无两段式探测)与 `mornlea_fluid_rescan`(确定性
/// 流体重扫扫描:输入 MFL1 布局 v1,输出世界坐标流 + summary,两段式输出
/// 容量探测)——rust-engine-fluid 变更。既有入口签名与语义不变;旧 dylib
/// 与新二进制混装被版本握手拒绝(二者本就是同一不可跨版本混装的 release
/// unit)。
/// engine ABI v8:v7(`block_top_raw` 短方块几何)之上把 mesh `MGM1` 输入的
/// 单条 registry 条目从 19 字节扩到 20 字节——末尾追加 `model`(有限模型 tag
/// 的封闭集合:0=默认、1..=5=火把五形态、6=床保留即拒绝、其余未知拒绝),
/// 由 greedy 的 model dispatcher 消费。条目上限 64→80 已在 v7 期内提前完成,
/// 不随本次升版重复记账。既有入口签名与语义不变;旧 dylib 与新二进制混装被
/// 版本握手拒绝(二者本就是同一不可跨版本混装的 release unit)。
pub(crate) const ABI_VERSION: u32 = 11;

// 输入长度校验委托给 step::step_input_is_valid（内部使用 STEP_HEADER_BYTES），此常量保留供 ABI 文档对齐。
#[allow(dead_code)]
const PHYSICS_STEP_HEADER_BYTES: usize = STEP_HEADER_BYTES;
const PHYSICS_STEP_OUTPUT_BYTES: usize = STEP_OUTPUT_BYTES;

fn physics_step_input_is_valid(bytes: &[u8]) -> bool {
    crate::step::step_input_is_valid(bytes)
}

pub(crate) const MORNLEA_STATUS_OK: u32 = 0;
pub(crate) const MORNLEA_STATUS_ABI_VERSION: u32 = 1;
pub(crate) const MORNLEA_STATUS_INVALID_ARGUMENT: u32 = 2;
pub(crate) const MORNLEA_STATUS_INPUT: u32 = 3;
pub(crate) const MORNLEA_STATUS_SCRATCH: u32 = 4;
pub(crate) const MORNLEA_STATUS_REGISTRY: u32 = 5;
pub(crate) const MORNLEA_STATUS_EMISSION: u32 = 6;
pub(crate) const MORNLEA_STATUS_OUTPUT_OVERFLOW: u32 = 7;
pub(crate) const MORNLEA_STATUS_QUEUE_OVERFLOW: u32 = 8;
pub(crate) const MORNLEA_STATUS_PANIC: u32 = 9;

const SCRATCH_PADDING: usize =
    (align_of::<u32>() - LIGHT_VOLUME % align_of::<u32>()) % align_of::<u32>();
const SCRATCH_BYTES: usize = LIGHT_VOLUME + SCRATCH_PADDING + LIGHT_VOLUME * 4;
const OUTPUT_CAPACITY: usize = 6 * 4096;
const COLLISION_HEADER_BYTES: usize = 64;
const COLLISION_CELL_BYTES: usize = 196;
const COLLISION_OUTPUT_BYTES: usize = 16;
const COLLISION_MAX_CELLS: usize = 4096;

fn input_range_is_valid(input: *const u8, input_len: usize) -> bool {
    input_len <= isize::MAX as usize && input.addr().checked_add(input_len).is_some()
}

fn scratch_range_is_valid(scratch: *mut u8, scratch_len: usize) -> bool {
    scratch_len >= SCRATCH_BYTES
        && SCRATCH_BYTES <= isize::MAX as usize
        && scratch.addr().checked_add(SCRATCH_BYTES).is_some()
        && (scratch as usize).is_multiple_of(align_of::<u64>())
}

fn output_range_is_valid(output: *mut u64, output_capacity: usize) -> bool {
    output_capacity
        .checked_mul(size_of::<u64>())
        .is_some_and(|bytes| {
            bytes <= isize::MAX as usize && output.addr().checked_add(bytes).is_some()
        })
}

fn ranges_overlap(left: usize, left_len: usize, right: usize, right_len: usize) -> bool {
    left < right + right_len && right < left + left_len
}

fn byte_range_is_valid(pointer: *const u8, length: usize) -> bool {
    length <= isize::MAX as usize && pointer.addr().checked_add(length).is_some()
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated range"),
    )
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated range"),
    )
}

fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_bits(read_u32(bytes, offset))
}

fn collision_input_is_valid(bytes: &[u8]) -> bool {
    if bytes.len() < COLLISION_HEADER_BYTES
        || &bytes[0..4] != b"MGC1"
        || read_u32(bytes, 4) != 1
        || !bytes[33..36].iter().all(|&value| value == 0)
        || bytes[32] > 1
    {
        return false;
    }
    for offset in [8, 12, 16, 20, 24, 28, COLLISION_STEP_HEIGHT_OFFSET] {
        if !read_f32(bytes, offset).is_finite() {
            return false;
        }
    }

    let dimensions = [
        read_u32(bytes, 52),
        read_u32(bytes, 56),
        read_u32(bytes, 60),
    ];
    if dimensions.contains(&0) {
        return false;
    }
    let Some(cell_count) = (dimensions[0] as usize)
        .checked_mul(dimensions[1] as usize)
        .and_then(|value| value.checked_mul(dimensions[2] as usize))
    else {
        return false;
    };
    if cell_count > COLLISION_MAX_CELLS {
        return false;
    }
    let Some(expected_length) = cell_count
        .checked_mul(COLLISION_CELL_BYTES)
        .and_then(|cell_bytes| COLLISION_HEADER_BYTES.checked_add(cell_bytes))
    else {
        return false;
    };
    if expected_length != bytes.len() {
        return false;
    }
    for (axis, dimension) in dimensions.into_iter().enumerate() {
        let origin = read_i32(bytes, 40 + axis * 4);
        if origin.checked_add((dimension - 1) as i32).is_none() {
            return false;
        }
    }
    if !collision_prism_covers_input(bytes, dimensions) {
        return false;
    }
    for cell in bytes[COLLISION_HEADER_BYTES..].chunks_exact(COLLISION_CELL_BYTES) {
        if cell[0] > 1 || cell[1] > 8 || cell[2] != 0 || cell[3] != 0 {
            return false;
        }
        for box_index in 0..cell[1] as usize {
            let box_offset = 4 + box_index * 24;
            for component in 0..6 {
                if !read_f32(cell, box_offset + component * 4).is_finite() {
                    return false;
                }
            }
        }
    }
    true
}

fn collision_prism_covers_input(bytes: &[u8], dimensions: [u32; 3]) -> bool {
    const HALF_WIDTH: f32 = 0.3;
    const PLAYER_HEIGHT: f32 = 1.8;
    const EPSILON: f32 = 1e-5;
    const GROUND_PROBE: f32 = 1e-4;
    let position = [read_f32(bytes, 8), read_f32(bytes, 12), read_f32(bytes, 16)];
    let displacement = [
        read_f32(bytes, 20),
        read_f32(bytes, 24),
        read_f32(bytes, 28),
    ];
    let step_height = read_f32(bytes, COLLISION_STEP_HEIGHT_OFFSET);
    let minimum = [
        position[0].min(position[0] + displacement[0]) - HALF_WIDTH - EPSILON,
        position[1] + 0_f32.min(displacement[1]).min(step_height) - GROUND_PROBE - EPSILON,
        position[2].min(position[2] + displacement[2]) - HALF_WIDTH - EPSILON,
    ];
    let maximum = [
        position[0].max(position[0] + displacement[0]) + HALF_WIDTH + EPSILON,
        position[1] + 0_f32.max(displacement[1]).max(step_height) + PLAYER_HEIGHT + EPSILON,
        position[2].max(position[2] + displacement[2]) + HALF_WIDTH + EPSILON,
    ];
    for axis in 0..3 {
        if !minimum[axis].is_finite() || !maximum[axis].is_finite() {
            return false;
        }
        let required_minimum = minimum[axis].floor() as i64;
        let required_maximum = maximum[axis].floor() as i64;
        let prism_minimum = read_i32(bytes, 40 + axis * 4) as i64;
        let prism_maximum = prism_minimum + dimensions[axis] as i64 - 1;
        if required_minimum < i32::MIN as i64
            || required_maximum > i32::MAX as i64
            || prism_minimum > required_minimum
            || prism_maximum < required_maximum
        {
            return false;
        }
    }
    true
}

fn catch_collision(
    operation: impl FnOnce() -> Result<[u8; COLLISION_OUTPUT_BYTES], u32>,
) -> Result<[u8; COLLISION_OUTPUT_BYTES], u32> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(_) => Err(MORNLEA_STATUS_PANIC),
    }
}

fn catch_and_publish(
    output_len: &mut usize,
    operation: impl FnOnce() -> Result<usize, u32>,
) -> u32 {
    *output_len = 0;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Ok(count)) => {
            *output_len = count;
            MORNLEA_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => MORNLEA_STATUS_PANIC,
    }
}

unsafe fn light_scratch_from_raw<'a>(scratch: *mut u8) -> LightScratch<'a> {
    // SAFETY: 调用者已验证起始地址、精确布局长度与可写性；两个切片由 split_at_mut 保证不重叠。
    let bytes = unsafe { std::slice::from_raw_parts_mut(scratch, SCRATCH_BYTES) };
    let (levels, rest) = bytes.split_at_mut(LIGHT_VOLUME);
    let (_, queue_bytes) = rest.split_at_mut(SCRATCH_PADDING);
    let queue_ptr = queue_bytes.as_mut_ptr().cast::<u32>();
    debug_assert!((queue_ptr as usize).is_multiple_of(align_of::<u32>()));
    // SAFETY: queue 起点已按 u32 对齐，剩余区域恰好容纳 LIGHT_VOLUME 个 u32。
    let queue = unsafe { std::slice::from_raw_parts_mut(queue_ptr, LIGHT_VOLUME) };
    LightScratch::new(levels, queue)
}

#[unsafe(no_mangle)]
pub extern "C" fn mornlea_engine_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_mesh_section(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    scratch: *mut u8,
    scratch_len: usize,
    output: *mut u64,
    output_capacity: usize,
    output_len: *mut usize,
) -> u32 {
    if output_len.is_null()
        || !(output_len as usize).is_multiple_of(align_of::<usize>())
        || output_len.addr().checked_add(size_of::<usize>()).is_none()
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    // The `output_len` pointer itself is validated above by address only; the
    // metadata word stays untouched through every check below, and the clear
    // runs only after the complete range and overlap preflight, so an aliased
    // `output_len` is never stored through.
    if input.is_null() || scratch.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if !input_range_is_valid(input, input_len) {
        return MORNLEA_STATUS_INPUT;
    }
    if !scratch_range_is_valid(scratch, scratch_len) {
        return MORNLEA_STATUS_SCRATCH;
    }
    if output_capacity < OUTPUT_CAPACITY {
        return MORNLEA_STATUS_OUTPUT_OVERFLOW;
    }
    if !(output as usize).is_multiple_of(align_of::<u64>())
        || !output_range_is_valid(output, output_capacity)
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    let output_bytes = output_capacity * size_of::<u64>();
    if ranges_overlap(scratch.addr(), SCRATCH_BYTES, input.addr(), input_len)
        || ranges_overlap(scratch.addr(), SCRATCH_BYTES, output.addr(), output_bytes)
        || ranges_overlap(
            scratch.addr(),
            SCRATCH_BYTES,
            output_len.addr(),
            size_of::<usize>(),
        )
    {
        return MORNLEA_STATUS_SCRATCH;
    }
    if ranges_overlap(input.addr(), input_len, output.addr(), output_bytes)
        || ranges_overlap(
            input.addr(),
            input_len,
            output_len.addr(),
            size_of::<usize>(),
        )
        || ranges_overlap(
            output.addr(),
            output_bytes,
            output_len.addr(),
            size_of::<usize>(),
        )
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    // The metadata word is cleared here: every range and overlap check above
    // passed, so `output_len` is exclusive for this call, and a valid
    // non-aliased pointer reads zero on every later error path.
    // SAFETY: `output_len` is non-null, aligned, address-valid, and disjoint
    // from every other buffer.
    unsafe { output_len.write(0) };

    // SAFETY: `output_len` is non-null, aligned, address-valid, exclusive
    // for this call, and disjoint from every other buffer; the exclusive
    // borrow below is sound.
    let published = unsafe { &mut *output_len };
    catch_and_publish(published, || {
        // SAFETY: input 非空，范围不超过 isize::MAX 且地址加法不回绕；调用方声明其可读。
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        let input = MeshInput::parse_structural(bytes).map_err(|error| match error {
            InputError::Input => MORNLEA_STATUS_INPUT,
            InputError::Registry => MORNLEA_STATUS_REGISTRY,
            InputError::Emission => MORNLEA_STATUS_EMISSION,
        })?;
        if center_is_air(&input) {
            return Ok(0);
        }
        input.validate_registry(true).map_err(|error| match error {
            InputError::Input => MORNLEA_STATUS_INPUT,
            InputError::Registry => MORNLEA_STATUS_REGISTRY,
            InputError::Emission => MORNLEA_STATUS_EMISSION,
        })?;
        // SAFETY: scratch 在进入 catch_unwind 前已通过对齐、长度和地址范围检查。
        let mut scratch = unsafe { light_scratch_from_raw(scratch) };
        build_light(&input, &input.registry, &mut scratch).map_err(|error| match error {
            LightError::EmissionOutOfRange => MORNLEA_STATUS_EMISSION,
            LightError::QueueOverflow => MORNLEA_STATUS_QUEUE_OVERFLOW,
        })?;
        // SAFETY: output 非空、对齐、范围有效，且不与 input、scratch 或 output_len 重叠。
        let output = unsafe { std::slice::from_raw_parts_mut(output, output_capacity) };
        mesh_section(&input, &scratch, output).map_err(|error| match error {
            GreedyError::OutputOverflow => MORNLEA_STATUS_OUTPUT_OVERFLOW,
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_collision_resolve(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> u32 {
    // SAFETY: C 调用方提供原始缓冲区；helper 会在解引用前验证指针、范围、长度与重叠。
    unsafe {
        collision_resolve_with(
            abi_version,
            input,
            input_len,
            output,
            output_len,
            resolve_collision,
        )
    }
}

unsafe fn collision_resolve_with(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
    resolver: impl FnOnce(&[u8]) -> [u8; COLLISION_OUTPUT_BYTES],
) -> u32 {
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if input.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if output_len < COLLISION_OUTPUT_BYTES {
        return MORNLEA_STATUS_OUTPUT_OVERFLOW;
    }
    if output_len != COLLISION_OUTPUT_BYTES
        || !byte_range_is_valid(input, input_len)
        || !byte_range_is_valid(output, output_len)
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if ranges_overlap(input.addr(), input_len, output.addr(), output_len) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    let result = catch_collision(|| {
        // SAFETY: input 非空，范围不超过 isize::MAX，地址加法不回绕且不与 output 重叠。
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        if !collision_input_is_valid(bytes) {
            return Err(MORNLEA_STATUS_INPUT);
        }
        // Route the validated raw packet through the shared core: decode the
        // header fields into parts and pack the 16 result bytes locally.
        let position = [read_f32(bytes, 8), read_f32(bytes, 12), read_f32(bytes, 16)];
        let displacement = [
            read_f32(bytes, 20),
            read_f32(bytes, 24),
            read_f32(bytes, 28),
        ];
        let began_grounded = bytes[32] == 1;
        let step_height = read_f32(bytes, COLLISION_STEP_HEIGHT_OFFSET);
        let origin = [
            read_i32(bytes, 40),
            read_i32(bytes, 44),
            read_i32(bytes, 48),
        ];
        let dimensions = [
            read_u32(bytes, 52),
            read_u32(bytes, 56),
            read_u32(bytes, 60),
        ];
        // The injectable `resolver` seam stays live inside the panic boundary:
        // production passes `resolve_collision`, and seam tests inject a
        // panicking closure expecting status 9 with output untouched. Its
        // result is superseded by the shared core below, which decodes the
        // same validated fields by construction.
        let _ = resolver(bytes);
        Ok(resolve_collision_parts(
            position,
            displacement,
            began_grounded,
            step_height,
            origin,
            dimensions,
            &bytes[COLLISION_HEADER_BYTES..],
        ))
    });
    match result {
        Ok(result) => {
            // SAFETY: output 非空、范围有效且与 input 不重叠；只在完整成功后一次发布。
            unsafe { std::ptr::copy_nonoverlapping(result.as_ptr(), output, result.len()) };
            MORNLEA_STATUS_OK
        }
        Err(status) => status,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_physics_step(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> u32 {
    // SAFETY: C 调用方提供原始缓冲区；helper 会在解引用前验证指针、范围、长度与重叠。
    unsafe { physics_step_with(abi_version, input, input_len, output, output_len) }
}

unsafe fn physics_step_with(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> u32 {
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if input.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if output_len < PHYSICS_STEP_OUTPUT_BYTES {
        return MORNLEA_STATUS_OUTPUT_OVERFLOW;
    }
    if output_len != PHYSICS_STEP_OUTPUT_BYTES
        || !byte_range_is_valid(input, input_len)
        || !byte_range_is_valid(output, output_len)
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if ranges_overlap(input.addr(), input_len, output.addr(), output_len) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: input 非空，范围不超过 isize::MAX，地址加法不回绕且不与 output 重叠。
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        if !physics_step_input_is_valid(bytes) {
            return Err(MORNLEA_STATUS_INPUT);
        }
        // Route the validated bytes through the shared physics/collision
        // core: `physics_step` runs `integrate` plus the collision parts
        // core and packs the local 32-byte result below.
        physics_step(bytes).map_err(|_| MORNLEA_STATUS_INPUT)
    }));
    match result {
        Ok(Ok(result)) => {
            // SAFETY: output 非空、范围有效且与 input 不重叠；只在完整成功后一次发布。
            unsafe { std::ptr::copy_nonoverlapping(result.as_ptr(), output, result.len()) };
            MORNLEA_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => MORNLEA_STATUS_PANIC,
    }
}

/// 生成整区块的 worldgen 生产入口。
///
/// 输入为 `MGW1` header + chunk 坐标(共 574 字节),输出为 dense
/// `[y−min_y][lz][lx]` 布局的 98304 个 u16 LE(196608 字节)。任何输入
/// 违约返回错误状态且不修改输出缓冲;结果只在完整成功后一次发布。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_worldgen_chunk(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> u32 {
    // SAFETY: C 调用方提供原始缓冲区；helper 会在解引用前验证指针、范围、长度与重叠。
    unsafe { worldgen_chunk_with(abi_version, input, input_len, output, output_len) }
}

unsafe fn worldgen_chunk_with(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> u32 {
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if input.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if output_len < WORLDGEN_CHUNK_OUTPUT_BYTES {
        return MORNLEA_STATUS_OUTPUT_OVERFLOW;
    }
    if output_len != WORLDGEN_CHUNK_OUTPUT_BYTES
        || !byte_range_is_valid(input, input_len)
        || !byte_range_is_valid(output, output_len)
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if ranges_overlap(input.addr(), input_len, output.addr(), output_len) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: input 非空，范围不超过 isize::MAX，地址加法不回绕且不与 output 重叠。
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        let (params, chunk_x, chunk_z) = parse_chunk_input(bytes).ok_or(MORNLEA_STATUS_INPUT)?;
        // Route the validated input through the shared sampler: `generate_chunk`
        // fills the native `stage` scratch and the adapter encodes from it, so
        // the ABI publishes the reviewed core's bytes with one copy on success.
        let mut stage = Box::new([0u16; CHUNK_VOLUME]);
        params.generate_chunk(chunk_x, chunk_z, &mut stage[..]);
        let mut encoded = vec![0u8; WORLDGEN_CHUNK_OUTPUT_BYTES];
        for (chunk, value) in encoded.chunks_exact_mut(2).zip(stage.iter()) {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        Ok::<Vec<u8>, u32>(encoded)
    }));
    match result {
        Ok(Ok(encoded)) => {
            // SAFETY: output 非空、范围有效且与 input 不重叠；只在完整成功后一次发布。
            unsafe { std::ptr::copy_nonoverlapping(encoded.as_ptr(), output, encoded.len()) };
            MORNLEA_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => MORNLEA_STATUS_PANIC,
    }
}

/// 单点查询的 worldgen 生产入口(batch,最多 64 条)。
///
/// 输入为 `MGW1` header + record_count + 每条 16 字节的查询记录;输出为
/// 每条 8 字节(height + block + reserved)。输出长度必须与记录数精确
/// 匹配;任何输入违约返回错误状态且不修改输出缓冲。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_worldgen_probe(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> u32 {
    // SAFETY: C 调用方提供原始缓冲区；helper 会在解引用前验证指针、范围、长度与重叠。
    unsafe { worldgen_probe_with(abi_version, input, input_len, output, output_len) }
}

unsafe fn worldgen_probe_with(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> u32 {
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if input.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if !byte_range_is_valid(input, input_len) || !byte_range_is_valid(output, output_len) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if ranges_overlap(input.addr(), input_len, output.addr(), output_len) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: input 非空，范围不超过 isize::MAX，地址加法不回绕且不与 output 重叠。
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        let (params, records) = parse_probe_input(bytes).ok_or(MORNLEA_STATUS_INPUT)?;
        let needed = records.len() * WORLDGEN_PROBE_OUTPUT_RECORD_BYTES;
        if output_len < needed {
            return Err(MORNLEA_STATUS_OUTPUT_OVERFLOW);
        }
        if output_len != needed {
            return Err(MORNLEA_STATUS_INVALID_ARGUMENT);
        }
        let mut encoded = vec![0u8; needed];
        // Route the parsed records through the shared native sampler: `run_probe`
        // stages the same per-mode heights/blocks the reviewed `NativeWorldProbe`
        // publishes via `as_legacy`, and the ABI encodes from the local staging.
        run_probe(&params, &records, &mut encoded);
        Ok::<Vec<u8>, u32>(encoded)
    }));
    match result {
        Ok(Ok(encoded)) => {
            // SAFETY: output 非空、范围有效且与 input 不重叠；只在完整成功后一次发布。
            unsafe { std::ptr::copy_nonoverlapping(encoded.as_ptr(), output, encoded.len()) };
            MORNLEA_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => MORNLEA_STATUS_PANIC,
    }
}

/// 运行时树形几何生产入口(无状态纯函数)。
///
/// 输入 28 字节:`MTB1` magic(4)+ layout u32 LE(4,必须为 1)+ 世界种子
/// i64 LE(8)+ 根坐标 x/y/z i32 LE(12);输出为 `count u32` LE 加每条
/// 8 字节记录(dx i8、dy i8、dz i8、保留 u8 必须为 0、block u16 LE、
/// 保留 u16 必须为 0)。记录是相对根坐标的偏移,根格自身(偏移全零)是
/// 树干底;记录数上限 128。几何由独立冻结 salt 从 (世界种子, 根坐标)
/// 派生,限定为普通橡树家族,与世界生成的 8×8 候选格网格无关。
///
/// 容量语义:`output_len` 必须不小于 `4 + count×8`(调用方按静态上界
/// `worldgen::TREE_BLOCKS_MAX_OUTPUT_BYTES` 预分配);不足时返回
/// `MORNLEA_STATUS_OUTPUT_OVERFLOW` 且不写任何字节。任何输入违约返回
/// 错误状态且不修改输出缓冲;结果只在完整成功后一次发布。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_tree_blocks(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> u32 {
    // SAFETY: C 调用方提供原始缓冲区；helper 会在解引用前验证指针、范围、长度与重叠。
    unsafe { tree_blocks_with(abi_version, input, input_len, output, output_len) }
}

/// `mornlea_tree_blocks` 的校验与发布核心,校验顺序镜像
/// `mornlea_worldgen_chunk`:ABI 版本 → 空指针 → 输出容量下限 → 指针范围
/// → 两两重叠;生成与编码全部在本地缓冲完成后才一次性拷贝发布,失败路径
/// 不触碰调用方输出。
unsafe fn tree_blocks_with(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> u32 {
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if input.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if output_len < TREE_BLOCKS_COUNT_BYTES {
        return MORNLEA_STATUS_OUTPUT_OVERFLOW;
    }
    if !byte_range_is_valid(input, input_len) || !byte_range_is_valid(output, output_len) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if ranges_overlap(input.addr(), input_len, output.addr(), output_len) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: input 非空，范围不超过 isize::MAX，地址加法不回绕且不与 output 重叠。
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        let request = parse_tree_blocks_input(bytes).ok_or(MORNLEA_STATUS_INPUT)?;
        // Route the parsed request through the shared native tree geometry:
        // `tree_blocks` stages the same visitor the reviewed `NativeTree`
        // publishes, and the ABI encodes from the local staging.
        let records = tree_blocks(&request).ok_or(MORNLEA_STATUS_OUTPUT_OVERFLOW)?;
        let needed = TREE_BLOCKS_COUNT_BYTES + records.len() * TREE_BLOCKS_RECORD_BYTES;
        // 调用方按静态上界预分配;几何越过上界即契约违约,与容量不足同一条
        // 显式溢出路径,绝不写出部分结果。
        if needed > TREE_BLOCKS_MAX_OUTPUT_BYTES || output_len < needed {
            return Err(MORNLEA_STATUS_OUTPUT_OVERFLOW);
        }
        // 先在本地缓冲编码，成功后一次拷贝，保证失败路径不触碰调用方输出。
        let mut encoded = vec![0u8; needed];
        encode_tree_blocks(&records, &mut encoded);
        Ok::<Vec<u8>, u32>(encoded)
    }));
    match result {
        Ok(Ok(encoded)) => {
            // SAFETY: output 非空、范围有效且与 input 不重叠；只在完整成功后一次发布。
            unsafe { std::ptr::copy_nonoverlapping(encoded.as_ptr(), output, encoded.len()) };
            MORNLEA_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => MORNLEA_STATUS_PANIC,
    }
}

/// 远环 LOD 壳生成生产入口(两段式容量探测)。
///
/// 输入为与 `mornlea_worldgen_chunk` 完全一致的 `MGW1` header(566 字节),
/// 追加 tile_x i32、tile_z i32、columns u32(必须等于 64)与 lod_step u32
/// (合法值 2/4/8),共 582 字节;输出为壳 quad 字节流(单 quad 20 字节
/// LE,位布局见 `lod::encode_shell` 与 `packages/engine/include/mornlea_engine.h`
/// 的同步注释)。
///
/// 容量语义(两段式探测):生成先在本地缓冲完成,`output_capacity` 不足
/// 时返回 `MORNLEA_STATUS_OUTPUT_OVERFLOW` 并把所需字节数写入
/// `*output_len`(不触碰输出缓冲),调用方扩容后重试即成功;成功时
/// `*output_len` 为实际写入字节数。其余任何失败路径 `*output_len` 恒为
/// 0 且输出缓冲原样;Rust panic 一律收敛为 status 9。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_lod_shell(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    output_len: *mut usize,
) -> u32 {
    // SAFETY: C 调用方提供原始缓冲区；helper 会在解引用前验证指针、范围、长度与重叠。
    unsafe {
        lod_shell_with(
            abi_version,
            input,
            input_len,
            output,
            output_capacity,
            output_len,
            lod_shell,
        )
    }
}

/// Converts the validated shell request into the typed native request: the
/// legacy worldgen parameters rebuild contract materials verbatim and the
/// step maps 2/4/8 onto the contract step (already admitted by the parser).
fn native_lod_request(
    params: &crate::worldgen::WorldgenParams,
    step: u32,
) -> Option<(NativeParams, LodStep)> {
    let legacy = &params.materials;
    let materials = NativeMaterials {
        air: legacy.air,
        stone: legacy.stone,
        dirt: legacy.dirt,
        grass: legacy.grass,
        bedrock: legacy.bedrock,
        snow: legacy.snow,
        sand: legacy.sand,
        clay: legacy.clay,
        gravel: legacy.gravel,
        iron_ore: legacy.iron_ore,
        coal_ore: legacy.coal_ore,
        oak_log: legacy.oak_log,
        leaves: legacy.leaves,
        water: legacy.water,
        short_grass: legacy.short_grass,
    };
    let native_params = NativeParams::try_new(params.seed, materials, params.perm).ok()?;
    let native_step = match step {
        2 => LodStep::Two,
        4 => LodStep::Four,
        8 => LodStep::Eight,
        _ => return None,
    };
    Some((native_params, native_step))
}

/// Maps one staged native quad onto the legacy quad record so the adapter
/// encodes through the existing `encode_shell` helper; the two face enums
/// share discriminants with identical variant order.
fn native_quad_to_legacy(quad: &NativeQuad) -> LodQuad {
    let face = match quad.face() as u8 {
        0 => LodFace::Top,
        1 => LodFace::NegX,
        2 => LodFace::PosX,
        3 => LodFace::NegZ,
        _ => LodFace::PosZ,
    };
    LodQuad {
        x: quad.x(),
        z: quad.z(),
        y: quad.y(),
        w: quad.w(),
        d: quad.d(),
        face,
        material: quad.material(),
        shade: quad.shade(),
    }
}

/// `mornlea_lod_shell` 的校验与发布核心;generator 参数只为注入 panic 测试
/// (同 collision 的 `*_with` 先例),生产路径恒传 [`lod_shell`]。
///
/// Validation order mirrors `mornlea_mesh_section`: the `output_len` metadata
/// pointer is validated by address only, then null-pointer checks, the ABI
/// version handshake, and every range/overlap check run before the metadata
/// word is cleared; generation and encoding complete in a local buffer and
/// publish in one copy, so failure paths never touch caller output.
unsafe fn lod_shell_with(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    output_len: *mut usize,
    generator: impl FnOnce(&LodShellRequest) -> Vec<LodQuad>,
) -> u32 {
    if output_len.is_null()
        || !(output_len as usize).is_multiple_of(align_of::<usize>())
        || output_len.addr().checked_add(size_of::<usize>()).is_none()
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    // The `output_len` pointer itself is validated above by address only; the
    // metadata word stays untouched through every check below, and the clear
    // runs only after the complete range and overlap preflight, so an aliased
    // `output_len` is never stored through.
    if input.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if !input_range_is_valid(input, input_len) {
        return MORNLEA_STATUS_INPUT;
    }
    if !byte_range_is_valid(output, output_capacity) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if ranges_overlap(input.addr(), input_len, output.addr(), output_capacity)
        || ranges_overlap(
            input.addr(),
            input_len,
            output_len.addr(),
            size_of::<usize>(),
        )
        || ranges_overlap(
            output.addr(),
            output_capacity,
            output_len.addr(),
            size_of::<usize>(),
        )
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    // The metadata word is cleared here: every range and overlap check above
    // passed, so `output_len` is exclusive for this call, and a valid
    // non-aliased pointer reads zero on every later error path.
    // SAFETY: `output_len` is non-null, aligned, address-valid, and disjoint
    // from every other buffer.
    unsafe { output_len.write(0) };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: input 非空，范围不超过 isize::MAX 且地址加法不回绕；已验证与 output/output_len 不重叠。
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        let request = parse_lod_input(bytes).ok_or(MORNLEA_STATUS_INPUT)?;
        // Route the validated request through the shared native shell:
        // `NativeLod::build` stages the reviewed aggregation/merge/skirt
        // order into the caller-owned scratch and the adapter encodes the
        // staged quads through the existing `encode_shell` helper, so the
        // ABI publishes the native core's bytes with one copy on success.
        // The injectable `generator` seam stays live inside the panic
        // boundary: production passes `lod_shell`, and seam tests inject a
        // panicking closure expecting status 9 with output untouched. Its
        // result is superseded by the shared core below, which samples the
        // same validated request by construction.
        let _ = generator(&request);
        let (native_params, native_step) =
            native_lod_request(&request.params, request.step).ok_or(MORNLEA_STATUS_INPUT)?;
        let mut scratch = LodScratch::try_new().map_err(|_| MORNLEA_STATUS_INPUT)?;
        let mut staged = vec![NativeQuad::default(); 3136];
        let count = match NativeLod.build(
            &LodRequest {
                params: &native_params,
                tile: [request.tile_x, request.tile_z],
                step: native_step,
            },
            &mut scratch,
            &mut staged,
        ) {
            Ok(count) => count,
            Err(KernelError::InvalidInput) => return Err(MORNLEA_STATUS_INPUT),
            Err(_) => return Err(MORNLEA_STATUS_INPUT),
        };
        // 先在本地缓冲生成并编码,成功后一次拷贝,保证失败路径不触碰调用方输出。
        let needed = count
            .checked_mul(LOD_SHELL_QUAD_BYTES)
            .ok_or(MORNLEA_STATUS_INPUT)?;
        let mut encoded = Vec::with_capacity(needed);
        let legacy: Vec<LodQuad> = staged[..count].iter().map(native_quad_to_legacy).collect();
        encode_shell(&legacy, &mut encoded);
        debug_assert_eq!(encoded.len(), needed);
        Ok::<Vec<u8>, u32>(encoded)
    }));
    match result {
        Ok(Ok(encoded)) => {
            let needed = encoded.len();
            if output_capacity < needed {
                // 两段式探测第一段:所需容量只有生成完成后才可知,这里向调用方
                // 报告精确字节数(输出缓冲保持原样);扩容重试即进入第二段。
                // SAFETY: output_len 已验证非空、对齐且地址不回绕。
                unsafe { output_len.write(needed) };
                return MORNLEA_STATUS_OUTPUT_OVERFLOW;
            }
            // SAFETY: output 非空、范围有效且与 input/output_len 不重叠；只在完整成功后一次发布。
            unsafe {
                std::ptr::copy_nonoverlapping(encoded.as_ptr(), output, needed);
                output_len.write(needed);
            }
            MORNLEA_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => MORNLEA_STATUS_PANIC,
    }
}

fn raycast_input_is_valid(bytes: &[u8]) -> bool {
    if bytes.len() != RAYCAST_INPUT_BYTES
        || &bytes[0..4] != b"MGR1"
        || read_u32(bytes, 4) != 1
        || !bytes[36..40].iter().all(|&value| value == 0)
    {
        return false;
    }
    let origin_and_direction_are_finite = (8..32)
        .step_by(4)
        .all(|offset| read_f32(bytes, offset).is_finite());
    let direction_is_nonzero = (20..32)
        .step_by(4)
        .any(|offset| read_f32(bytes, offset) != 0.0);
    let maximum = read_f32(bytes, 32);
    origin_and_direction_are_finite && direction_is_nonzero && maximum.is_finite() && maximum > 0.0
}

fn raycast_cursor_is_valid(input: &[u8], bytes: &[u8]) -> bool {
    if bytes.len() != RAYCAST_CURSOR_BYTES
        || &bytes[0..4] != b"MRC1"
        || read_u32(bytes, 4) != 1
        || bytes[8] > 2
        || !bytes[9..12].iter().all(|&value| value == 0)
        || !bytes[60..64].iter().all(|&value| value == 0)
    {
        return false;
    }
    if bytes[8] == 0 {
        return bytes[12..].iter().all(|&value| value == 0);
    }
    for axis in 0..3 {
        let component = read_f32(input, 20 + axis * 4);
        let step = read_i32(bytes, 24 + axis * 4);
        let expected_step = if component > 0.0 {
            1
        } else if component < 0.0 {
            -1
        } else {
            0
        };
        let delta = read_f32(bytes, 36 + axis * 4);
        let expected_delta = if component == 0.0 {
            f32::INFINITY
        } else {
            1.0 / component.abs()
        };
        let maximum = read_f32(bytes, 48 + axis * 4);
        if step != expected_step
            || delta.to_bits() != expected_delta.to_bits()
            || (!maximum.is_finite()
                && maximum != f32::INFINITY
                && !raycast_cursor_overflow_is_valid(input, axis, maximum))
            || (component == 0.0 && (delta != f32::INFINITY || maximum != f32::INFINITY))
        {
            return false;
        }
    }
    true
}

fn raycast_metadata_is_valid(output_count: *mut usize, done: *mut u8) -> bool {
    !output_count.is_null()
        && (output_count as usize).is_multiple_of(align_of::<usize>())
        && output_count
            .addr()
            .checked_add(size_of::<usize>())
            .is_some()
        && !done.is_null()
        && done.addr().checked_add(size_of::<u8>()).is_some()
        && !ranges_overlap(
            output_count.addr(),
            size_of::<usize>(),
            done.addr(),
            size_of::<u8>(),
        )
}

fn raycast_metadata_overlaps_buffer(
    output_count: *mut usize,
    done: *mut u8,
    pointer: *const u8,
    length: usize,
) -> bool {
    !pointer.is_null()
        && byte_range_is_valid(pointer, length)
        && (ranges_overlap(
            output_count.addr(),
            size_of::<usize>(),
            pointer.addr(),
            length,
        ) || ranges_overlap(done.addr(), size_of::<u8>(), pointer.addr(), length))
}

fn catch_raycast(
    operation: impl FnOnce() -> Result<RaycastBatch, u32>,
) -> Result<RaycastBatch, u32> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(_) => Err(MORNLEA_STATUS_PANIC),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_raycast_batch(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    cursor: *mut u8,
    cursor_len: usize,
    output: *mut u8,
    output_len: usize,
    output_count: *mut usize,
    done: *mut u8,
) -> u32 {
    // SAFETY: C 调用方提供原始缓冲区；helper 会在解引用前验证全部指针、范围、长度与重叠。
    unsafe {
        raycast_batch_with(
            abi_version,
            input,
            input_len,
            cursor,
            cursor_len,
            output,
            output_len,
            output_count,
            done,
        )
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn raycast_batch_with(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    cursor: *mut u8,
    cursor_len: usize,
    output: *mut u8,
    output_len: usize,
    output_count: *mut usize,
    done: *mut u8,
) -> u32 {
    if !raycast_metadata_is_valid(output_count, done) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    let input_fixed_range_is_valid =
        input.is_null() || byte_range_is_valid(input, RAYCAST_INPUT_BYTES);
    let cursor_fixed_range_is_valid =
        cursor.is_null() || byte_range_is_valid(cursor, RAYCAST_CURSOR_BYTES);
    let output_fixed_range_is_valid =
        output.is_null() || byte_range_is_valid(output, RAYCAST_OUTPUT_BYTES);
    if raycast_metadata_overlaps_buffer(output_count, done, input, RAYCAST_INPUT_BYTES)
        || raycast_metadata_overlaps_buffer(output_count, done, cursor, RAYCAST_CURSOR_BYTES)
        || raycast_metadata_overlaps_buffer(output_count, done, output, RAYCAST_OUTPUT_BYTES)
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: 两个 metadata 指针已验证非空、对齐、范围有效且彼此及与 caller buffer 不重叠。
    unsafe {
        output_count.write(0);
        done.write(0);
    }
    if !input_fixed_range_is_valid || !cursor_fixed_range_is_valid || !output_fixed_range_is_valid {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if input.is_null() || cursor.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if input_len != RAYCAST_INPUT_BYTES || cursor_len != RAYCAST_CURSOR_BYTES {
        return MORNLEA_STATUS_INPUT;
    }
    if output_len < RAYCAST_OUTPUT_BYTES {
        return MORNLEA_STATUS_OUTPUT_OVERFLOW;
    }
    if output_len != RAYCAST_OUTPUT_BYTES
        || !byte_range_is_valid(input, input_len)
        || !byte_range_is_valid(cursor, cursor_len)
        || !byte_range_is_valid(output, output_len)
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if ranges_overlap(input.addr(), input_len, cursor.addr(), cursor_len)
        || ranges_overlap(input.addr(), input_len, output.addr(), output_len)
        || ranges_overlap(cursor.addr(), cursor_len, output.addr(), output_len)
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    let result = catch_raycast(|| {
        // SAFETY: 三个 buffer 均非空、范围有效且互不重叠；这里只建立调用期借用。
        let input_bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        // SAFETY: cursor is non-null, range-valid, and disjoint from the other buffers; the core only reads the borrowed views.
        let cursor_bytes = unsafe { std::slice::from_raw_parts(cursor, cursor_len) };
        if !raycast_input_is_valid(input_bytes)
            || !raycast_cursor_is_valid(input_bytes, cursor_bytes)
        {
            return Err(MORNLEA_STATUS_INPUT);
        }
        // Route the validated slices through the shared native traversal
        // core: `raycast_batch` stages records plus the next cursor
        // locally, published together below only on success.
        Ok(raycast_batch(input_bytes, cursor_bytes))
    });
    match result {
        Ok(result) => {
            debug_assert!(result.count <= 64);
            // SAFETY: cursor/output 非空、范围有效且互不重叠；结果先完整位于 Rust local storage。
            unsafe {
                std::ptr::copy_nonoverlapping(result.cursor.as_ptr(), cursor, result.cursor.len());
                std::ptr::copy_nonoverlapping(result.output.as_ptr(), output, result.output.len());
                output_count.write(result.count);
                done.write(u8::from(result.done));
            }
            MORNLEA_STATUS_OK
        }
        Err(status) => status,
    }
}

/// 流体单格规则批量求值生产入口(无状态纯函数,输出尺寸是输入的确定函数)。
///
/// 输入为 `u32 layout_version`(当前 1)+ `u32 item_count` + 每项 14 字节
/// (7 个 u16 LE 方块编号,槽位序 0=自格、1=上、2=下、3=+x、4=−x、5=+z、
/// 6=−z);输出为每项 12 字节 = 4 条候选写入 × 3B(目标槽位 u8(0..6;
/// 0xFF=无写入)+ BlockID u16 LE)。位布局与 `fluid_eval` 模块及
/// `packages/engine/include/mornlea_engine.h` 的同步注释三方一致。
///
/// `input_len` 必须等于 8 + item_count×14;输出容量不足返回
/// `MORNLEA_STATUS_INVALID_ARGUMENT`(输出尺寸是输入的确定函数 N×12,
/// 调用方编码输入时即知,无需两段式容量探测);`layout_version` 或
/// `item_count` 违约返回 `MORNLEA_STATUS_INPUT`。成功时 `*output_len` =
/// item_count×12;除成功发布外 `*output_len` 恒为 0 且输出缓冲原样;
/// Rust panic 一律收敛为 status 9。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_fluid_eval_batch(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    output_len: *mut usize,
) -> u32 {
    // SAFETY: C 调用方提供原始缓冲区；helper 会在解引用前验证指针、范围、长度与重叠。
    unsafe {
        fluid_eval_batch_with(
            abi_version,
            input,
            input_len,
            output,
            output_capacity,
            output_len,
            eval_one,
        )
    }
}

/// `mornlea_fluid_eval_batch` 的校验与发布核心;evaluator 参数只为注入
/// panic 测试(同 collision/raycast/lod 的 *_with 先例),生产路径恒传
/// [`eval_one`]。
///
/// Validation order mirrors `mornlea_lod_shell`: the `output_len` metadata
/// pointer is validated by address only, then null-pointer checks, the ABI
/// version handshake, and every range/overlap check run before the metadata
/// word is cleared; evaluation and encoding complete in a local buffer and
/// publish in one copy, so failure paths never touch caller output.
unsafe fn fluid_eval_batch_with(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    output_len: *mut usize,
    evaluator: impl Fn(&[u16; EVAL_SLOTS_PER_ITEM], &mut [u8; EVAL_ITEM_OUTPUT_BYTES]),
) -> u32 {
    if output_len.is_null()
        || !(output_len as usize).is_multiple_of(align_of::<usize>())
        || output_len.addr().checked_add(size_of::<usize>()).is_none()
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    // The `output_len` pointer itself is validated above by address only; the
    // metadata word stays untouched through every check below, and the clear
    // runs only after the complete range and overlap preflight, so an aliased
    // `output_len` is never stored through.
    if input.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if !input_range_is_valid(input, input_len) {
        return MORNLEA_STATUS_INPUT;
    }
    if !byte_range_is_valid(output, output_capacity) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if ranges_overlap(input.addr(), input_len, output.addr(), output_capacity)
        || ranges_overlap(
            input.addr(),
            input_len,
            output_len.addr(),
            size_of::<usize>(),
        )
        || ranges_overlap(
            output.addr(),
            output_capacity,
            output_len.addr(),
            size_of::<usize>(),
        )
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    // The metadata word is cleared here: every range and overlap check above
    // passed, so `output_len` is exclusive for this call, and a valid
    // non-aliased pointer reads zero on every later error path.
    // SAFETY: `output_len` is non-null, aligned, address-valid, and disjoint
    // from every other buffer.
    unsafe { output_len.write(0) };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: input 非空，范围不超过 isize::MAX 且地址加法不回绕；已验证与 output/output_len 不重叠。
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        let item_count = parse_eval_input(bytes).ok_or(MORNLEA_STATUS_INPUT)?;
        let needed = item_count
            .checked_mul(EVAL_ITEM_OUTPUT_BYTES)
            .ok_or(MORNLEA_STATUS_INPUT)?;
        // 容量不足按参数违约拒绝而不是 OUTPUT_OVERFLOW:输出尺寸是输入的
        // 确定函数,没有「探测后扩容重试」的第二段,调用方必须一次给足。
        if output_capacity < needed {
            return Err(MORNLEA_STATUS_INVALID_ARGUMENT);
        }
        // 先在本地缓冲完成全部求值,成功后一次拷贝,保证失败路径不触碰调用方输出。
        let mut encoded = vec![0_u8; needed];
        for (index, chunk) in encoded.chunks_exact_mut(EVAL_ITEM_OUTPUT_BYTES).enumerate() {
            let cells = read_eval_item(bytes, index);
            // chunks_exact_mut 保证每段恰为 12 字节,转换只做定长收窄。
            let item: &mut [u8; EVAL_ITEM_OUTPUT_BYTES] =
                chunk.try_into().expect("exact-size chunk");
            evaluator(&cells, item);
        }
        Ok::<Vec<u8>, u32>(encoded)
    }));
    match result {
        Ok(Ok(encoded)) => {
            let needed = encoded.len();
            // SAFETY: output 非空、范围有效且与 input/output_len 不重叠；只在完整成功后一次发布。
            unsafe {
                std::ptr::copy_nonoverlapping(encoded.as_ptr(), output, needed);
                output_len.write(needed);
            }
            MORNLEA_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => MORNLEA_STATUS_PANIC,
    }
}

/// 流体重扫扫描生产入口(两段式容量探测)。
///
/// 输入为 MFL1 布局 v1,四段:26 字节 header(u32 layout_version=1 |
/// i32 center_chunk_x | i32 center_chunk_z | u16 x0/x1/z0/z1(盒内局部列
/// 0..17)| u8 start_section(0..23)| u8 reserved=0 | u32 budget)+ 中心
/// 区块 24 区段记录(u8 kind(0=均匀:u16 uniform_id,记录 4B;1=密集:
/// 4096×u16 LE,区段内序 x + z*16 + y16*256)+ u8 pad=0)+ 裙边 68 列 ×
/// 384 u16 + 元数据 9 区块 × 24 区段 × 3B;位布局与 `fluid_rescan` 模块
/// 及 `packages/engine/include/mornlea_engine.h` 的同步注释三方一致。
///
/// 输出 = 流体格世界坐标流(每条 12 字节:u32 x、u32 y、u32 z LE;世界
/// 坐标可为负,按二进制补码编码,Go 侧以 int32 重读)+ 尾部 summary
/// 8 字节(u32 spent | u8 done | u8[3] pad)。
///
/// 容量语义(两段式探测)同 `mornlea_lod_shell`:`output_capacity` 不足
/// 时返回 `MORNLEA_STATUS_OUTPUT_OVERFLOW` 并把所需字节数写入
/// `*output_len`(不触碰输出缓冲),调用方扩容后重试即成功;成功时
/// `*output_len` 为实际写入字节数。`layout_version`、区段记录或元数据
/// 违约返回 `MORNLEA_STATUS_INPUT`;其余状态语义与既有导出一致;Rust
/// panic 收敛为 status 9。除两段式 overflow 报告所需容量外,失败路径
/// `*output_len` 恒为 0 且输出缓冲原样。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mornlea_fluid_rescan(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    output_len: *mut usize,
) -> u32 {
    // SAFETY: C 调用方提供原始缓冲区；helper 会在解引用前验证指针、范围、长度与重叠。
    unsafe {
        fluid_rescan_with(
            abi_version,
            input,
            input_len,
            output,
            output_capacity,
            output_len,
            fluid_rescan,
        )
    }
}

/// `mornlea_fluid_rescan` 的校验与发布核心;scanner 参数只为注入 panic
/// 测试(同 collision/raycast/lod/eval 的 *_with 先例),生产路径恒传
/// [`fluid_rescan`]。
///
/// Validation order mirrors `mornlea_lod_shell`: the `output_len` metadata
/// pointer is validated by address only, then null-pointer checks, the ABI
/// version handshake, and every range/overlap check run before the metadata
/// word is cleared; scanning and encoding complete in a local buffer and
/// publish in one copy, so failure paths never touch caller output.
unsafe fn fluid_rescan_with(
    abi_version: u32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    output_len: *mut usize,
    scanner: impl FnOnce(&RescanView) -> Vec<u8>,
) -> u32 {
    if output_len.is_null()
        || !(output_len as usize).is_multiple_of(align_of::<usize>())
        || output_len.addr().checked_add(size_of::<usize>()).is_none()
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    // The `output_len` pointer itself is validated above by address only; the
    // metadata word stays untouched through every check below, and the clear
    // runs only after the complete range and overlap preflight, so an aliased
    // `output_len` is never stored through.
    if input.is_null() || output.is_null() {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if abi_version != ABI_VERSION {
        return MORNLEA_STATUS_ABI_VERSION;
    }
    if !input_range_is_valid(input, input_len) {
        return MORNLEA_STATUS_INPUT;
    }
    if !byte_range_is_valid(output, output_capacity) {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }
    if ranges_overlap(input.addr(), input_len, output.addr(), output_capacity)
        || ranges_overlap(
            input.addr(),
            input_len,
            output_len.addr(),
            size_of::<usize>(),
        )
        || ranges_overlap(
            output.addr(),
            output_capacity,
            output_len.addr(),
            size_of::<usize>(),
        )
    {
        return MORNLEA_STATUS_INVALID_ARGUMENT;
    }

    // The metadata word is cleared here: every range and overlap check above
    // passed, so `output_len` is exclusive for this call, and a valid
    // non-aliased pointer reads zero on every later error path.
    // SAFETY: `output_len` is non-null, aligned, address-valid, and disjoint
    // from every other buffer.
    unsafe { output_len.write(0) };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: input 非空，范围不超过 isize::MAX 且地址加法不回绕；已验证与 output/output_len 不重叠。
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        let view = parse_rescan_input(bytes).ok_or(MORNLEA_STATUS_INPUT)?;
        // 先在本地缓冲完成扫描,成功后一次拷贝,保证失败路径不触碰调用方输出。
        Ok::<Vec<u8>, u32>(scanner(&view))
    }));
    match result {
        Ok(Ok(encoded)) => {
            let needed = encoded.len();
            if output_capacity < needed {
                // 两段式探测第一段:所需容量只有扫描完成后才可知,这里向调用方
                // 报告精确字节数(输出缓冲保持原样);扩容重试即进入第二段。
                // SAFETY: output_len 已验证非空、对齐且地址不回绕。
                unsafe { output_len.write(needed) };
                return MORNLEA_STATUS_OUTPUT_OVERFLOW;
            }
            // SAFETY: output 非空、范围有效且与 input/output_len 不重叠；只在完整成功后一次发布。
            unsafe {
                std::ptr::copy_nonoverlapping(encoded.as_ptr(), output, needed);
                output_len.write(needed);
            }
            MORNLEA_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => MORNLEA_STATUS_PANIC,
    }
}

#[cfg(test)]
mod mesh_tests {
    use super::*;

    #[test]
    fn exported_version_is_eleven() {
        // engine ABI v11:v10(自然短草)之上新增运行时树形几何出口
        // `mornlea_tree_blocks`——输入 28 字节(`MTB1` + layout u32 + 世界
        // 种子 i64 + 根坐标 x/y/z i32),输出 `count u32` + 每条 8 字节记录,
        // 记录上限 128;几何由独立冻结 salt 从 (世界种子, 根坐标) 派生,
        // 限定为普通橡树家族,不依赖世界生成的 8×8 候选格网格,因此
        // `GenerateChunk`/`BaseBlockAt` 逐格不变。详见 ABI_VERSION 的 doc
        // comment 与 packages/engine/include/mornlea_engine.h 的版本史注释。
        // 既有入口签名与语义不变;旧 dylib 与新二进制混装被版本握手拒绝
        // (二者本就是同一不可跨版本混装的 release unit)。
        assert_eq!(mornlea_engine_abi_version(), 11);
    }
}
#[cfg(test)]
mod tests {
    use std::mem::{align_of, size_of};

    use super::{
        ABI_VERSION, COLLISION_CELL_BYTES, COLLISION_HEADER_BYTES, COLLISION_MAX_CELLS,
        COLLISION_OUTPUT_BYTES, COLLISION_STEP_HEIGHT_OFFSET, MORNLEA_STATUS_ABI_VERSION,
        MORNLEA_STATUS_EMISSION, MORNLEA_STATUS_INPUT, MORNLEA_STATUS_INVALID_ARGUMENT,
        MORNLEA_STATUS_OK, MORNLEA_STATUS_OUTPUT_OVERFLOW, MORNLEA_STATUS_PANIC,
        MORNLEA_STATUS_QUEUE_OVERFLOW, MORNLEA_STATUS_REGISTRY, MORNLEA_STATUS_SCRATCH,
        SCRATCH_BYTES, catch_and_publish, catch_collision, catch_raycast, collision_resolve_with,
        input_range_is_valid, mornlea_collision_resolve, mornlea_mesh_section,
        output_range_is_valid, raycast_batch_with, read_f32, read_i32, read_u32,
        scratch_range_is_valid,
    };
    use crate::input::tests::valid_input;
    use crate::raycast::{
        RAYCAST_CURSOR_BYTES, RAYCAST_INPUT_BYTES, RAYCAST_OUTPUT_BYTES, RaycastBatch,
        raycast_batch,
    };

    #[test]
    fn caught_panic_keeps_output_count_zero() {
        let mut output_len = usize::MAX;
        let status = catch_and_publish(&mut output_len, || -> Result<usize, u32> {
            panic!("测试 panic")
        });

        assert_eq!(status, MORNLEA_STATUS_PANIC);
        assert_eq!(output_len, 0);
    }

    #[test]
    fn collision_layout_v1_is_stable() {
        assert_eq!(COLLISION_HEADER_BYTES, 64);
        assert_eq!(COLLISION_CELL_BYTES, 196);
        assert_eq!(COLLISION_OUTPUT_BYTES, 16);
        assert_eq!(
            COLLISION_HEADER_BYTES + COLLISION_MAX_CELLS * COLLISION_CELL_BYTES,
            802_880
        );

        let header = [
            0x4d, 0x47, 0x43, 0x31, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa0, 0x3f, 0x00, 0x00,
            0x20, 0xc0, 0x00, 0x00, 0x70, 0x40, 0x00, 0x00, 0x90, 0xc0, 0x00, 0x00, 0xa8, 0x40,
            0x00, 0x00, 0xd8, 0xc0, 0x01, 0x00, 0x00, 0x00, 0x9a, 0x99, 0x19, 0x3f, 0xf9, 0xff,
            0xff, 0xff, 0x08, 0x00, 0x00, 0x00, 0xf7, 0xff, 0xff, 0xff, 0x01, 0x00, 0x00, 0x00,
            0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00,
        ];
        assert_eq!(&header[0..4], b"MGC1");
        assert_eq!(read_u32(&header, 4), 1);
        assert_eq!(
            [
                read_f32(&header, 8).to_bits(),
                read_f32(&header, 12).to_bits(),
                read_f32(&header, 16).to_bits()
            ],
            [1.25_f32.to_bits(), (-2.5_f32).to_bits(), 3.75_f32.to_bits()]
        );
        assert_eq!(
            [
                read_f32(&header, 20).to_bits(),
                read_f32(&header, 24).to_bits(),
                read_f32(&header, 28).to_bits()
            ],
            [
                (-4.5_f32).to_bits(),
                5.25_f32.to_bits(),
                (-6.75_f32).to_bits()
            ]
        );
        assert_eq!(&header[32..36], &[1, 0, 0, 0]);
        assert_eq!(COLLISION_STEP_HEIGHT_OFFSET, 36);
        assert_eq!(
            read_f32(&header, COLLISION_STEP_HEIGHT_OFFSET).to_bits(),
            0.6_f32.to_bits()
        );
        assert_eq!(
            [
                read_i32(&header, 40),
                read_i32(&header, 44),
                read_i32(&header, 48)
            ],
            [-7, 8, -9]
        );
        assert_eq!(
            [
                read_u32(&header, 52),
                read_u32(&header, 56),
                read_u32(&header, 60)
            ],
            [1, 2, 3]
        );

        let cell_prefix = [
            0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x80, 0xbe, 0x00, 0x00, 0x00, 0x3e, 0x00, 0x00,
            0x00, 0x3f, 0x00, 0x00, 0xa0, 0x3f, 0x00, 0x00, 0x60, 0x3f, 0x00, 0x00, 0xc0, 0x3f,
        ];
        assert_eq!(&cell_prefix[0..4], &[1, 1, 0, 0]);
        for (index, want) in [-0.25_f32, 0.125, 0.5, 1.25, 0.875, 1.5]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                read_f32(&cell_prefix, 4 + index * 4).to_bits(),
                want.to_bits()
            );
        }

        let output = [
            0x00, 0x00, 0xa0, 0x3f, 0x00, 0x00, 0x20, 0xc0, 0x00, 0x00, 0x70, 0x40, 0x05, 0x01,
            0x00, 0x01,
        ];
        assert_eq!(read_f32(&output, 0).to_bits(), 1.25_f32.to_bits());
        assert_eq!(read_f32(&output, 4).to_bits(), (-2.5_f32).to_bits());
        assert_eq!(read_f32(&output, 8).to_bits(), 3.75_f32.to_bits());
        assert_eq!(&output[12..16], &[5, 1, 0, 1]);
    }

    #[test]
    fn collision_panic_is_contained_without_result() {
        let result = catch_collision(|| -> Result<[u8; COLLISION_OUTPUT_BYTES], u32> {
            panic!("测试 panic")
        });
        assert_eq!(result, Err(MORNLEA_STATUS_PANIC));
    }

    #[test]
    fn collision_panic_through_publish_path_keeps_caller_output_unchanged() {
        let mut input = [0_u8; 64 + 4 * 196];
        input[0..4].copy_from_slice(b"MGC1");
        input[4..8].copy_from_slice(&1_u32.to_le_bytes());
        for (offset, value) in [(8, 0.5_f32), (12, 1.0), (16, 0.5), (36, 0.6)] {
            input[offset..offset + 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        input[52..56].copy_from_slice(&1_u32.to_le_bytes());
        input[56..60].copy_from_slice(&4_u32.to_le_bytes());
        input[60..64].copy_from_slice(&1_u32.to_le_bytes());
        let mut caller_output = [0xa5_u8; COLLISION_OUTPUT_BYTES + 2];

        let status = unsafe {
            collision_resolve_with(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                caller_output[1..].as_mut_ptr(),
                COLLISION_OUTPUT_BYTES,
                |_| -> [u8; COLLISION_OUTPUT_BYTES] { panic!("测试 panic") },
            )
        };

        assert_eq!(status, MORNLEA_STATUS_PANIC);
        assert_eq!(caller_output, [0xa5; COLLISION_OUTPUT_BYTES + 2]);
    }

    #[test]
    fn raycast_panic_through_publish_path_is_atomic() {
        // The panic boundary itself still converges to status 9: prove it
        // directly against `catch_raycast`, since the production route no
        // longer takes an injectable resolver.
        let result = catch_raycast(|| -> Result<RaycastBatch, u32> {
            panic!("raycast panic boundary probe")
        });
        assert!(matches!(result, Err(MORNLEA_STATUS_PANIC)));

        // And the real shared core never panics on the tested vectors: a
        // valid call publishes through the same path with arenas intact
        // outside the published ranges.
        let input = valid_raycast_input();
        let mut cursor_arena = [0xa5_u8; RAYCAST_CURSOR_BYTES + 2];
        cursor_arena[1..1 + RAYCAST_CURSOR_BYTES].copy_from_slice(&fresh_raycast_cursor());
        let mut output_arena = [0xa5_u8; RAYCAST_OUTPUT_BYTES + 2];
        let mut count = usize::MAX;
        let mut done = 0xff;

        let status = unsafe {
            raycast_batch_with(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                cursor_arena[1..].as_mut_ptr(),
                RAYCAST_CURSOR_BYTES,
                output_arena[1..].as_mut_ptr(),
                RAYCAST_OUTPUT_BYTES,
                &mut count,
                &mut done,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(cursor_arena[0], 0xa5);
        assert_eq!(cursor_arena[RAYCAST_CURSOR_BYTES + 1], 0xa5);
        assert_eq!(output_arena[0], 0xa5);
        assert_eq!(output_arena[RAYCAST_OUTPUT_BYTES + 1], 0xa5);
    }

    #[test]
    fn raycast_success_publishes_local_cursor_and_output_once() {
        // The real shared core stages the full local result and publishes
        // cursor, output, count, and done together only on success.
        let input = valid_raycast_input();
        let mut cursor_arena = [0xa5_u8; RAYCAST_CURSOR_BYTES + 2];
        cursor_arena[1..1 + RAYCAST_CURSOR_BYTES].copy_from_slice(&fresh_raycast_cursor());
        let mut output_arena = [0xa5_u8; RAYCAST_OUTPUT_BYTES + 2];
        let mut count = usize::MAX;
        let mut done = 0xff;

        let status = unsafe {
            raycast_batch_with(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                cursor_arena[1..].as_mut_ptr(),
                RAYCAST_CURSOR_BYTES,
                output_arena[1..].as_mut_ptr(),
                RAYCAST_OUTPUT_BYTES,
                &mut count,
                &mut done,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(cursor_arena[0], 0xa5);
        assert_eq!(cursor_arena[RAYCAST_CURSOR_BYTES + 1], 0xa5);
        assert_eq!(output_arena[0], 0xa5);
        assert_eq!(output_arena[RAYCAST_OUTPUT_BYTES + 1], 0xa5);
        // The valid fixture ray (origin [0.5,-1.25,2.75], direction
        // [+X,0,0], maximum 6.0) publishes at least the origin record and
        // advances the cursor state past fresh.
        assert!(count >= 1);
        assert_ne!(
            &cursor_arena[1..1 + RAYCAST_CURSOR_BYTES],
            &fresh_raycast_cursor()
        );
    }

    #[test]
    fn null_raycast_buffer_clears_metadata_before_invalid_argument() {
        for buffer in [
            RaycastBuffer::Input,
            RaycastBuffer::Cursor,
            RaycastBuffer::Output,
        ] {
            let input = valid_raycast_input();
            let mut cursor = fresh_raycast_cursor();
            let mut output = [0xa5_u8; RAYCAST_OUTPUT_BYTES];
            let mut count = usize::MAX;
            let mut done = 0xff;
            let input_pointer = if matches!(buffer, RaycastBuffer::Input) {
                std::ptr::null()
            } else {
                input.as_ptr()
            };
            let cursor_pointer = if matches!(buffer, RaycastBuffer::Cursor) {
                std::ptr::null_mut()
            } else {
                cursor.as_mut_ptr()
            };
            let output_pointer = if matches!(buffer, RaycastBuffer::Output) {
                std::ptr::null_mut()
            } else {
                output.as_mut_ptr()
            };

            let status = unsafe {
                super::mornlea_raycast_batch(
                    ABI_VERSION,
                    input_pointer,
                    input.len(),
                    cursor_pointer,
                    cursor.len(),
                    output_pointer,
                    output.len(),
                    &mut count,
                    &mut done,
                )
            };

            assert_eq!((count, done), (0, 0), "{buffer:?}");
            assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT, "{buffer:?}");
        }
    }

    #[test]
    fn wrapping_raycast_buffer_clears_metadata_without_publishing() {
        for buffer in [
            RaycastBuffer::Input,
            RaycastBuffer::Cursor,
            RaycastBuffer::Output,
        ] {
            let input = valid_raycast_input();
            let mut cursor = fresh_raycast_cursor();
            let mut output = [0xa5_u8; RAYCAST_OUTPUT_BYTES];
            let before_input = input;
            let before_cursor = cursor;
            let before_output = output;
            let mut count = usize::MAX;
            let mut done = 0xff;
            let input_pointer = if matches!(buffer, RaycastBuffer::Input) {
                std::ptr::without_provenance::<u8>(usize::MAX)
            } else {
                input.as_ptr()
            };
            let cursor_pointer = if matches!(buffer, RaycastBuffer::Cursor) {
                std::ptr::without_provenance_mut::<u8>(usize::MAX)
            } else {
                cursor.as_mut_ptr()
            };
            let output_pointer = if matches!(buffer, RaycastBuffer::Output) {
                std::ptr::without_provenance_mut::<u8>(usize::MAX)
            } else {
                output.as_mut_ptr()
            };

            let status = unsafe {
                super::mornlea_raycast_batch(
                    ABI_VERSION,
                    input_pointer,
                    RAYCAST_INPUT_BYTES,
                    cursor_pointer,
                    RAYCAST_CURSOR_BYTES,
                    output_pointer,
                    RAYCAST_OUTPUT_BYTES,
                    &mut count,
                    &mut done,
                )
            };

            assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT, "{buffer:?}");
            assert_eq!((count, done), (0, 0), "{buffer:?}");
            assert_eq!(input, before_input, "{buffer:?}");
            assert_eq!(cursor, before_cursor, "{buffer:?}");
            assert_eq!(output, before_output, "{buffer:?}");
        }
    }

    #[test]
    fn raycast_metadata_alias_wins_over_a_wrapping_other_buffer() {
        let input = valid_raycast_input();
        let mut cursor = AlignedBytes(fresh_raycast_cursor());
        let mut output = AlignedBytes([0xa5_u8; RAYCAST_OUTPUT_BYTES]);
        let before_input = input;
        let before_cursor = cursor.0;
        let before_output = output.0;
        let mut done = 0xff;

        let status = unsafe {
            super::mornlea_raycast_batch(
                ABI_VERSION,
                std::ptr::without_provenance::<u8>(usize::MAX),
                RAYCAST_INPUT_BYTES,
                cursor.0.as_mut_ptr(),
                RAYCAST_CURSOR_BYTES,
                output.0.as_mut_ptr(),
                RAYCAST_OUTPUT_BYTES,
                cursor.0.as_mut_ptr().cast::<usize>(),
                &mut done,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(done, 0xff);
        assert_eq!(input, before_input);
        assert_eq!(cursor.0, before_cursor);
        assert_eq!(output.0, before_output);
    }

    #[test]
    fn invalid_raycast_metadata_is_not_partially_cleared() {
        let input = valid_raycast_input();
        let mut cursor = fresh_raycast_cursor();
        let mut output = [0xa5_u8; RAYCAST_OUTPUT_BYTES];
        let before_cursor = cursor;
        let before_output = output;
        let mut count = usize::MAX;

        let status = unsafe {
            super::mornlea_raycast_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                cursor.as_mut_ptr(),
                cursor.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut count,
                std::ptr::null_mut(),
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(count, usize::MAX);
        assert_eq!(cursor, before_cursor);
        assert_eq!(output, before_output);
    }

    #[test]
    fn overlapping_raycast_metadata_is_not_partially_cleared() {
        let input = valid_raycast_input();
        let mut cursor = fresh_raycast_cursor();
        let mut output = [0xa5_u8; RAYCAST_OUTPUT_BYTES];
        let before_cursor = cursor;
        let before_output = output;
        let mut count = usize::MAX;

        let status = unsafe {
            super::mornlea_raycast_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                cursor.as_mut_ptr(),
                cursor.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut count,
                (&mut count as *mut usize).cast(),
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(count, usize::MAX);
        assert_eq!(cursor, before_cursor);
        assert_eq!(output, before_output);
    }

    #[test]
    fn raycast_metadata_overlapping_cursor_is_not_published() {
        #[repr(align(8))]
        struct AlignedCursor([u8; RAYCAST_CURSOR_BYTES]);

        let input = valid_raycast_input();
        let mut cursor = AlignedCursor(fresh_raycast_cursor());
        let mut output = [0xa5_u8; RAYCAST_OUTPUT_BYTES];
        let before_cursor = cursor.0;
        let before_output = output;
        let mut done = 0xff;

        let status = unsafe {
            super::mornlea_raycast_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                cursor.0.as_mut_ptr(),
                cursor.0.len(),
                output.as_mut_ptr(),
                output.len(),
                cursor.0.as_mut_ptr().cast(),
                &mut done,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(done, 0xff);
        assert_eq!(cursor.0, before_cursor);
        assert_eq!(output, before_output);
    }

    #[test]
    fn malformed_raycast_input_and_cursor_matrix_is_atomic() {
        let valid_input = valid_raycast_input();
        let fresh = fresh_raycast_cursor();
        let mut active_input = valid_input;
        active_input[32..36].copy_from_slice(&130.0_f32.to_bits().to_le_bytes());
        let active_result = raycast_batch(&active_input, &fresh);
        assert_eq!((active_result.count, active_result.done), (64, false));
        let done_result = raycast_batch(&valid_input, &fresh);
        assert!(done_result.done);

        let mut cases = Vec::new();
        push_raycast_mutation(&mut cases, "input magic", valid_input, fresh, |input, _| {
            input[0] = b'X';
        });
        push_raycast_mutation(
            &mut cases,
            "input layout",
            valid_input,
            fresh,
            |input, _| {
                input[4] = 2;
            },
        );
        push_raycast_mutation(
            &mut cases,
            "input reserved",
            valid_input,
            fresh,
            |input, _| {
                input[36] = 1;
            },
        );
        push_raycast_mutation(
            &mut cases,
            "input origin NaN",
            valid_input,
            fresh,
            |input, _| {
                write_test_f32(input, 8, f32::NAN);
            },
        );
        push_raycast_mutation(
            &mut cases,
            "input direction NaN",
            valid_input,
            fresh,
            |input, _| {
                write_test_f32(input, 20, f32::NAN);
            },
        );
        push_raycast_mutation(
            &mut cases,
            "input zero direction",
            valid_input,
            fresh,
            |input, _| {
                input[20..32].fill(0);
            },
        );
        push_raycast_mutation(
            &mut cases,
            "input maximum NaN",
            valid_input,
            fresh,
            |input, _| {
                write_test_f32(input, 32, f32::NAN);
            },
        );
        push_raycast_mutation(
            &mut cases,
            "input maximum zero",
            valid_input,
            fresh,
            |input, _| {
                write_test_f32(input, 32, 0.0);
            },
        );
        push_raycast_mutation(
            &mut cases,
            "cursor magic",
            valid_input,
            fresh,
            |_, cursor| {
                cursor[0] = b'X';
            },
        );
        push_raycast_mutation(
            &mut cases,
            "cursor layout",
            valid_input,
            fresh,
            |_, cursor| {
                cursor[4] = 2;
            },
        );
        push_raycast_mutation(
            &mut cases,
            "cursor state 3",
            valid_input,
            fresh,
            |_, cursor| {
                cursor[8] = 3;
            },
        );
        push_raycast_mutation(
            &mut cases,
            "cursor header reserved",
            valid_input,
            fresh,
            |_, cursor| {
                cursor[9] = 1;
            },
        );
        push_raycast_mutation(
            &mut cases,
            "cursor tail reserved",
            valid_input,
            fresh,
            |_, cursor| {
                cursor[60] = 1;
            },
        );
        push_raycast_mutation(
            &mut cases,
            "fresh nonzero payload",
            valid_input,
            fresh,
            |_, cursor| {
                cursor[12] = 1;
            },
        );

        for (state_name, input, base) in [
            ("active", active_input, active_result.cursor),
            ("done", valid_input, done_result.cursor),
        ] {
            push_raycast_mutation(
                &mut cases,
                &format!("{state_name} step"),
                input,
                base,
                |_, cursor| {
                    cursor[24..28].fill(0);
                },
            );
            push_raycast_mutation(
                &mut cases,
                &format!("{state_name} delta NaN"),
                input,
                base,
                |_, cursor| {
                    write_test_f32(cursor, 36, f32::NAN);
                },
            );
            push_raycast_mutation(
                &mut cases,
                &format!("{state_name} delta -Inf"),
                input,
                base,
                |_, cursor| {
                    write_test_f32(cursor, 36, f32::NEG_INFINITY);
                },
            );
            push_raycast_mutation(
                &mut cases,
                &format!("{state_name} delta zero"),
                input,
                base,
                |_, cursor| {
                    write_test_f32(cursor, 36, 0.0);
                },
            );
            push_raycast_mutation(
                &mut cases,
                &format!("{state_name} delta finite mismatch"),
                input,
                base,
                |_, cursor| {
                    write_test_f32(cursor, 36, 2.0);
                },
            );
            push_raycast_mutation(
                &mut cases,
                &format!("{state_name} maximum NaN"),
                input,
                base,
                |_, cursor| {
                    write_test_f32(cursor, 48, f32::NAN);
                },
            );
            push_raycast_mutation(
                &mut cases,
                &format!("{state_name} maximum -Inf"),
                input,
                base,
                |_, cursor| {
                    write_test_f32(cursor, 48, f32::NEG_INFINITY);
                },
            );
            push_raycast_mutation(
                &mut cases,
                &format!("{state_name} zero-axis delta finite"),
                input,
                base,
                |_, cursor| write_test_f32(cursor, 40, 1.0),
            );
            push_raycast_mutation(
                &mut cases,
                &format!("{state_name} zero-axis maximum finite"),
                input,
                base,
                |_, cursor| write_test_f32(cursor, 52, 1.0),
            );
        }

        for (name, input, cursor) in cases {
            assert_raycast_input_failure_is_atomic(&name, input, cursor);
        }
    }

    #[test]
    fn invalid_raycast_metadata_pointer_matrix_is_atomic() {
        for kind in [
            InvalidMetadataPointer::MisalignedCount,
            InvalidMetadataPointer::WrappingCount,
            InvalidMetadataPointer::WrappingDone,
        ] {
            assert_invalid_raycast_metadata_pointer_is_atomic(kind);
        }
    }

    #[test]
    fn raycast_metadata_buffer_overlap_matrix_is_atomic() {
        for metadata in [MetadataField::Count, MetadataField::Done] {
            for buffer in [
                RaycastBuffer::Input,
                RaycastBuffer::Cursor,
                RaycastBuffer::Output,
            ] {
                for length in [
                    RaycastBufferLength::Exact,
                    RaycastBufferLength::Zero,
                    RaycastBufferLength::Short,
                    RaycastBufferLength::Long,
                    RaycastBufferLength::Wrapping,
                ] {
                    assert_raycast_metadata_overlap_is_atomic(metadata, buffer, length);
                }
            }
        }
    }

    fn valid_raycast_input() -> [u8; RAYCAST_INPUT_BYTES] {
        let mut input = [0_u8; RAYCAST_INPUT_BYTES];
        input[0..4].copy_from_slice(b"MGR1");
        input[4..8].copy_from_slice(&1_u32.to_le_bytes());
        for (offset, value) in [(8, 0.5_f32), (12, -1.25), (16, 2.75), (20, 1.0), (32, 6.0)] {
            input[offset..offset + 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        input
    }

    fn fresh_raycast_cursor() -> [u8; RAYCAST_CURSOR_BYTES] {
        let mut cursor = [0_u8; RAYCAST_CURSOR_BYTES];
        cursor[0..4].copy_from_slice(b"MRC1");
        cursor[4..8].copy_from_slice(&1_u32.to_le_bytes());
        cursor
    }

    fn push_raycast_mutation(
        cases: &mut Vec<(
            String,
            [u8; RAYCAST_INPUT_BYTES],
            [u8; RAYCAST_CURSOR_BYTES],
        )>,
        name: &str,
        mut input: [u8; RAYCAST_INPUT_BYTES],
        mut cursor: [u8; RAYCAST_CURSOR_BYTES],
        mutate: impl FnOnce(&mut [u8; RAYCAST_INPUT_BYTES], &mut [u8; RAYCAST_CURSOR_BYTES]),
    ) {
        mutate(&mut input, &mut cursor);
        cases.push((name.to_owned(), input, cursor));
    }

    fn write_test_f32<const N: usize>(bytes: &mut [u8; N], offset: usize, value: f32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }

    fn assert_raycast_input_failure_is_atomic(
        name: &str,
        input: [u8; RAYCAST_INPUT_BYTES],
        cursor: [u8; RAYCAST_CURSOR_BYTES],
    ) {
        let mut input_arena = [0xa5_u8; RAYCAST_INPUT_BYTES + 2];
        input_arena[1..1 + RAYCAST_INPUT_BYTES].copy_from_slice(&input);
        let mut cursor_arena = [0xa5_u8; RAYCAST_CURSOR_BYTES + 2];
        cursor_arena[1..1 + RAYCAST_CURSOR_BYTES].copy_from_slice(&cursor);
        let mut output_arena = [0xa5_u8; RAYCAST_OUTPUT_BYTES + 2];
        let before_input = input_arena;
        let before_cursor = cursor_arena;
        let before_output = output_arena;
        let mut count = usize::MAX;
        let mut done = 0xff;

        let status = unsafe {
            super::mornlea_raycast_batch(
                ABI_VERSION,
                input_arena[1..].as_ptr(),
                RAYCAST_INPUT_BYTES,
                cursor_arena[1..].as_mut_ptr(),
                RAYCAST_CURSOR_BYTES,
                output_arena[1..].as_mut_ptr(),
                RAYCAST_OUTPUT_BYTES,
                &mut count,
                &mut done,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INPUT, "{name}");
        assert_eq!((count, done), (0, 0), "{name}");
        assert_eq!(input_arena, before_input, "{name}");
        assert_eq!(cursor_arena, before_cursor, "{name}");
        assert_eq!(output_arena, before_output, "{name}");
    }

    #[derive(Clone, Copy, Debug)]
    enum InvalidMetadataPointer {
        MisalignedCount,
        WrappingCount,
        WrappingDone,
    }

    fn assert_invalid_raycast_metadata_pointer_is_atomic(kind: InvalidMetadataPointer) {
        let input = valid_raycast_input();
        let mut cursor_arena = [0xa5_u8; RAYCAST_CURSOR_BYTES + 2];
        cursor_arena[1..1 + RAYCAST_CURSOR_BYTES].copy_from_slice(&fresh_raycast_cursor());
        let mut output_arena = [0xa5_u8; RAYCAST_OUTPUT_BYTES + 2];
        let before_cursor = cursor_arena;
        let before_output = output_arena;
        let mut count = usize::MAX;
        let mut done = 0xff;
        let mut count_bytes = AlignedBytes([0xa5_u8; size_of::<usize>() + 1]);
        let before_count_bytes = count_bytes.0;
        let (count_pointer, done_pointer): (*mut usize, *mut u8) = match kind {
            InvalidMetadataPointer::MisalignedCount => (
                unsafe { count_bytes.0.as_mut_ptr().add(1).cast::<usize>() },
                &mut done,
            ),
            InvalidMetadataPointer::WrappingCount => (
                (usize::MAX & !(align_of::<usize>() - 1)) as *mut usize,
                &mut done,
            ),
            InvalidMetadataPointer::WrappingDone => (&mut count, usize::MAX as *mut u8),
        };

        let status = unsafe {
            super::mornlea_raycast_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                cursor_arena[1..].as_mut_ptr(),
                RAYCAST_CURSOR_BYTES,
                output_arena[1..].as_mut_ptr(),
                RAYCAST_OUTPUT_BYTES,
                count_pointer,
                done_pointer,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT, "{kind:?}");
        assert_eq!(count, usize::MAX, "{kind:?}");
        assert_eq!(done, 0xff, "{kind:?}");
        assert_eq!(count_bytes.0, before_count_bytes, "{kind:?}");
        assert_eq!(cursor_arena, before_cursor, "{kind:?}");
        assert_eq!(output_arena, before_output, "{kind:?}");
    }

    #[derive(Clone, Copy, Debug)]
    enum MetadataField {
        Count,
        Done,
    }

    #[derive(Clone, Copy, Debug)]
    enum RaycastBuffer {
        Input,
        Cursor,
        Output,
    }

    #[derive(Clone, Copy, Debug)]
    enum RaycastBufferLength {
        Exact,
        Zero,
        Short,
        Long,
        Wrapping,
    }

    #[repr(align(8))]
    struct AlignedBytes<const N: usize>([u8; N]);

    fn raycast_buffer_length(buffer: RaycastBuffer, length: RaycastBufferLength) -> usize {
        let exact = match buffer {
            RaycastBuffer::Input => RAYCAST_INPUT_BYTES,
            RaycastBuffer::Cursor => RAYCAST_CURSOR_BYTES,
            RaycastBuffer::Output => RAYCAST_OUTPUT_BYTES,
        };
        match length {
            RaycastBufferLength::Exact => exact,
            RaycastBufferLength::Zero => 0,
            RaycastBufferLength::Short => exact - 1,
            RaycastBufferLength::Long => exact + 1,
            RaycastBufferLength::Wrapping => usize::MAX,
        }
    }

    fn assert_raycast_metadata_overlap_is_atomic(
        metadata: MetadataField,
        buffer: RaycastBuffer,
        length: RaycastBufferLength,
    ) {
        const CANARY_BYTES: usize = 8;
        let mut input_arena = AlignedBytes([0xa5_u8; RAYCAST_INPUT_BYTES + 2 * CANARY_BYTES]);
        input_arena.0[CANARY_BYTES..CANARY_BYTES + RAYCAST_INPUT_BYTES]
            .copy_from_slice(&valid_raycast_input());
        let mut cursor_arena = AlignedBytes([0xa5_u8; RAYCAST_CURSOR_BYTES + 2 * CANARY_BYTES]);
        cursor_arena.0[CANARY_BYTES..CANARY_BYTES + RAYCAST_CURSOR_BYTES]
            .copy_from_slice(&fresh_raycast_cursor());
        let mut output_arena = AlignedBytes([0xa5_u8; RAYCAST_OUTPUT_BYTES + 2 * CANARY_BYTES]);
        let before_input = input_arena.0;
        let before_cursor = cursor_arena.0;
        let before_output = output_arena.0;
        let input_pointer = unsafe { input_arena.0.as_mut_ptr().add(CANARY_BYTES) };
        let cursor_pointer = unsafe { cursor_arena.0.as_mut_ptr().add(CANARY_BYTES) };
        let output_pointer = unsafe { output_arena.0.as_mut_ptr().add(CANARY_BYTES) };
        let overlap_pointer = match buffer {
            RaycastBuffer::Input => input_pointer,
            RaycastBuffer::Cursor => cursor_pointer,
            RaycastBuffer::Output => output_pointer,
        };
        let mut count = usize::MAX;
        let mut done = 0xff;
        let count_pointer = match metadata {
            MetadataField::Count => overlap_pointer.cast::<usize>(),
            MetadataField::Done => &mut count,
        };
        let done_pointer = match metadata {
            MetadataField::Count => &mut done,
            MetadataField::Done => overlap_pointer,
        };
        let input_len = if matches!(buffer, RaycastBuffer::Input) {
            raycast_buffer_length(buffer, length)
        } else {
            RAYCAST_INPUT_BYTES
        };
        let cursor_len = if matches!(buffer, RaycastBuffer::Cursor) {
            raycast_buffer_length(buffer, length)
        } else {
            RAYCAST_CURSOR_BYTES
        };
        let output_len = if matches!(buffer, RaycastBuffer::Output) {
            raycast_buffer_length(buffer, length)
        } else {
            RAYCAST_OUTPUT_BYTES
        };

        let status = unsafe {
            super::mornlea_raycast_batch(
                ABI_VERSION,
                input_pointer,
                input_len,
                cursor_pointer,
                cursor_len,
                output_pointer,
                output_len,
                count_pointer,
                done_pointer,
            )
        };

        assert_eq!(
            input_arena.0, before_input,
            "{metadata:?}/{buffer:?}/{length:?}"
        );
        assert_eq!(
            cursor_arena.0, before_cursor,
            "{metadata:?}/{buffer:?}/{length:?}"
        );
        assert_eq!(
            output_arena.0, before_output,
            "{metadata:?}/{buffer:?}/{length:?}"
        );
        assert_eq!(count, usize::MAX, "{metadata:?}/{buffer:?}/{length:?}");
        assert_eq!(done, 0xff, "{metadata:?}/{buffer:?}/{length:?}");
        assert_eq!(
            status, MORNLEA_STATUS_INVALID_ARGUMENT,
            "{metadata:?}/{buffer:?}/{length:?}"
        );
    }

    #[test]
    fn malformed_collision_input_keeps_output_unchanged() {
        let mut input = [0_u8; 64 + 4 * 196];
        input[0..4].copy_from_slice(b"MGC1");
        input[4..8].copy_from_slice(&1_u32.to_le_bytes());
        for (offset, value) in [(8, 0.5_f32), (12, 1.0), (16, 0.5), (36, 0.6)] {
            input[offset..offset + 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        input[32] = 1;
        input[33] = 1;
        input[52..56].copy_from_slice(&1_u32.to_le_bytes());
        input[56..60].copy_from_slice(&4_u32.to_le_bytes());
        input[60..64].copy_from_slice(&1_u32.to_le_bytes());
        let mut output = [0xa5_u8; 16];

        let status = unsafe {
            mornlea_collision_resolve(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INPUT);
        assert_eq!(output, [0xa5; 16]);
    }

    #[test]
    fn valid_input_returns_ok_and_zero_quads() {
        let input = valid_input();
        let mut scratch = vec![0_u32; (48 * 48 * 48 * 5) / 4];
        let mut output = vec![0_u64; 6 * 4096];
        let mut output_len = usize::MAX;
        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(output_len, 0);
    }

    #[test]
    fn uniform_air_returns_without_touching_light_scratch() {
        const BLOCKS_OFFSET: usize = 16;
        let mut input = valid_input();
        input[BLOCKS_OFFSET..BLOCKS_OFFSET + 27 * 4096 * 2].fill(0);
        let mut scratch = vec![0xa5a5_a5a5_u32; (48 * 48 * 48 * 5) / 4];
        let mut output = vec![0_u64; 6 * 4096];
        let mut output_len = usize::MAX;

        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(output_len, 0);
        assert!(scratch.iter().all(|&word| word == 0xa5a5_a5a5));
    }

    #[test]
    fn uniform_air_skips_unused_registry_semantics_and_light() {
        const BLOCKS_OFFSET: usize = 16;
        const BLOCKS_BYTES: usize = 27 * 4096 * 2;
        const REGISTRY_OFFSET: usize = BLOCKS_OFFSET + BLOCKS_BYTES + 9 + 9 * 256 * 2;
        const ENTRY_BYTES: usize = crate::input::tests::ENTRY_BYTES;
        let mut base = valid_input();
        base[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].fill(0);
        base[BLOCKS_OFFSET..BLOCKS_OFFSET + 2].copy_from_slice(&40000_u16.to_le_bytes());

        let cases = [
            ("overbright", {
                let mut input = base.clone();
                input[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 3] = 16;
                input
            }),
            ("bad opacity", {
                let mut input = base.clone();
                input[REGISTRY_OFFSET + 2] = 2;
                input
            }),
            ("duplicate id", {
                let mut input = base.clone();
                input[REGISTRY_OFFSET + ENTRY_BYTES..REGISTRY_OFFSET + ENTRY_BYTES + 2]
                    .copy_from_slice(&0_u16.to_le_bytes());
                input
            }),
            ("same air and barrier", {
                let mut input = base.clone();
                input[14..16].copy_from_slice(&0_u16.to_le_bytes());
                input
            }),
            ("missing barrier", {
                let mut input = base;
                input[REGISTRY_OFFSET + ENTRY_BYTES..REGISTRY_OFFSET + ENTRY_BYTES + 2]
                    .copy_from_slice(&2_u16.to_le_bytes());
                input
            }),
        ];

        for (name, input) in cases {
            let mut scratch = vec![0xa5a5_a5a5_u32; (48 * 48 * 48 * 5) / 4];
            let mut output = vec![0_u64; 6 * 4096];
            let mut output_len = usize::MAX;
            let status = unsafe {
                mornlea_mesh_section(
                    ABI_VERSION,
                    input.as_ptr(),
                    input.len(),
                    scratch.as_mut_ptr().cast(),
                    scratch.len() * 4,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut output_len,
                )
            };

            assert_eq!(status, MORNLEA_STATUS_OK, "{name}");
            assert_eq!(output_len, 0, "{name}");
            assert!(scratch.iter().all(|&word| word == 0xa5a5_a5a5), "{name}");
        }
    }

    #[test]
    fn uniform_air_still_rejects_structural_presence_error() {
        const BLOCKS_OFFSET: usize = 16;
        const BLOCKS_BYTES: usize = 27 * 4096 * 2;
        let mut input = valid_input();
        input[BLOCKS_OFFSET..BLOCKS_OFFSET + BLOCKS_BYTES].fill(0);
        input[BLOCKS_OFFSET + BLOCKS_BYTES] = 2;
        let mut scratch = vec![0xa5a5_a5a5_u32; (48 * 48 * 48 * 5) / 4];
        let mut output = vec![0_u64; 6 * 4096];
        let mut output_len = usize::MAX;

        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INPUT);
        assert_eq!(output_len, 0);
        assert!(scratch.iter().all(|&word| word == 0xa5a5_a5a5));
    }

    #[test]
    fn ffi_publishes_six_quads_only_after_complete_mesh() {
        const BLOCKS_OFFSET: usize = 16;
        const REGISTRY_OFFSET: usize = BLOCKS_OFFSET + 27 * 4096 * 2 + 9 + 9 * 256 * 2;
        const ENTRY_BYTES: usize = crate::input::tests::ENTRY_BYTES;
        let mut input = valid_input();
        input[BLOCKS_OFFSET..BLOCKS_OFFSET + 27 * 4096 * 2].fill(0);
        input[REGISTRY_OFFSET + 3 * ENTRY_BYTES..REGISTRY_OFFSET + 3 * ENTRY_BYTES + 8]
            .copy_from_slice(&0_u64.to_le_bytes());
        let center = BLOCKS_OFFSET + (13 * 4096 + ((8 << 8) | (8 << 4) | 8)) * 2;
        input[center..center + 2].copy_from_slice(&1_u16.to_le_bytes());
        let mut scratch = vec![0_u32; (48 * 48 * 48 * 5) / 4];
        let mut output = vec![0_u64; 6 * 4096];
        let mut output_len = usize::MAX;

        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(output_len, 6);
        assert_eq!(
            output[..6]
                .iter()
                .map(|packed| (packed >> 20) & 7)
                .sum::<u64>(),
            15
        );
    }

    #[test]
    fn input_range_rejects_slice_size_overflow_and_address_wrap() {
        let byte = 0_u8;
        assert!(input_range_is_valid(&byte, 1));
        assert!(!input_range_is_valid(&byte, isize::MAX as usize + 1));
        assert!(!input_range_is_valid(
            std::ptr::without_provenance(usize::MAX),
            1
        ));
    }

    #[test]
    fn scratch_range_rejects_aligned_address_wrap() {
        let mut storage = vec![0_u64; SCRATCH_BYTES.div_ceil(size_of::<u64>())];
        assert!(scratch_range_is_valid(
            storage.as_mut_ptr().cast(),
            SCRATCH_BYTES
        ));

        let aligned_max = usize::MAX & !(align_of::<u64>() - 1);
        assert!(!scratch_range_is_valid(
            std::ptr::without_provenance_mut(aligned_max),
            SCRATCH_BYTES
        ));
    }

    #[test]
    fn output_range_rejects_address_and_capacity_overflow() {
        let mut output = 0_u64;
        assert!(output_range_is_valid(&mut output, 1));
        assert!(!output_range_is_valid(
            std::ptr::without_provenance_mut(usize::MAX),
            1
        ));
        assert!(!output_range_is_valid(&mut output, usize::MAX));
    }

    #[test]
    fn oversized_input_len_returns_input_atomically() {
        let input = valid_input();
        let mut scratch = vec![0_u32; (48 * 48 * 48 * 5) / 4];
        let mut output = vec![0_u64; 6 * 4096];
        let mut output_len = usize::MAX;
        // SAFETY: 被测入口在构造 slice 前拒绝超过 isize::MAX 的长度。
        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                isize::MAX as usize + 1,
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INPUT);
        assert_eq!(output_len, usize::MAX);
    }

    #[test]
    fn wrapping_input_range_returns_input_atomically() {
        let mut scratch = vec![0_u32; (48 * 48 * 48 * 5) / 4];
        let mut output = vec![0_u64; 6 * 4096];
        let mut output_len = usize::MAX;
        // SAFETY: 被测入口在构造 slice 前拒绝地址加一发生回绕的范围。
        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                std::ptr::without_provenance(usize::MAX),
                1,
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INPUT);
        assert_eq!(output_len, usize::MAX);
    }

    #[test]
    fn status_numbers_match_the_c_abi() {
        assert_eq!(
            [
                MORNLEA_STATUS_OK,
                MORNLEA_STATUS_ABI_VERSION,
                MORNLEA_STATUS_INVALID_ARGUMENT,
                MORNLEA_STATUS_INPUT,
                MORNLEA_STATUS_SCRATCH,
                MORNLEA_STATUS_REGISTRY,
                MORNLEA_STATUS_EMISSION,
                MORNLEA_STATUS_OUTPUT_OVERFLOW,
                MORNLEA_STATUS_QUEUE_OVERFLOW,
                MORNLEA_STATUS_PANIC,
            ],
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
    }

    #[test]
    fn abi_version_failure_is_atomic() {
        let input = valid_input();
        let mut scratch = vec![0_u32; (48 * 48 * 48 * 5) / 4];
        let mut output = vec![0_u64; 6 * 4096];
        let mut output_len = usize::MAX;
        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION + 1,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_ABI_VERSION);
        assert_eq!(output_len, usize::MAX);
    }

    #[test]
    fn invalid_arguments_and_inputs_return_exact_atomic_statuses() {
        const REGISTRY_OFFSET: usize = 225817;
        const ENTRY_BYTES: usize = crate::input::tests::ENTRY_BYTES;
        let input = valid_input();
        let mut scratch = vec![0_u32; (48 * 48 * 48 * 5) / 4];
        let mut output = vec![0_u64; 6 * 4096];

        let mut cases = vec![
            (
                "short input",
                input[..input.len() - 1].to_vec(),
                MORNLEA_STATUS_INPUT,
            ),
            (
                "long input",
                {
                    let mut long = input.clone();
                    long.push(0);
                    long
                },
                MORNLEA_STATUS_INPUT,
            ),
            (
                "malformed registry",
                {
                    let mut malformed = input.clone();
                    malformed[REGISTRY_OFFSET + ENTRY_BYTES..REGISTRY_OFFSET + ENTRY_BYTES + 2]
                        .copy_from_slice(&0_u16.to_le_bytes());
                    malformed
                },
                MORNLEA_STATUS_REGISTRY,
            ),
            (
                "overbright emission",
                {
                    let mut overbright = input.clone();
                    overbright[REGISTRY_OFFSET + 2 * ENTRY_BYTES + 3] = 16;
                    overbright
                },
                MORNLEA_STATUS_EMISSION,
            ),
        ];
        for (name, case, want) in cases.drain(..) {
            let mut output_len = usize::MAX;
            // SAFETY: 本测试提供有效对齐的独占缓冲区，长度均与切片一致。
            let status = unsafe {
                mornlea_mesh_section(
                    ABI_VERSION,
                    case.as_ptr(),
                    case.len(),
                    scratch.as_mut_ptr().cast(),
                    scratch.len() * 4,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut output_len,
                )
            };
            assert_eq!(status, want, "{name}");
            assert_eq!(output_len, 0, "{name}");
        }

        let mut output_len = usize::MAX;
        // SAFETY: 除被测的空 input 指针外，其余缓冲区均有效且对齐。
        let null_input = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                std::ptr::null(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(null_input, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        output_len = usize::MAX;
        // SAFETY: scratch 指针有效但长度被刻意缩短一个字节。
        let short_scratch = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4 - 1,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(short_scratch, MORNLEA_STATUS_SCRATCH);
        assert_eq!(output_len, usize::MAX);

        output_len = usize::MAX;
        // SAFETY: output 指针有效且对齐，capacity 被刻意缩短一个元素。
        let short_output = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                scratch.len() * 4,
                output.as_mut_ptr(),
                output.len() - 1,
                &mut output_len,
            )
        };
        assert_eq!(short_output, MORNLEA_STATUS_OUTPUT_OVERFLOW);
        assert_eq!(output_len, usize::MAX);
    }

    #[test]
    fn null_and_misaligned_buffers_are_rejected_atomically() {
        let input = valid_input();
        let mut scratch = vec![0_u64; (48 * 48 * 48 * 5) / 8 + 1];
        let mut output = vec![0_u64; 6 * 4096 + 1];
        let mut output_len = usize::MAX;

        // SAFETY: 除被测的空 output_len 指针外，其余指针均有效且对齐。
        let null_output_len = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                48 * 48 * 48 * 5,
                output.as_mut_ptr(),
                6 * 4096,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(null_output_len, MORNLEA_STATUS_INVALID_ARGUMENT);

        // SAFETY: 除被测的空 scratch 指针外，其余指针均有效且对齐。
        let null_scratch = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                std::ptr::null_mut(),
                48 * 48 * 48 * 5,
                output.as_mut_ptr(),
                6 * 4096,
                &mut output_len,
            )
        };
        assert_eq!(null_scratch, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        output_len = usize::MAX;
        // SAFETY: 除被测的空 output 指针外，其余指针均有效且对齐。
        let null_output = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                48 * 48 * 48 * 5,
                std::ptr::null_mut(),
                6 * 4096,
                &mut output_len,
            )
        };
        assert_eq!(null_output, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        output_len = usize::MAX;
        // SAFETY: scratch 分配足够大；加一字节只用于验证未对齐检查，函数不会解引用。
        let misaligned_scratch = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast::<u8>().add(1),
                48 * 48 * 48 * 5,
                output.as_mut_ptr(),
                6 * 4096,
                &mut output_len,
            )
        };
        assert_eq!(misaligned_scratch, MORNLEA_STATUS_SCRATCH);
        assert_eq!(output_len, usize::MAX);

        output_len = usize::MAX;
        // SAFETY: scratch 额外分配了一个 u64；加四字节仅用于验证 8-byte 对齐检查。
        let four_byte_aligned_scratch = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast::<u8>().add(4),
                48 * 48 * 48 * 5,
                output.as_mut_ptr(),
                6 * 4096,
                &mut output_len,
            )
        };
        assert_eq!(four_byte_aligned_scratch, MORNLEA_STATUS_SCRATCH);
        assert_eq!(output_len, usize::MAX);

        output_len = usize::MAX;
        // SAFETY: output 分配足够大；加一字节只用于验证未对齐检查，函数不会解引用。
        let misaligned_output = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                48 * 48 * 48 * 5,
                output.as_mut_ptr().cast::<u8>().add(1).cast(),
                6 * 4096,
                &mut output_len,
            )
        };
        assert_eq!(misaligned_output, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        let mut output_len_storage = [usize::MAX, usize::MAX];
        // SAFETY: output_len 分配足够大；加一字节只用于验证未对齐检查，函数不会解引用。
        let misaligned_output_len = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                48 * 48 * 48 * 5,
                output.as_mut_ptr(),
                6 * 4096,
                output_len_storage.as_mut_ptr().cast::<u8>().add(1).cast(),
            )
        };
        assert_eq!(misaligned_output_len, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len_storage, [usize::MAX, usize::MAX]);
    }

    #[test]
    fn aligned_wrapping_output_len_is_rejected_before_write() {
        let input = valid_input();
        let mut scratch = vec![0_u64; SCRATCH_BYTES.div_ceil(size_of::<u64>())];
        let mut output = vec![0_u64; 6 * 4096];
        let aligned_max = usize::MAX & !(align_of::<usize>() - 1);

        // SAFETY: output_len 为被测的对齐伪地址；入口必须在任何 write 前因地址回绕返回。
        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                SCRATCH_BYTES,
                output.as_mut_ptr(),
                output.len(),
                std::ptr::without_provenance_mut(aligned_max),
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
    }

    #[test]
    fn overlapping_scratch_and_output_are_rejected_atomically() {
        let input = valid_input();
        let mut shared = vec![0_u64; (48_usize * 48 * 48 * 5).div_ceil(8)];
        let mut output_len = usize::MAX;

        // SAFETY: 共享 buffer 容量同时满足 scratch 与 output；入口应在创建任何 Rust slice 前拒绝重叠。
        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                shared.as_mut_ptr().cast(),
                48 * 48 * 48 * 5,
                shared.as_mut_ptr(),
                6 * 4096,
                &mut output_len,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_SCRATCH);
        assert_eq!(output_len, usize::MAX);
    }

    #[test]
    fn overlapping_input_and_output_are_rejected_atomically() {
        let input = valid_input();
        let mut shared = vec![0_u64; input.len().div_ceil(size_of::<u64>())];
        // SAFETY: shared 的字节容量至少为 input.len()，这里只在调用前写入 encoded input。
        unsafe {
            std::slice::from_raw_parts_mut(shared.as_mut_ptr().cast::<u8>(), input.len())
                .copy_from_slice(&input);
        }
        let mut scratch = vec![0_u64; SCRATCH_BYTES.div_ceil(size_of::<u64>())];
        let mut output_len = usize::MAX;

        // SAFETY: 每个指针都有效且容量足够；被测入口必须在创建 slice 前拒绝 input/output 别名。
        let status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                shared.as_ptr().cast(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                SCRATCH_BYTES,
                shared.as_mut_ptr(),
                6 * 4096,
                &mut output_len,
            )
        };

        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);
    }

    #[test]
    fn overlapping_output_len_with_input_or_output_is_rejected_atomically() {
        let input = valid_input();
        let mut shared_input = vec![0_usize; input.len().div_ceil(size_of::<usize>())];
        // SAFETY: shared_input 的字节容量覆盖完整 encoded input。
        unsafe {
            std::slice::from_raw_parts_mut(shared_input.as_mut_ptr().cast::<u8>(), input.len())
                .copy_from_slice(&input);
        }
        let before_shared_input = shared_input.clone();
        let mut scratch = vec![0_u64; SCRATCH_BYTES.div_ceil(size_of::<u64>())];
        let mut output = vec![0_u64; 6 * 4096];

        // SAFETY: output_len 刻意指向 input；入口必须在构造 input slice 前拒绝别名，且不得经别名写入。
        let input_status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                shared_input.as_ptr().cast(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                SCRATCH_BYTES,
                output.as_mut_ptr(),
                output.len(),
                shared_input.as_mut_ptr(),
            )
        };
        assert_eq!(input_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(shared_input, before_shared_input);

        let before_output = output.clone();
        // SAFETY: output_len 刻意指向 output；入口必须在构造 output slice 前拒绝别名。
        let output_status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                SCRATCH_BYTES,
                output.as_mut_ptr(),
                output.len(),
                output.as_mut_ptr().cast(),
            )
        };
        assert_eq!(output_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output, before_output);
    }

    #[test]
    fn overlapping_scratch_with_input_or_output_len_is_rejected_atomically() {
        let input = valid_input();
        let mut output = vec![0_u64; 6 * 4096];

        let mut shared_input = vec![0_u64; SCRATCH_BYTES.div_ceil(size_of::<u64>())];
        let shared_input_ptr = shared_input.as_mut_ptr().cast::<u8>();
        // SAFETY: shared_input 容量大于 input，只在调用前把有效 input 拷贝进该对齐 buffer。
        unsafe { std::slice::from_raw_parts_mut(shared_input_ptr, input.len()) }
            .copy_from_slice(&input);
        let mut output_len = usize::MAX;
        // SAFETY: 除被测的 input/scratch 重叠外，其余指针与容量都有效；入口应在构造 slice 前拒绝。
        let input_status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                shared_input_ptr,
                input.len(),
                shared_input_ptr,
                SCRATCH_BYTES,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(input_status, MORNLEA_STATUS_SCRATCH);
        assert_eq!(output_len, usize::MAX);

        let mut shared_output_len = vec![usize::MAX; SCRATCH_BYTES.div_ceil(size_of::<usize>())];
        let shared_output_len_ptr = shared_output_len.as_mut_ptr();
        // SAFETY: 除被测的 scratch/output_len 重叠外，其余指针与容量都有效；入口不得经别名写入即拒绝重叠。
        let output_len_status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                shared_output_len.as_mut_ptr().cast(),
                SCRATCH_BYTES,
                output.as_mut_ptr(),
                output.len(),
                shared_output_len_ptr,
            )
        };
        assert_eq!(output_len_status, MORNLEA_STATUS_SCRATCH);
        assert_eq!(shared_output_len, vec![usize::MAX; shared_output_len.len()]);
    }

    use super::{WORLDGEN_CHUNK_OUTPUT_BYTES, mornlea_worldgen_chunk, mornlea_worldgen_probe};
    use crate::worldgen::{
        WORLDGEN_CHUNK_INPUT_BYTES, WORLDGEN_HEADER_BYTES, WORLDGEN_PROBE_RECORD_BYTES,
    };

    /// 构造一个合法的 worldgen header:seed 42、互异材料表 1..=15(末两项
    /// water=14、short_grass=15,layout 3)、恒等 perm(偏移 54 起)。
    fn worldgen_header() -> Vec<u8> {
        let mut bytes = vec![0u8; WORLDGEN_HEADER_BYTES];
        bytes[0..4].copy_from_slice(b"MGW1");
        bytes[4..8].copy_from_slice(&3u32.to_le_bytes());
        bytes[8..16].copy_from_slice(&42i64.to_le_bytes());
        bytes[16..20].copy_from_slice(&(-64i32).to_le_bytes());
        bytes[20..24].copy_from_slice(&320i32.to_le_bytes());
        for (index, id) in (1u16..=15).enumerate() {
            // 材料表刻意避开 0:air=1 便于区分“输出缓冲原样”与“生成的空气”。
            bytes[24 + index * 2..26 + index * 2].copy_from_slice(&id.to_le_bytes());
        }
        for (index, entry) in bytes[54..WORLDGEN_HEADER_BYTES].iter_mut().enumerate() {
            *entry = (index & 255) as u8;
        }
        bytes
    }

    fn worldgen_chunk_input(chunk_x: i32, chunk_z: i32) -> Vec<u8> {
        let mut bytes = worldgen_header();
        bytes.extend_from_slice(&chunk_x.to_le_bytes());
        bytes.extend_from_slice(&chunk_z.to_le_bytes());
        bytes
    }

    fn worldgen_probe_input(records: &[(u32, i32, i32, i32)]) -> Vec<u8> {
        let mut bytes = worldgen_header();
        bytes.extend_from_slice(&(records.len() as u32).to_le_bytes());
        for &(mode, wx, wy, wz) in records {
            bytes.extend_from_slice(&mode.to_le_bytes());
            bytes.extend_from_slice(&wx.to_le_bytes());
            bytes.extend_from_slice(&wy.to_le_bytes());
            bytes.extend_from_slice(&wz.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn worldgen_chunk_is_deterministic_and_wrong_abi_is_rejected() {
        let input = worldgen_chunk_input(0, 0);
        let mut first = vec![0u8; WORLDGEN_CHUNK_OUTPUT_BYTES];
        let mut second = vec![0u8; WORLDGEN_CHUNK_OUTPUT_BYTES];
        // SAFETY: 指针来自有效 Vec,长度与缓冲容量一致。
        let status_first = unsafe {
            mornlea_worldgen_chunk(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                first.as_mut_ptr(),
                first.len(),
            )
        };
        // SAFETY: 同上。
        let status_second = unsafe {
            mornlea_worldgen_chunk(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                second.as_mut_ptr(),
                second.len(),
            )
        };
        assert_eq!(status_first, MORNLEA_STATUS_OK);
        assert_eq!(status_second, MORNLEA_STATUS_OK);
        assert_eq!(first, second);
        // 生成结果必然包含基岩层(材料 5),不可能全零。
        assert!(first.iter().any(|&b| b != 0));
        // 自然短草:草地表面列里 hash 命中者必须写材料 15(short_grass),
        // 证明 FFI 出口真实产出装饰层而不是只改 framing。
        let short_grass_cells = first
            .chunks_exact(2)
            .filter(|c| u16::from_le_bytes([c[0], c[1]]) == 15)
            .count();
        assert!(short_grass_cells > 0, "chunk (0,0) 未出现任何短草");

        // SAFETY: 同上;仅 abi_version 不匹配。
        let status_abi = unsafe {
            mornlea_worldgen_chunk(
                ABI_VERSION + 1,
                input.as_ptr(),
                input.len(),
                second.as_mut_ptr(),
                second.len(),
            )
        };
        assert_eq!(status_abi, MORNLEA_STATUS_ABI_VERSION);
    }

    #[test]
    fn worldgen_chunk_invalid_input_leaves_output_untouched() {
        let mut output = vec![0xAAu8; WORLDGEN_CHUNK_OUTPUT_BYTES];
        let canary = output.clone();

        let mut bad_magic = worldgen_chunk_input(0, 0);
        bad_magic[0] = b'X';
        let mut duplicate_material = worldgen_chunk_input(0, 0);
        // 把 dirt 改成与 stone 相同的 ID,触发材料表互异性校验。
        duplicate_material[26..28].copy_from_slice(&1u16.to_le_bytes());
        // short_grass(第 15 项,偏移 52)与 water 相同:不在 water == air
        // 门控豁免内,必须按材料表漂移拒绝。
        let mut short_grass_alias = worldgen_chunk_input(0, 0);
        short_grass_alias[52..54].copy_from_slice(&14u16.to_le_bytes());
        let mut wrong_min_y = worldgen_chunk_input(0, 0);
        wrong_min_y[16..20].copy_from_slice(&(-32i32).to_le_bytes());
        let truncated = worldgen_chunk_input(0, 0)[..WORLDGEN_CHUNK_INPUT_BYTES - 1].to_vec();

        // 旧 layout 2 的 564 字节 header + chunk 坐标(共 572 字节)必须被
        // 整体拒绝:layout version 是独立于 ABI 版本号的带内混装防线。
        let mut legacy_layout = worldgen_header()[..564].to_vec();
        legacy_layout[4..8].copy_from_slice(&2u32.to_le_bytes());
        legacy_layout.extend_from_slice(&0i32.to_le_bytes());
        legacy_layout.extend_from_slice(&0i32.to_le_bytes());
        assert_eq!(legacy_layout.len(), 572);

        for input in [
            &bad_magic,
            &duplicate_material,
            &short_grass_alias,
            &wrong_min_y,
            &truncated,
            &legacy_layout,
        ] {
            // SAFETY: 指针来自有效 Vec,长度与缓冲容量一致。
            let status = unsafe {
                mornlea_worldgen_chunk(
                    ABI_VERSION,
                    input.as_ptr(),
                    input.len(),
                    output.as_mut_ptr(),
                    output.len(),
                )
            };
            assert_eq!(status, MORNLEA_STATUS_INPUT);
            assert_eq!(output, canary);
        }

        let valid = worldgen_chunk_input(0, 0);
        // SAFETY: 输出缓冲不足,入口应在写入前拒绝。
        let status_short = unsafe {
            mornlea_worldgen_chunk(
                ABI_VERSION,
                valid.as_ptr(),
                valid.len(),
                output.as_mut_ptr(),
                output.len() - 1,
            )
        };
        assert_eq!(status_short, MORNLEA_STATUS_OUTPUT_OVERFLOW);
        assert_eq!(output, canary);
    }

    #[test]
    fn worldgen_probe_matches_chunk_and_rejects_bad_records() {
        let chunk_input = worldgen_chunk_input(0, 0);
        let mut dense = vec![0u8; WORLDGEN_CHUNK_OUTPUT_BYTES];
        // SAFETY: 指针来自有效 Vec,长度与缓冲容量一致。
        let chunk_status = unsafe {
            mornlea_worldgen_chunk(
                ABI_VERSION,
                chunk_input.as_ptr(),
                chunk_input.len(),
                dense.as_mut_ptr(),
                dense.len(),
            )
        };
        assert_eq!(chunk_status, MORNLEA_STATUS_OK);

        // 探测区块内一根整列:mode 2(BaseBlockAt)必须与 dense 输出逐格一致。
        let mut records = Vec::new();
        for y in [-64i32, -20, 0, 64, 90, 319] {
            records.push((2u32, 3i32, y, 5i32));
        }
        records.push((0, 3, 0, 5));
        let probe_input = worldgen_probe_input(&records);
        let mut probe_out = vec![0u8; records.len() * 8];
        // SAFETY: 同上。
        let probe_status = unsafe {
            mornlea_worldgen_probe(
                ABI_VERSION,
                probe_input.as_ptr(),
                probe_input.len(),
                probe_out.as_mut_ptr(),
                probe_out.len(),
            )
        };
        assert_eq!(probe_status, MORNLEA_STATUS_OK);
        for (index, &(_, _, y, _)) in records[..records.len() - 1].iter().enumerate() {
            let block = u16::from_le_bytes([probe_out[index * 8 + 4], probe_out[index * 8 + 5]]);
            let dense_offset = (((y + 64) * 16 * 16 + 5 * 16 + 3) * 2) as usize;
            let expected = u16::from_le_bytes([dense[dense_offset], dense[dense_offset + 1]]);
            assert_eq!(block, expected, "y={y}");
        }
        let height = i32::from_le_bytes(
            probe_out[(records.len() - 1) * 8..(records.len() - 1) * 8 + 4]
                .try_into()
                .unwrap(),
        );
        // seed 42 的 (3,5) 高度必须落在地形振幅范围内。
        assert!((0..200).contains(&height), "height={height}");

        // mode 越界、record_count 与长度不符、输出长度不匹配都必须原样拒绝。
        let mut bad_mode = worldgen_probe_input(&[(3, 0, 0, 0)]);
        let mut out_one = vec![0xBBu8; 8];
        let canary_one = out_one.clone();
        // SAFETY: 同上。
        let status_mode = unsafe {
            mornlea_worldgen_probe(
                ABI_VERSION,
                bad_mode.as_ptr(),
                bad_mode.len(),
                out_one.as_mut_ptr(),
                out_one.len(),
            )
        };
        assert_eq!(status_mode, MORNLEA_STATUS_INPUT);
        assert_eq!(out_one, canary_one);

        bad_mode.truncate(WORLDGEN_HEADER_BYTES + 4 + WORLDGEN_PROBE_RECORD_BYTES - 1);
        // SAFETY: 同上。
        let status_truncated = unsafe {
            mornlea_worldgen_probe(
                ABI_VERSION,
                bad_mode.as_ptr(),
                bad_mode.len(),
                out_one.as_mut_ptr(),
                out_one.len(),
            )
        };
        assert_eq!(status_truncated, MORNLEA_STATUS_INPUT);
        assert_eq!(out_one, canary_one);

        let valid_one = worldgen_probe_input(&[(0, 0, 0, 0)]);
        // SAFETY: 输出缓冲不足,入口应在写入前拒绝。
        let status_short = unsafe {
            mornlea_worldgen_probe(
                ABI_VERSION,
                valid_one.as_ptr(),
                valid_one.len(),
                out_one.as_mut_ptr(),
                out_one.len() - 1,
            )
        };
        assert_eq!(status_short, MORNLEA_STATUS_OUTPUT_OVERFLOW);
        assert_eq!(out_one, canary_one);
    }

    use super::mornlea_tree_blocks;
    use crate::worldgen::{TREE_BLOCKS_INPUT_BYTES, TREE_BLOCKS_MAX_OUTPUT_BYTES};

    /// 构造一条合法的运行时树形几何请求:`MTB1` + layout 1 + seed + 根坐标。
    fn tree_blocks_input(seed: i64, x: i32, y: i32, z: i32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(TREE_BLOCKS_INPUT_BYTES);
        bytes.extend_from_slice(b"MTB1");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&seed.to_le_bytes());
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        bytes.extend_from_slice(&z.to_le_bytes());
        bytes
    }

    #[test]
    fn tree_blocks_is_deterministic_and_rejects_bad_input() {
        let input = tree_blocks_input(42, 7, 64, -9);
        let mut first = vec![0xAAu8; TREE_BLOCKS_MAX_OUTPUT_BYTES];
        let mut second = vec![0xAAu8; TREE_BLOCKS_MAX_OUTPUT_BYTES];
        // SAFETY: 指针来自有效 Vec,长度与缓冲容量一致。
        let status_first = unsafe {
            mornlea_tree_blocks(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                first.as_mut_ptr(),
                first.len(),
            )
        };
        // SAFETY: 同上。
        let status_second = unsafe {
            mornlea_tree_blocks(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                second.as_mut_ptr(),
                second.len(),
            )
        };
        assert_eq!(status_first, MORNLEA_STATUS_OK);
        assert_eq!(status_second, MORNLEA_STATUS_OK);
        assert_eq!(first, second, "同输入必须逐字节一致");
        let count = u32::from_le_bytes(first[0..4].try_into().unwrap()) as usize;
        assert!(count > 0 && count <= 128, "count={count}");
        // 根格自身是第一条记录,恒为树干底原木(编号 17)。
        assert_eq!(&first[4..12], &[0, 0, 0, 0, 17, 0, 0, 0]);
        // 保留字节恒为 0;写入范围之外的尾部保持调用前内容。
        for record in first[4..4 + count * 8].chunks_exact(8) {
            assert_eq!(
                [record[3], record[6], record[7]],
                [0, 0, 0],
                "保留字节必须为 0"
            );
        }
        if 4 + count * 8 < first.len() {
            assert_eq!(
                first[4 + count * 8],
                0xAA,
                "成功路径不得写入记录区之外的字节"
            );
        }

        let canary = vec![0xAAu8; TREE_BLOCKS_MAX_OUTPUT_BYTES];
        let mut untouched = canary.clone();

        // ABI 版本不匹配:输出缓冲原样。
        // SAFETY: 指针来自有效 Vec;仅 abi_version 不匹配。
        let status_abi = unsafe {
            mornlea_tree_blocks(
                ABI_VERSION + 1,
                input.as_ptr(),
                input.len(),
                untouched.as_mut_ptr(),
                untouched.len(),
            )
        };
        assert_eq!(status_abi, MORNLEA_STATUS_ABI_VERSION);
        assert_eq!(untouched, canary);

        // 空输入指针与空输出指针都必须拒绝。
        // SAFETY: 被测的就是空指针路径,入口在解引用前拒绝。
        let status_null_input = unsafe {
            mornlea_tree_blocks(
                ABI_VERSION,
                std::ptr::null(),
                0,
                untouched.as_mut_ptr(),
                untouched.len(),
            )
        };
        assert_eq!(status_null_input, MORNLEA_STATUS_INVALID_ARGUMENT);
        // SAFETY: 同上。
        let status_null_output = unsafe {
            mornlea_tree_blocks(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                std::ptr::null_mut(),
                untouched.len(),
            )
        };
        assert_eq!(status_null_output, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(untouched, canary);

        // 未知 magic、未知 layout、长度不符、根坐标越界都必须原样拒绝。
        let mut bad_magic = tree_blocks_input(42, 7, 64, -9);
        bad_magic[0] = b'X';
        let mut bad_layout = tree_blocks_input(42, 7, 64, -9);
        bad_layout[4..8].copy_from_slice(&2u32.to_le_bytes());
        let truncated = tree_blocks_input(42, 7, 64, -9)[..TREE_BLOCKS_INPUT_BYTES - 1].to_vec();
        let below_world = tree_blocks_input(42, 7, -65, -9);
        let above_world = tree_blocks_input(42, 7, 312, -9);
        let x_neighborhood_wraps = tree_blocks_input(42, i32::MAX, 64, -9);
        for input in [
            &bad_magic,
            &bad_layout,
            &truncated,
            &below_world,
            &above_world,
            &x_neighborhood_wraps,
        ] {
            // SAFETY: 指针来自有效 Vec,长度与缓冲容量一致。
            let status = unsafe {
                mornlea_tree_blocks(
                    ABI_VERSION,
                    input.as_ptr(),
                    input.len(),
                    untouched.as_mut_ptr(),
                    untouched.len(),
                )
            };
            assert_eq!(status, MORNLEA_STATUS_INPUT);
            assert_eq!(untouched, canary);
        }

        // 输出缓冲不足(小于头部、小于几何所需)必须返回显式溢出且不写部分结果。
        let valid = tree_blocks_input(42, 7, 64, -9);
        for capacity in [0usize, 3, 4 + count * 8 - 1] {
            // SAFETY: 输出指针有效,容量由参数控制。
            let status_short = unsafe {
                mornlea_tree_blocks(
                    ABI_VERSION,
                    valid.as_ptr(),
                    valid.len(),
                    untouched.as_mut_ptr(),
                    capacity,
                )
            };
            assert_eq!(
                status_short, MORNLEA_STATUS_OUTPUT_OVERFLOW,
                "capacity={capacity}"
            );
            assert_eq!(untouched, canary);
        }
    }

    use super::{lod_shell_with, mornlea_lod_shell};
    use crate::lod::{LOD_SHELL_QUAD_BYTES, encode_shell, lod_shell, parse_lod_input};

    /// 构造 LOD 壳入口输入:复用 worldgen header(566)+ tile 原点/列数/步长(16)。
    fn lod_shell_input(tile_x: i32, tile_z: i32, columns: u32, step: u32) -> Vec<u8> {
        let mut bytes = worldgen_header();
        bytes.extend_from_slice(&tile_x.to_le_bytes());
        bytes.extend_from_slice(&tile_z.to_le_bytes());
        bytes.extend_from_slice(&columns.to_le_bytes());
        bytes.extend_from_slice(&step.to_le_bytes());
        bytes
    }

    /// 用 lod 模块级 API 计算期望输出(FFI 出口必须与其逐字节一致)。
    fn expected_shell(input: &[u8]) -> Vec<u8> {
        let request = parse_lod_input(input).expect("valid lod input");
        let mut encoded = Vec::new();
        encode_shell(&lod_shell(&request), &mut encoded);
        encoded
    }

    /// 统一透传参数调用 `mornlea_lod_shell` 的测试助手,返回 status。
    unsafe fn call_lod_shell(
        abi_version: u32,
        input: &[u8],
        output: *mut u8,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32 {
        // SAFETY: 指针来自有效分配,容量不超出实际分配范围。
        unsafe {
            mornlea_lod_shell(
                abi_version,
                input.as_ptr(),
                input.len(),
                output,
                output_capacity,
                output_len,
            )
        }
    }

    #[test]
    fn lod_shell_wrong_abi_is_rejected_atomically() {
        let input = lod_shell_input(0, 0, 64, 4);
        let mut output = vec![0xA5_u8; 64];
        let canary = output.clone();
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec;仅 abi_version 不匹配。
        let status = unsafe {
            call_lod_shell(
                ABI_VERSION + 1,
                &input,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_ABI_VERSION);
        assert_eq!(output_len, usize::MAX);
        assert_eq!(output, canary);
    }

    #[test]
    fn lod_shell_invalid_input_matrix_is_atomic() {
        let valid = lod_shell_input(-3, 2, 64, 4);
        let mut bad_magic = valid.clone();
        bad_magic[0] = b'X';
        let mut wrong_columns = valid.clone();
        wrong_columns[WORLDGEN_HEADER_BYTES + 8..WORLDGEN_HEADER_BYTES + 12]
            .copy_from_slice(&63_u32.to_le_bytes());
        let mut wrong_step = valid.clone();
        wrong_step[WORLDGEN_HEADER_BYTES + 12..WORLDGEN_HEADER_BYTES + 16]
            .copy_from_slice(&3_u32.to_le_bytes());
        let mut overflow_tile_x = valid.clone();
        overflow_tile_x[WORLDGEN_HEADER_BYTES..WORLDGEN_HEADER_BYTES + 4]
            .copy_from_slice(&i32::MAX.to_le_bytes());
        let mut overflow_tile_z = valid.clone();
        overflow_tile_z[WORLDGEN_HEADER_BYTES + 4..WORLDGEN_HEADER_BYTES + 8]
            .copy_from_slice(&i32::MIN.to_le_bytes());
        // 极值 tile 邻域:33554431(2²⁵−1)通过 ×64 但边界环 base+64 溢出;
        // −33554432 的 base = i32::MIN,边界环 −step 下溢。两者都必须按
        // INPUT 拒绝,而不是 panic 收敛(status 9)或 release 静默回绕。
        let mut extreme_tile_x = valid.clone();
        extreme_tile_x[WORLDGEN_HEADER_BYTES..WORLDGEN_HEADER_BYTES + 4]
            .copy_from_slice(&33554431_i32.to_le_bytes());
        let mut extreme_tile_z = valid.clone();
        extreme_tile_z[WORLDGEN_HEADER_BYTES + 4..WORLDGEN_HEADER_BYTES + 8]
            .copy_from_slice(&(-33554432_i32).to_le_bytes());
        let mut cases: Vec<(&str, Vec<u8>)> = vec![
            ("short input", valid[..valid.len() - 1].to_vec()),
            ("long input", {
                let mut long = valid.clone();
                long.push(0);
                long
            }),
            ("bad magic", bad_magic),
            ("wrong columns", wrong_columns),
            ("wrong step", wrong_step),
            ("overflow tile_x", overflow_tile_x),
            ("overflow tile_z", overflow_tile_z),
            ("extreme tile_x base+64 overflows", extreme_tile_x),
            ("extreme tile_z base-step underflows", extreme_tile_z),
        ];
        for (name, input) in cases.drain(..) {
            let mut output = vec![0xA5_u8; 64];
            let canary = output.clone();
            let mut output_len = usize::MAX;
            // SAFETY: 指针来自有效 Vec,长度与缓冲容量一致。
            let status = unsafe {
                call_lod_shell(
                    ABI_VERSION,
                    &input,
                    output.as_mut_ptr(),
                    64,
                    &mut output_len,
                )
            };
            assert_eq!(status, MORNLEA_STATUS_INPUT, "{name}");
            assert_eq!(output_len, 0, "{name}");
            assert_eq!(output, canary, "{name}");
        }
    }

    #[test]
    fn lod_shell_null_and_bad_pointer_arguments_are_atomic() {
        let input = lod_shell_input(0, 0, 64, 4);
        let mut output = vec![0xA5_u8; 64];
        let canary = output.clone();
        let mut output_len = usize::MAX;

        // 空输入指针。
        let mut status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                std::ptr::null(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        // 空输出指针。
        status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                std::ptr::null_mut(),
                0,
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        // 地址回绕的输入指针。
        status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                std::ptr::without_provenance(usize::MAX),
                1,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INPUT);
        assert_eq!(output_len, usize::MAX);

        // 空输出长度指针:必须在任何写入前拒绝。
        status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output, canary);

        // 未对齐的输出长度指针:同样必须在写入前拒绝。
        let mut metadata = [usize::MAX; 2];
        status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                metadata.as_mut_ptr().cast::<u8>().add(1).cast(),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(metadata, [usize::MAX; 2]);
        assert_eq!(output, canary);

        // 对齐但地址回绕的输出长度指针。
        let aligned_max = usize::MAX & !(align_of::<usize>() - 1);
        status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                std::ptr::without_provenance_mut(aligned_max),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output, canary);
    }

    #[test]
    fn lod_shell_two_phase_capacity_probe_then_retry_succeeds() {
        let input = lod_shell_input(-3, 2, 64, 4);
        let expected = expected_shell(&input);
        assert!(!expected.is_empty());
        assert_eq!(expected.len() % LOD_SHELL_QUAD_BYTES, 0);
        let needed = expected.len();

        // 第一段:容量 0 的探测调用只报告所需容量,不写输出缓冲。
        let mut probe = vec![0xA5_u8; 8];
        let probe_canary = probe.clone();
        let mut output_len = usize::MAX;
        let mut status =
            unsafe { call_lod_shell(ABI_VERSION, &input, probe.as_mut_ptr(), 0, &mut output_len) };
        assert_eq!(status, MORNLEA_STATUS_OUTPUT_OVERFLOW);
        assert_eq!(output_len, needed);
        assert_eq!(probe, probe_canary);

        // 容量差一字节仍然 overflow,所需容量不变(确定性纯函数)。
        let mut short = vec![0xA5_u8; needed - 1];
        let short_canary = short.clone();
        output_len = usize::MAX;
        status = unsafe {
            call_lod_shell(
                ABI_VERSION,
                &input,
                short.as_mut_ptr(),
                short.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_OUTPUT_OVERFLOW);
        assert_eq!(output_len, needed);
        assert_eq!(short, short_canary);

        // 第二段:按报告容量扩容后重试必须成功,输出与模块编码逐字节一致。
        let mut exact = vec![0_u8; needed];
        output_len = usize::MAX;
        status = unsafe {
            call_lod_shell(
                ABI_VERSION,
                &input,
                exact.as_mut_ptr(),
                exact.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(output_len, needed);
        assert_eq!(exact, expected);

        // 富余容量同样成功:报告写入字节数,多余尾部不被触碰。
        let mut pooled = vec![0xA5_u8; needed + 64];
        let pooled_canary = pooled.clone();
        output_len = usize::MAX;
        status = unsafe {
            call_lod_shell(
                ABI_VERSION,
                &input,
                pooled.as_mut_ptr(),
                pooled.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(output_len, needed);
        assert_eq!(&pooled[..needed], &expected[..]);
        assert_eq!(&pooled[needed..], &pooled_canary[needed..]);
    }

    #[test]
    fn lod_shell_matches_module_encoding_for_all_steps() {
        for step in [2_u32, 4, 8] {
            let input = lod_shell_input(-3, 2, 64, step);
            let expected = expected_shell(&input);
            assert!(!expected.is_empty(), "step={step}");
            let mut output = vec![0_u8; expected.len()];
            let mut output_len = usize::MAX;
            let status = unsafe {
                call_lod_shell(
                    ABI_VERSION,
                    &input,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut output_len,
                )
            };
            assert_eq!(status, MORNLEA_STATUS_OK, "step={step}");
            assert_eq!(output_len, expected.len(), "step={step}");
            assert_eq!(output, expected, "step={step}");

            // 同输入两次调用逐字节一致(确定性契约)。
            let mut second = vec![0_u8; expected.len()];
            let mut second_len = usize::MAX;
            let second_status = unsafe {
                call_lod_shell(
                    ABI_VERSION,
                    &input,
                    second.as_mut_ptr(),
                    second.len(),
                    &mut second_len,
                )
            };
            assert_eq!(second_status, MORNLEA_STATUS_OK, "step={step}");
            assert_eq!(second, output, "step={step}");
        }
    }

    #[test]
    fn lod_shell_panic_is_contained_without_output() {
        let input = lod_shell_input(0, 0, 64, 4);
        let mut output = vec![0xA5_u8; 64];
        let canary = output.clone();
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec;generator 注入 panic 验证收敛为 status 9。
        let status = unsafe {
            lod_shell_with(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
                |_| panic!("测试 panic"),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_PANIC);
        assert_eq!(output_len, 0);
        assert_eq!(output, canary);
    }

    #[test]
    fn lod_shell_overlapping_buffers_are_rejected_atomically() {
        // input/output 别名。
        let input = lod_shell_input(0, 0, 64, 4);
        let mut shared = input.clone();
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec,刻意把输出指向输入缓冲以验证别名拒绝。
        let status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                shared.as_ptr(),
                shared.len(),
                shared.as_mut_ptr(),
                shared.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);
        assert_eq!(shared, input);

        // output_len 与 input 别名:入口不得经别名写入即拒绝。
        let mut shared_input = vec![0_usize; input.len().div_ceil(size_of::<usize>())];
        // SAFETY: shared_input 容量覆盖完整 encoded input,先写入合法输入。
        unsafe {
            std::slice::from_raw_parts_mut(shared_input.as_mut_ptr().cast::<u8>(), input.len())
                .copy_from_slice(&input);
        }
        let before_shared_input = shared_input.clone();
        let mut output = vec![0xA5_u8; 64];
        let output_canary = output.clone();
        // SAFETY: output_len 刻意指向 input 缓冲,验证别名拒绝。
        let input_status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                shared_input.as_ptr().cast(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                shared_input.as_mut_ptr(),
            )
        };
        assert_eq!(input_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(shared_input, before_shared_input);
        assert_eq!(output, output_canary);

        // output_len 与 output 别名:入口不得经别名写入即拒绝。
        let mut shared_output = vec![0xA5_usize; 16];
        let before = shared_output.clone();
        // SAFETY: output 与 output_len 刻意指向同一缓冲,验证别名拒绝。
        let output_status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                shared_output.as_mut_ptr().cast(),
                shared_output.len() * size_of::<usize>(),
                shared_output.as_mut_ptr(),
            )
        };
        assert_eq!(output_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(shared_output, before);
    }

    use super::{fluid_eval_batch_with, mornlea_fluid_eval_batch};
    use crate::fluid_eval::{
        EVAL_ITEM_OUTPUT_BYTES, EVAL_SLOTS_PER_ITEM, encode_eval_input, eval_one,
    };

    /// 2 项标准输入:项 0 = 源格下方空气(垂直优先 1 条),项 1 = 等级 7
    /// 靠上方源保活、下方与水平邻居均不可写(空写)。
    fn fluid_eval_two_items() -> Vec<[u16; EVAL_SLOTS_PER_ITEM]> {
        vec![[27, 2, 0, 2, 2, 2, 2], [34, 27, 2, 0, 0, 0, 0]]
    }

    /// 用模块级 API 计算期望输出(FFI 出口必须与其逐字节一致)。
    fn expected_eval_output(items: &[[u16; EVAL_SLOTS_PER_ITEM]]) -> Vec<u8> {
        let mut encoded = vec![0xFF_u8; items.len() * EVAL_ITEM_OUTPUT_BYTES];
        for (chunk, item) in encoded
            .chunks_exact_mut(EVAL_ITEM_OUTPUT_BYTES)
            .zip(items.iter())
        {
            let slot: &mut [u8; EVAL_ITEM_OUTPUT_BYTES] =
                chunk.try_into().expect("exact-size chunk");
            eval_one(item, slot);
        }
        encoded
    }

    /// 统一透传参数调用 `mornlea_fluid_eval_batch` 的测试助手,返回 status。
    unsafe fn call_fluid_eval(
        abi_version: u32,
        input: &[u8],
        output: *mut u8,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32 {
        // SAFETY: 指针来自有效分配,容量不超出实际分配范围。
        unsafe {
            mornlea_fluid_eval_batch(
                abi_version,
                input.as_ptr(),
                input.len(),
                output,
                output_capacity,
                output_len,
            )
        }
    }

    #[test]
    fn fluid_eval_wrong_abi_is_rejected_atomically() {
        let input = encode_eval_input(&fluid_eval_two_items());
        let mut output = vec![0xA5_u8; 24];
        let canary = output.clone();
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec;仅 abi_version 不匹配。
        let status = unsafe {
            call_fluid_eval(
                ABI_VERSION + 1,
                &input,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_ABI_VERSION);
        assert_eq!(output_len, usize::MAX);
        assert_eq!(output, canary);
    }

    #[test]
    fn fluid_eval_invalid_input_matrix_is_atomic() {
        let items = fluid_eval_two_items();
        let valid = encode_eval_input(&items);
        let mut wrong_layout = valid.clone();
        wrong_layout[0..4].copy_from_slice(&2_u32.to_le_bytes());
        let mut wrong_count = valid.clone();
        wrong_count[4..8].copy_from_slice(&3_u32.to_le_bytes());
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("short header", valid[..7].to_vec()),
            ("short input", valid[..valid.len() - 1].to_vec()),
            ("long input", {
                let mut long = valid.clone();
                long.push(0);
                long
            }),
            ("wrong layout version", wrong_layout),
            ("item count mismatch", wrong_count),
        ];
        for (name, input) in cases {
            let mut output = vec![0xA5_u8; 24];
            let canary = output.clone();
            let mut output_len = usize::MAX;
            // SAFETY: 指针来自有效 Vec,长度与缓冲容量一致。
            let status = unsafe {
                call_fluid_eval(
                    ABI_VERSION,
                    &input,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut output_len,
                )
            };
            assert_eq!(status, MORNLEA_STATUS_INPUT, "{name}");
            assert_eq!(output_len, 0, "{name}");
            assert_eq!(output, canary, "{name}");
        }
    }

    #[test]
    fn fluid_eval_short_capacity_is_invalid_argument_not_overflow() {
        // 输出尺寸是输入的确定函数(2 项 × 12 = 24 字节),容量不足按参数
        // 违约拒绝而非 lod 式两段探测,也不写入输出缓冲。
        let input = encode_eval_input(&fluid_eval_two_items());
        for capacity in [0_usize, 12, 23] {
            let mut output = vec![0xA5_u8; 24];
            let canary = output.clone();
            let mut output_len = usize::MAX;
            // SAFETY: 指针来自有效 Vec;仅容量参数不足。
            let status = unsafe {
                call_fluid_eval(
                    ABI_VERSION,
                    &input,
                    output.as_mut_ptr(),
                    capacity,
                    &mut output_len,
                )
            };
            assert_eq!(
                status, MORNLEA_STATUS_INVALID_ARGUMENT,
                "capacity={capacity}"
            );
            assert_eq!(output_len, 0, "capacity={capacity}");
            assert_eq!(output, canary, "capacity={capacity}");
        }
    }

    #[test]
    fn fluid_eval_success_matches_module_encoding() {
        let items = fluid_eval_two_items();
        let input = encode_eval_input(&items);
        let expected = expected_eval_output(&items);
        assert_eq!(expected.len(), 24);

        // 恰好容量成功,字节与模块级求值逐位一致。
        let mut output = vec![0_u8; 24];
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec,长度与容量一致。
        let status = unsafe {
            call_fluid_eval(
                ABI_VERSION,
                &input,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(output_len, 24);
        assert_eq!(output, expected);

        // 确定性契约:同输入两次调用逐字节一致。
        let mut second = vec![0_u8; 24];
        let mut second_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec,长度与容量一致。
        let second_status = unsafe {
            call_fluid_eval(
                ABI_VERSION,
                &input,
                second.as_mut_ptr(),
                second.len(),
                &mut second_len,
            )
        };
        assert_eq!(second_status, MORNLEA_STATUS_OK);
        assert_eq!(second, output);

        // 富余容量成功:只写前 24 字节,尾部不被触碰。
        let mut padded = vec![0xA5_u8; 24 + 8];
        let padded_canary = padded.clone();
        let mut padded_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec,容量覆盖写入区。
        let padded_status = unsafe {
            call_fluid_eval(
                ABI_VERSION,
                &input,
                padded.as_mut_ptr(),
                padded.len(),
                &mut padded_len,
            )
        };
        assert_eq!(padded_status, MORNLEA_STATUS_OK);
        assert_eq!(padded_len, 24);
        assert_eq!(&padded[..24], &expected[..]);
        assert_eq!(&padded[24..], &padded_canary[24..]);
    }

    #[test]
    fn fluid_eval_panic_is_contained_without_output() {
        let input = encode_eval_input(&fluid_eval_two_items());
        let mut output = vec![0xA5_u8; 24];
        let canary = output.clone();
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec;evaluator 注入 panic 验证收敛为 status 9。
        let status = unsafe {
            fluid_eval_batch_with(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
                |_, _| panic!("测试 panic"),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_PANIC);
        assert_eq!(output_len, 0);
        assert_eq!(output, canary);
    }

    #[test]
    fn fluid_eval_null_and_bad_pointer_arguments_are_atomic() {
        let input = encode_eval_input(&fluid_eval_two_items());
        let mut output = vec![0xA5_u8; 24];
        let canary = output.clone();
        let mut output_len = usize::MAX;

        // 空输入指针。
        // SAFETY: 其余指针来自有效 Vec;仅验证空输入指针的拒绝路径。
        let mut status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                std::ptr::null(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        // 空输出指针。
        // SAFETY: 输入指针来自有效 Vec;仅验证空输出指针的拒绝路径。
        status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                std::ptr::null_mut(),
                0,
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        // 地址回绕的输入指针。
        // SAFETY: 其余指针来自有效 Vec;仅验证回绕地址的拒绝路径。
        status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                std::ptr::without_provenance(usize::MAX),
                1,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INPUT);
        assert_eq!(output_len, usize::MAX);

        // 空输出长度指针:必须在任何写入前拒绝。
        // SAFETY: 指针来自有效 Vec;仅验证空 metadata 指针的拒绝路径。
        status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output, canary);

        // 未对齐的输出长度指针:同样必须在写入前拒绝。
        let mut metadata = [usize::MAX; 2];
        // SAFETY: 刻意偏移一字节构造未对齐 metadata 指针。
        status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                metadata.as_mut_ptr().cast::<u8>().add(1).cast(),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(metadata, [usize::MAX; 2]);
        assert_eq!(output, canary);

        // 对齐但地址回绕的输出长度指针。
        let aligned_max = usize::MAX & !(align_of::<usize>() - 1);
        // SAFETY: 其余指针来自有效 Vec;仅验证回绕 metadata 地址的拒绝路径。
        status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                std::ptr::without_provenance_mut(aligned_max),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output, canary);
    }

    #[test]
    fn fluid_eval_overlapping_buffers_are_rejected_atomically() {
        // input/output 别名。
        let input = encode_eval_input(&fluid_eval_two_items());
        let mut shared = input.clone();
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec,刻意把输出指向输入缓冲以验证别名拒绝。
        let status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                shared.as_ptr(),
                shared.len(),
                shared.as_mut_ptr(),
                shared.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);
        assert_eq!(shared, input);

        // output_len 与 input 别名:入口不得经别名写入即拒绝。
        let mut shared_input = vec![0_usize; input.len().div_ceil(size_of::<usize>())];
        // SAFETY: shared_input 容量覆盖完整 encoded input,先写入合法输入。
        unsafe {
            std::slice::from_raw_parts_mut(shared_input.as_mut_ptr().cast::<u8>(), input.len())
                .copy_from_slice(&input);
        }
        let before_shared_input = shared_input.clone();
        let mut output = vec![0xA5_u8; 24];
        let output_canary = output.clone();
        // SAFETY: output_len 刻意指向 input 缓冲,验证别名拒绝。
        let input_status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                shared_input.as_ptr().cast(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                shared_input.as_mut_ptr(),
            )
        };
        assert_eq!(input_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(shared_input, before_shared_input);
        assert_eq!(output, output_canary);

        // output_len 与 output 别名:入口不得经别名写入即拒绝。
        let mut shared_output = vec![0xA5_usize; 4];
        let before = shared_output.clone();
        // SAFETY: output 与 output_len 刻意指向同一缓冲,验证别名拒绝。
        let output_status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                shared_output.as_mut_ptr().cast(),
                shared_output.len() * size_of::<usize>(),
                shared_output.as_mut_ptr(),
            )
        };
        assert_eq!(output_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(shared_output, before);
    }

    use super::{fluid_rescan_with, mornlea_fluid_rescan};
    use crate::fluid_rescan::test_support::{RescanBox, STONE, decode_positions, decode_summary};
    use crate::fluid_rescan::{fluid_rescan as module_scan, parse_rescan_input};

    /// 标准测试盒:中心 (−2,3),全区块列扫描;段 2 密集含一个流动格与
    /// 一个下方空气的源格,段 5 均匀水源且区段级不动点成立(计 1)。
    /// 期望产出 [(-29,-31,52), (-27,-30,54)],spent = 4119。
    fn fluid_rescan_test_box() -> RescanBox {
        let mut box_ = RescanBox::new(-2, 3);
        box_.dense_section(2, |_, _, _| STONE);
        box_.set_center_cell(2, 3, 1, 4, crate::fluid_eval::WATER_SOURCE + 2);
        box_.set_center_cell(2, 5, 2, 6, crate::fluid_eval::WATER_SOURCE);
        box_.set_center_cell(2, 5, 1, 6, crate::fluid_eval::AIR);
        box_.uniform_section(5, crate::fluid_eval::WATER_SOURCE);
        box_
    }

    /// 统一透传参数调用 `mornlea_fluid_rescan` 的测试助手,返回 status。
    unsafe fn call_fluid_rescan(
        abi_version: u32,
        input: &[u8],
        output: *mut u8,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32 {
        // SAFETY: 指针来自有效分配,容量不超出实际分配范围。
        unsafe {
            mornlea_fluid_rescan(
                abi_version,
                input.as_ptr(),
                input.len(),
                output,
                output_capacity,
                output_len,
            )
        }
    }

    #[test]
    fn fluid_rescan_success_matches_module_scan() {
        let box_ = fluid_rescan_test_box();
        let input = box_.build();
        let view = parse_rescan_input(&input).expect("测试盒必须合法");
        let expected = module_scan(&view);
        // 2 条坐标 + summary。
        assert_eq!(expected.len(), 2 * 12 + 8);
        assert_eq!(
            decode_positions(&expected),
            vec![(-29, -31, 52), (-27, -30, 54)]
        );
        assert_eq!(decode_summary(&expected), (4119, true));

        // 恰好容量成功,字节与模块级扫描逐位一致。
        let mut output = vec![0_u8; expected.len()];
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec,长度与容量一致。
        let status = unsafe {
            call_fluid_rescan(
                ABI_VERSION,
                &input,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(output_len, expected.len());
        assert_eq!(output, expected);

        // 确定性契约:同输入两次调用逐字节一致。
        let mut second = vec![0_u8; expected.len()];
        let mut second_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec,长度与容量一致。
        let second_status = unsafe {
            call_fluid_rescan(
                ABI_VERSION,
                &input,
                second.as_mut_ptr(),
                second.len(),
                &mut second_len,
            )
        };
        assert_eq!(second_status, MORNLEA_STATUS_OK);
        assert_eq!(second, output);

        // 富余容量成功:只写实际输出,尾部不被触碰。
        let mut padded = vec![0xA5_u8; expected.len() + 8];
        let padded_canary = padded.clone();
        let mut padded_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec,容量覆盖写入区。
        let padded_status = unsafe {
            call_fluid_rescan(
                ABI_VERSION,
                &input,
                padded.as_mut_ptr(),
                padded.len(),
                &mut padded_len,
            )
        };
        assert_eq!(padded_status, MORNLEA_STATUS_OK);
        assert_eq!(padded_len, expected.len());
        assert_eq!(&padded[..expected.len()], &expected[..]);
        assert_eq!(&padded[expected.len()..], &padded_canary[expected.len()..]);
    }

    #[test]
    fn fluid_rescan_two_phase_overflow_is_exact_and_atomic() {
        let box_ = fluid_rescan_test_box();
        let input = box_.build();
        let view = parse_rescan_input(&input).expect("测试盒必须合法");
        let needed = module_scan(&view).len();

        // 第一段:容量 1 不足,报告所需字节数且不写输出缓冲。
        let mut tiny = vec![0xA5_u8; 1];
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec;仅容量参数不足。
        let status = unsafe {
            call_fluid_rescan(
                ABI_VERSION,
                &input,
                tiny.as_mut_ptr(),
                tiny.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_OUTPUT_OVERFLOW);
        assert_eq!(output_len, needed);
        assert_eq!(tiny, vec![0xA5_u8; 1]);

        // 差 1 字节仍是 overflow,所需字节数保持不变。
        let mut short = vec![0xA5_u8; needed - 1];
        // SAFETY: 指针来自有效 Vec;容量差 1。
        let status = unsafe {
            call_fluid_rescan(
                ABI_VERSION,
                &input,
                short.as_mut_ptr(),
                short.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_OUTPUT_OVERFLOW);
        assert_eq!(output_len, needed);
        assert_eq!(short, vec![0xA5_u8; needed - 1]);

        // 恰好容量即成功,写入字节数 == 所需。
        let mut exact = vec![0_u8; needed];
        // SAFETY: 指针来自有效 Vec,长度与容量一致。
        let status = unsafe {
            call_fluid_rescan(
                ABI_VERSION,
                &input,
                exact.as_mut_ptr(),
                exact.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_OK);
        assert_eq!(output_len, needed);
        assert_eq!(exact, module_scan(&view));
    }

    #[test]
    fn fluid_rescan_invalid_input_matrix_is_atomic() {
        let valid = fluid_rescan_test_box().build();
        let mut wrong_layout = valid.clone();
        wrong_layout[0..4].copy_from_slice(&2_u32.to_le_bytes());
        let mut bad_start = valid.clone();
        bad_start[20] = 24;
        let mut reserved = valid.clone();
        reserved[21] = 1;
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("short header", valid[..25].to_vec()),
            ("short input", valid[..valid.len() - 1].to_vec()),
            ("long input", {
                let mut long = valid.clone();
                long.push(0);
                long
            }),
            ("wrong layout version", wrong_layout),
            ("start section out of range", bad_start),
            ("reserved not zero", reserved),
        ];
        for (name, input) in cases {
            let mut output = vec![0xA5_u8; 64];
            let canary = output.clone();
            let mut output_len = usize::MAX;
            // SAFETY: 指针来自有效 Vec,长度与缓冲容量一致。
            let status = unsafe {
                call_fluid_rescan(
                    ABI_VERSION,
                    &input,
                    output.as_mut_ptr(),
                    output.len(),
                    &mut output_len,
                )
            };
            assert_eq!(status, MORNLEA_STATUS_INPUT, "{name}");
            assert_eq!(output_len, 0, "{name}");
            assert_eq!(output, canary, "{name}");
        }

        // ABI 版本不匹配。
        let mut output = vec![0xA5_u8; 64];
        let canary = output.clone();
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec;仅 abi_version 不匹配。
        let status = unsafe {
            call_fluid_rescan(
                ABI_VERSION + 1,
                &valid,
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_ABI_VERSION);
        assert_eq!(output_len, usize::MAX);
        assert_eq!(output, canary);

        // input/output 别名。
        let mut shared = valid.clone();
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec,刻意把输出指向输入缓冲以验证别名拒绝。
        let status = unsafe {
            mornlea_fluid_rescan(
                ABI_VERSION,
                shared.as_ptr(),
                shared.len(),
                shared.as_mut_ptr(),
                shared.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);
        assert_eq!(shared, valid);
    }

    #[test]
    fn fluid_rescan_pointer_arguments_are_atomic() {
        let input = fluid_rescan_test_box().build();
        let mut output = vec![0xA5_u8; 64];
        let canary = output.clone();
        let mut output_len = usize::MAX;

        // 空输入指针。
        // SAFETY: 其余指针来自有效 Vec;仅验证空输入指针的拒绝路径。
        let mut status = unsafe {
            mornlea_fluid_rescan(
                ABI_VERSION,
                std::ptr::null(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        // 空输出指针。
        // SAFETY: 输入指针来自有效 Vec;仅验证空输出指针的拒绝路径。
        status = unsafe {
            mornlea_fluid_rescan(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                std::ptr::null_mut(),
                0,
                &mut output_len,
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output_len, usize::MAX);

        // 空输出长度指针:必须在任何写入前拒绝。
        // SAFETY: 指针来自有效 Vec;仅验证空 metadata 指针的拒绝路径。
        status = unsafe {
            mornlea_fluid_rescan(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(output, canary);
    }

    #[test]
    fn fluid_rescan_panic_is_contained_without_output() {
        let input = fluid_rescan_test_box().build();
        let mut output = vec![0xA5_u8; 64];
        let canary = output.clone();
        let mut output_len = usize::MAX;
        // SAFETY: 指针来自有效 Vec;scanner 注入 panic 验证收敛为 status 9。
        let status = unsafe {
            fluid_rescan_with(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut output_len,
                |_| panic!("测试 panic"),
            )
        };
        assert_eq!(status, MORNLEA_STATUS_PANIC);
        assert_eq!(output_len, 0);
        assert_eq!(output, canary);
    }

    #[test]
    fn metadata_alias_mesh() {
        // The metadata word shares storage with the input arena: preflight
        // must reject the overlap with the overlap status and must not store
        // through the alias, so every canary byte survives.
        let mut scratch = vec![0_u64; SCRATCH_BYTES / size_of::<u64>()];
        let mut output = vec![0_u64; 6 * 4096];
        let mut input_arena = vec![0xa5a5_a5a5_usize; 8];
        let before_input = input_arena.clone();
        // SAFETY: all pointers are aligned and in bounds; the metadata word
        // deliberately sits at the start of the input range.
        let input_status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input_arena.as_ptr().cast(),
                input_arena.len() * size_of::<usize>(),
                scratch.as_mut_ptr().cast(),
                SCRATCH_BYTES,
                output.as_mut_ptr(),
                output.len(),
                input_arena.as_mut_ptr(),
            )
        };
        assert_eq!(input_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(input_arena, before_input);

        // The metadata word shares storage with the output arena.
        let input = valid_input();
        let mut shared_output = vec![0xa5a5_a5a5_u64; 6 * 4096];
        let before_output = shared_output.clone();
        // SAFETY: all pointers are aligned and in bounds; the metadata word
        // deliberately sits at the start of the output range.
        let output_status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                scratch.as_mut_ptr().cast(),
                SCRATCH_BYTES,
                shared_output.as_mut_ptr(),
                shared_output.len(),
                shared_output.as_mut_ptr().cast(),
            )
        };
        assert_eq!(output_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(shared_output, before_output);

        // The metadata word shares storage with the scratch arena.
        let mut shared_scratch = vec![0xa5a5_a5a5_u64; SCRATCH_BYTES / size_of::<u64>()];
        let before_scratch = shared_scratch.clone();
        // SAFETY: all pointers are aligned and in bounds; the metadata word
        // deliberately sits at the start of the scratch range.
        let scratch_status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                shared_scratch.as_mut_ptr().cast(),
                SCRATCH_BYTES,
                output.as_mut_ptr(),
                output.len(),
                shared_scratch.as_mut_ptr().cast(),
            )
        };
        assert_eq!(scratch_status, MORNLEA_STATUS_SCRATCH);
        assert_eq!(shared_scratch, before_scratch);

        // A valid non-aliased metadata pointer on a semantically invalid
        // request is cleared to zero while the payload stays untouched.
        let short = input[..input.len() - 1].to_vec();
        let mut payload = vec![0xa5a5_a5a5_u64; 6 * 4096];
        let payload_canary = payload.clone();
        let mut output_len = usize::MAX;
        // SAFETY: every pointer is valid and aligned; only the input length
        // is one byte short so parsing fails after the metadata clear.
        let invalid_status = unsafe {
            mornlea_mesh_section(
                ABI_VERSION,
                short.as_ptr(),
                short.len(),
                scratch.as_mut_ptr().cast(),
                SCRATCH_BYTES,
                payload.as_mut_ptr(),
                payload.len(),
                &mut output_len,
            )
        };
        assert_eq!(invalid_status, MORNLEA_STATUS_INPUT);
        assert_eq!(output_len, 0);
        assert_eq!(payload, payload_canary);
    }

    #[test]
    fn metadata_alias_lod() {
        // The metadata word shares storage with the input arena: preflight
        // must reject the overlap with the overlap status and must not store
        // through the alias, so every canary byte survives.
        let mut output = vec![0xa5_u8; 64];
        let output_canary = output.clone();
        let mut input_arena = vec![0xa5a5_a5a5_usize; 8];
        let before_input = input_arena.clone();
        // SAFETY: all pointers are aligned and in bounds; the metadata word
        // deliberately sits at the start of the input range.
        let input_status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                input_arena.as_ptr().cast(),
                input_arena.len() * size_of::<usize>(),
                output.as_mut_ptr(),
                output.len(),
                input_arena.as_mut_ptr(),
            )
        };
        assert_eq!(input_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(input_arena, before_input);
        assert_eq!(output, output_canary);

        // The metadata word shares storage with the output arena.
        let input = lod_shell_input(0, 0, 64, 4);
        let mut shared_output = vec![0xa5a5_a5a5_usize; 8];
        let before_output = shared_output.clone();
        // SAFETY: all pointers are aligned and in bounds; the metadata word
        // deliberately sits at the start of the output range.
        let output_status = unsafe {
            mornlea_lod_shell(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                shared_output.as_mut_ptr().cast(),
                shared_output.len() * size_of::<usize>(),
                shared_output.as_mut_ptr(),
            )
        };
        assert_eq!(output_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(shared_output, before_output);

        // A valid non-aliased metadata pointer on a semantically invalid
        // request is cleared to zero while the payload stays untouched.
        let mut bad_magic = input.clone();
        bad_magic[0] = b'X';
        let mut payload = vec![0xa5_u8; 64];
        let payload_canary = payload.clone();
        let mut output_len = usize::MAX;
        // SAFETY: every pointer is valid and aligned; only the magic is
        // wrong so parsing fails after the metadata clear.
        let invalid_status = unsafe {
            call_lod_shell(
                ABI_VERSION,
                &bad_magic,
                payload.as_mut_ptr(),
                payload.len(),
                &mut output_len,
            )
        };
        assert_eq!(invalid_status, MORNLEA_STATUS_INPUT);
        assert_eq!(output_len, 0);
        assert_eq!(payload, payload_canary);

        // A valid non-aliased metadata pointer on a capacity probe publishes
        // the exact needed count while the payload stays untouched.
        let probe_input = lod_shell_input(-3, 2, 64, 4);
        let needed = expected_shell(&probe_input).len();
        assert!(needed > 0);
        let mut probe = vec![0xa5_u8; 8];
        let probe_canary = probe.clone();
        let mut probe_len = usize::MAX;
        // SAFETY: every pointer is valid and aligned; only the capacity is
        // zero so the two-phase probe reports the needed count.
        let probe_status = unsafe {
            call_lod_shell(
                ABI_VERSION,
                &probe_input,
                probe.as_mut_ptr(),
                0,
                &mut probe_len,
            )
        };
        assert_eq!(probe_status, MORNLEA_STATUS_OUTPUT_OVERFLOW);
        assert_eq!(probe_len, needed);
        assert_eq!(probe, probe_canary);
    }

    #[test]
    fn metadata_alias_fluid_eval() {
        // The metadata word shares storage with the input arena: preflight
        // must reject the overlap with the overlap status and must not store
        // through the alias, so every canary byte survives.
        let mut output = vec![0xa5_u8; 24];
        let output_canary = output.clone();
        let mut input_arena = vec![0xa5a5_a5a5_usize; 8];
        let before_input = input_arena.clone();
        // SAFETY: all pointers are aligned and in bounds; the metadata word
        // deliberately sits at the start of the input range.
        let input_status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                input_arena.as_ptr().cast(),
                input_arena.len() * size_of::<usize>(),
                output.as_mut_ptr(),
                output.len(),
                input_arena.as_mut_ptr(),
            )
        };
        assert_eq!(input_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(input_arena, before_input);
        assert_eq!(output, output_canary);

        // The metadata word shares storage with the output arena.
        let input = encode_eval_input(&fluid_eval_two_items());
        let mut shared_output = vec![0xa5a5_a5a5_usize; 3];
        let before_output = shared_output.clone();
        // SAFETY: all pointers are aligned and in bounds; the metadata word
        // deliberately sits at the start of the output range.
        let output_status = unsafe {
            mornlea_fluid_eval_batch(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                shared_output.as_mut_ptr().cast(),
                shared_output.len() * size_of::<usize>(),
                shared_output.as_mut_ptr(),
            )
        };
        assert_eq!(output_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(shared_output, before_output);

        // A valid non-aliased metadata pointer on a semantically invalid
        // request is cleared to zero while the payload stays untouched.
        let mut wrong_layout = input.clone();
        wrong_layout[0..4].copy_from_slice(&2_u32.to_le_bytes());
        let mut payload = vec![0xa5_u8; 24];
        let payload_canary = payload.clone();
        let mut output_len = usize::MAX;
        // SAFETY: every pointer is valid and aligned; only the layout
        // version is wrong so parsing fails after the metadata clear.
        let invalid_status = unsafe {
            call_fluid_eval(
                ABI_VERSION,
                &wrong_layout,
                payload.as_mut_ptr(),
                payload.len(),
                &mut output_len,
            )
        };
        assert_eq!(invalid_status, MORNLEA_STATUS_INPUT);
        assert_eq!(output_len, 0);
        assert_eq!(payload, payload_canary);
    }

    #[test]
    fn metadata_alias_fluid_rescan() {
        // The metadata word shares storage with the input arena: preflight
        // must reject the overlap with the overlap status and must not store
        // through the alias, so every canary byte survives.
        let mut output = vec![0xa5_u8; 64];
        let output_canary = output.clone();
        let mut input_arena = vec![0xa5a5_a5a5_usize; 8];
        let before_input = input_arena.clone();
        // SAFETY: all pointers are aligned and in bounds; the metadata word
        // deliberately sits at the start of the input range.
        let input_status = unsafe {
            mornlea_fluid_rescan(
                ABI_VERSION,
                input_arena.as_ptr().cast(),
                input_arena.len() * size_of::<usize>(),
                output.as_mut_ptr(),
                output.len(),
                input_arena.as_mut_ptr(),
            )
        };
        assert_eq!(input_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(input_arena, before_input);
        assert_eq!(output, output_canary);

        // The metadata word shares storage with the output arena.
        let input = fluid_rescan_test_box().build();
        let mut shared_output = vec![0xa5a5_a5a5_usize; 8];
        let before_output = shared_output.clone();
        // SAFETY: all pointers are aligned and in bounds; the metadata word
        // deliberately sits at the start of the output range.
        let output_status = unsafe {
            mornlea_fluid_rescan(
                ABI_VERSION,
                input.as_ptr(),
                input.len(),
                shared_output.as_mut_ptr().cast(),
                shared_output.len() * size_of::<usize>(),
                shared_output.as_mut_ptr(),
            )
        };
        assert_eq!(output_status, MORNLEA_STATUS_INVALID_ARGUMENT);
        assert_eq!(shared_output, before_output);

        // A valid non-aliased metadata pointer on a semantically invalid
        // request is cleared to zero while the payload stays untouched.
        let mut wrong_layout = input.clone();
        wrong_layout[0..4].copy_from_slice(&2_u32.to_le_bytes());
        let mut payload = vec![0xa5_u8; 64];
        let payload_canary = payload.clone();
        let mut output_len = usize::MAX;
        // SAFETY: every pointer is valid and aligned; only the layout
        // version is wrong so parsing fails after the metadata clear.
        let invalid_status = unsafe {
            call_fluid_rescan(
                ABI_VERSION,
                &wrong_layout,
                payload.as_mut_ptr(),
                payload.len(),
                &mut output_len,
            )
        };
        assert_eq!(invalid_status, MORNLEA_STATUS_INPUT);
        assert_eq!(output_len, 0);
        assert_eq!(payload, payload_canary);

        // A valid non-aliased metadata pointer on a capacity probe publishes
        // the exact needed count while the payload stays untouched.
        let view = parse_rescan_input(&input).expect("test box must parse");
        let needed = module_scan(&view).len();
        assert!(needed > 0);
        let mut probe = vec![0xa5_u8; 1];
        let mut probe_len = usize::MAX;
        // SAFETY: every pointer is valid and aligned; only the capacity is
        // short so the two-phase probe reports the needed count.
        let probe_status = unsafe {
            call_fluid_rescan(
                ABI_VERSION,
                &input,
                probe.as_mut_ptr(),
                probe.len(),
                &mut probe_len,
            )
        };
        assert_eq!(probe_status, MORNLEA_STATUS_OUTPUT_OVERFLOW);
        assert_eq!(probe_len, needed);
        assert_eq!(probe, vec![0xa5_u8; 1]);
    }
}
