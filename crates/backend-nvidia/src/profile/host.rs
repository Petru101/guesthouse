//! What changes with the host driver's branch: the sizes of its structures.

/// A control a guest may run on the host's card, and the size of its parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestControl {
    pub command: u32,
    pub params_size: usize,
}

const fn control(command: u32, params_size: usize) -> GuestControl {
    GuestControl { command, params_size }
}

/// Sizes and positions in the RM's requests and parameters, for one branch. Field names follow the
/// open kernel module's headers, where these can be checked.
#[derive(Debug, PartialEq, Eq)]
pub struct HostBranch {
    pub name: &'static str,
    /// Exact versions that have been run.
    pub tested_versions: &'static [&'static str],

    pub alloc_request: usize,
    pub alloc_request_status: usize,
    pub control_request: usize,
    pub control_request_status: usize,
    pub free_request: usize,
    pub free_request_status: usize,
    pub alloc_memory_request: usize,
    pub alloc_memory_request_fd: usize,
    pub alloc_memory_request_status: usize,
    pub map_dma_request: usize,
    pub map_dma_request_address: usize,
    pub map_dma_request_status: usize,
    pub unmap_dma_request: usize,
    pub unmap_dma_request_status: usize,
    pub map_memory_request: usize,
    pub map_memory_request_fd: usize,
    pub map_memory_request_status: usize,

    pub client_params: usize,
    pub device_params: usize,
    pub subdevice_params: usize,
    pub memory_params: usize,
    pub address_space_params: usize,
    pub channel_group_params: usize,
    pub context_share_params: usize,
    pub channel_params: usize,

    pub export_object_params: usize,
    pub gem_import_params: usize,
    pub gem_import_request: usize,
    pub gem_import_request_handle: usize,

    /// All of these only read. Their parameters hold no pointers, and the only handles in them are
    /// looked up in this daemon's own client, so none can reach another client's objects.
    pub guest_controls: &'static [GuestControl],
}

/// Branch 615.
pub const HOST_615: HostBranch = HostBranch {
    name: "615",
    tested_versions: &["615.78.08"],
    alloc_request: 32,
    alloc_request_status: 28,
    control_request: 32,
    control_request_status: 28,
    free_request: 16,
    free_request_status: 12,
    alloc_memory_request: 56,
    alloc_memory_request_fd: 48,
    alloc_memory_request_status: 40,
    map_dma_request: 64,
    map_dma_request_address: 48,
    map_dma_request_status: 56,
    unmap_dma_request: 48,
    unmap_dma_request_status: 40,
    map_memory_request: 56,
    map_memory_request_fd: 48,
    map_memory_request_status: 40,
    client_params: 120,
    device_params: 0x38,
    subdevice_params: 4,
    memory_params: 0x80,
    address_space_params: 0x38,
    channel_group_params: 0x1C,
    context_share_params: 0xC,
    channel_params: 0x178,
    export_object_params: 24,
    gem_import_params: 28,
    gem_import_request: 32,
    gem_import_request_handle: 24,
    // Names and sizes are the public headers' for the branch: ctrl0000, ctrl0080 and ctrl2080.
    guest_controls: &[
        control(0x0000_0102, 108),   // SYSTEM_GET_CPU_INFO
        control(0x0000_0201, 128),   // GPU_GET_ATTACHED_IDS
        control(0x0000_0203, 8),     // GPU_GET_INIT_STATUS
        control(0x0000_0204, 4),     // GPU_GET_DEVICE_IDS
        control(0x0000_0205, 32),    // GPU_GET_ID_INFO_V2
        control(0x0000_021B, 12),    // GPU_GET_PCI_INFO
        control(0x0000_0275, 268),   // GPU_GET_UUID_FROM_GPU_ID
        control(0x0000_0279, 12),    // GPU_QUERY_DRAIN_STATE
        control(0x0000_0289, 4),     // GPU_ASYNC_ATTACH_ID, which only checks the ID
        control(0x0000_0290, 4),     // GPU_WAIT_ATTACH_ID, which does nothing
        control(0x0000_0301, 16),    // GSYNC_GET_ATTACHED_IDS
        control(0x0080_0280, 4),     // GPU_GET_NUM_SUBDEVICES
        control(0x0080_0289, 8),     // GPU_GET_VIRTUALIZATION_MODE
        control(0x0080_0292, 804),   // GPU_GET_CLASSLIST_V2
        control(0x0080_1806, 192),   // DMA_ADV_SCHED_GET_VA_CAPS
        control(0x0080_180D, 12),    // DMA_GET_CAPS
        control(0x0080_1C02, 12),    // NVDEC_GET_CAPS_V2
        control(0x2080_0102, 580),   // GPU_GET_INFO_V2
        control(0x2080_0110, 68),    // GPU_GET_NAME_STRING
        control(0x2080_0111, 64),    // GPU_GET_SHORT_NAME_STRING
        control(0x2080_0119, 4),     // GPU_GET_SIMULATION_INFO
        control(0x2080_012F, 1664),  // GPU_QUERY_ECC_STATUS
        control(0x2080_0142, 4),     // GPU_GET_ID
        control(0x2080_014A, 268),   // GPU_GET_GID_INFO
        control(0x2080_0170, 340),   // GPU_GET_ENGINES_V2
        control(0x2080_0406, 264),   // TIMER_GET_GPU_CPU_TIME_CORRELATION_INFO
        control(0x2080_1206, 40),    // GR_GET_ZCULL_INFO
        control(0x2080_121B, 10264), // GR_GET_GLOBAL_SM_ORDER
        control(0x2080_1228, 528),   // GR_GET_INFO_V2
        control(0x2080_1303, 1028),  // FB_GET_INFO_V2
        control(0x2080_1315, 16),    // FB_GET_GPU_CACHE_INFO
        control(0x2080_1352, 32),    // FB_GET_SEMAPHORE_SURFACE_LAYOUT
        control(0x2080_1701, 16),    // MC_GET_ARCH_INFO
        control(0x2080_1801, 16),    // BUS_GET_PCI_INFO
        control(0x2080_1823, 420),   // BUS_GET_INFO_V2, and see `clears_counters`
        control(0x2080_2A03, 8),     // CE_GET_CAPS_V2
    ],
};

const BRANCHES: [&HostBranch; 1] = [&HOST_615];

/// The profile for a branch this build knows.
pub fn host_branch(name: &str) -> Option<&'static HostBranch> {
    BRANCHES.iter().copied().find(|branch| branch.name == name)
}

/// The branch of a version like `615.78.08`.
pub fn host_branch_of_version(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_names_its_branch() {
        assert_eq!(host_branch_of_version("615.78.08"), "615");
    }

    #[test]
    fn only_known_branches_have_a_profile() {
        assert!(host_branch("615").is_some());
        assert!(host_branch("999").is_none());
    }
}
