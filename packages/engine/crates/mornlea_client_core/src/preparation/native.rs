//! Reusable bounded native CPU scratch; output never succeeds with placeholder geometry.

use super::{PreparationPayload, PreparedGeometry};
use crate::ClientError;
use mornlea_engine::native::contracts::{
    KernelError, LodOp, LodQuad, LodScratch, MeshOp, MeshQuad, MeshScratch,
};
use mornlea_engine::native::{lod::NativeLod, mesh::NativeMesh};

#[derive(Default)]
pub(super) struct NativePreparation {
    near: Option<MeshScratch>,
    far: Option<LodScratch>,
}

const NEAR_SCRATCH_BYTES: usize = 110592 * 5 + 40960 * std::mem::size_of::<MeshQuad>();
const FAR_SCRATCH_BYTES: usize =
    1156 * std::mem::size_of::<[i32; 2]>() + 3136 * std::mem::size_of::<LodQuad>();

fn kernel_error(error: KernelError) -> ClientError {
    match error {
        KernelError::Allocation
        | KernelError::OutputTooSmall { .. }
        | KernelError::ScratchTooSmall { .. }
        | KernelError::QueueOverflow => ClientError::Capacity,
        KernelError::OutputInvariant => ClientError::Internal,
        _ => ClientError::InvalidInput,
    }
}

impl NativePreparation {
    pub(super) fn bytes(&self) -> usize {
        usize::from(self.near.is_some()) * NEAR_SCRATCH_BYTES
            + usize::from(self.far.is_some()) * FAR_SCRATCH_BYTES
    }

    pub(super) fn build(
        &mut self,
        payload: &PreparationPayload,
        available: usize,
    ) -> Result<PreparedGeometry, ClientError> {
        let (scratch, destination) = match payload {
            PreparationPayload::Near(_) => (
                if self.near.is_none() {
                    NEAR_SCRATCH_BYTES
                } else {
                    0
                },
                PreparedGeometry::MAX_SECTION_QUADS * std::mem::size_of::<MeshQuad>(),
            ),
            PreparationPayload::Far(_) => (
                if self.far.is_none() {
                    FAR_SCRATCH_BYTES
                } else {
                    0
                },
                PreparedGeometry::MAX_LOD_QUADS * std::mem::size_of::<LodQuad>(),
            ),
        };
        let peak = self
            .bytes()
            .checked_add(scratch)
            .and_then(|bytes| bytes.checked_add(destination + 1))
            .ok_or(ClientError::Capacity)?;
        if peak > available {
            return Err(ClientError::Capacity);
        }
        match payload {
            PreparationPayload::Near(view) => {
                if self.near.is_none() {
                    self.near = Some(MeshScratch::try_new().map_err(kernel_error)?);
                }
                let mut output = vec![MeshQuad::default(); PreparedGeometry::MAX_SECTION_QUADS];
                let count = NativeMesh
                    .mesh(
                        &view.as_mesh_view(),
                        self.near.as_mut().expect("initialized scratch"),
                        &mut output,
                    )
                    .map_err(kernel_error)?;
                output.truncate(count);
                PreparedGeometry::try_new(PreparedGeometry::Near(
                    output.into_boxed_slice().into_vec(),
                ))
            }
            PreparationPayload::Far(request) => {
                if self.far.is_none() {
                    self.far = Some(LodScratch::try_new().map_err(kernel_error)?);
                }
                let mut output = vec![LodQuad::default(); PreparedGeometry::MAX_LOD_QUADS];
                let count = NativeLod
                    .build(
                        &request.as_lod_request(),
                        self.far.as_mut().expect("initialized scratch"),
                        &mut output,
                    )
                    .map_err(kernel_error)?;
                output.truncate(count);
                PreparedGeometry::try_new(PreparedGeometry::Far(
                    output.into_boxed_slice().into_vec(),
                ))
            }
        }
    }
}
