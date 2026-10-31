// Whether the card OCR's GPU sessions run on has memory of its own (a dedicated card) or shares the
// system's RAM (an integrated one). With a dedicated card a model keeps a CPU session and a GPU one
// side by side (`sessions::DeviceSessions`): the CPU's in RAM, the GPU's in the card's memory. Asked
// at the user's request on 2026-10-30. `None` when it cannot tell; the caller then counts the card as
// integrated, one session per model, as before.
//
// Windows: the adapter DirectML takes (`paddle::gpu_builder`: adapter 0, or the one the performance
// preference picks), asked through Direct3D 12 whether its memory is the system's (UMA). Linux: the
// Vulkan devices Dawn chooses from (high performance unless the user asked for low power, which
// takes an integrated card when there is one), by their type. The Vulkan loader is opened at run
// time; without it, `None`.

use crate::settings::GpuPreference;

#[cfg(windows)]
pub fn dedicated(choice: Option<GpuPreference>) -> Option<bool> {
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::{D3D12CreateDevice, ID3D12Device, D3D12_FEATURE_ARCHITECTURE, D3D12_FEATURE_DATA_ARCHITECTURE};
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory6, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        DXGI_GPU_PREFERENCE_MINIMUM_POWER, DXGI_GPU_PREFERENCE_UNSPECIFIED,
    };

    let preference = match choice {
        None => DXGI_GPU_PREFERENCE_UNSPECIFIED,
        Some(GpuPreference::HighPerformance) => DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        Some(GpuPreference::LowPower) => DXGI_GPU_PREFERENCE_MINIMUM_POWER,
    };
    // SAFETY: plain COM calls; every interface is released when dropped, and `CheckFeatureSupport`
    // is given a structure of the size it is told.
    unsafe {
        let factory: IDXGIFactory6 = CreateDXGIFactory1().ok()?;
        let adapter: IDXGIAdapter1 = factory.EnumAdapterByGpuPreference(0, preference).ok()?;
        let desc = adapter.GetDesc1().ok()?;
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            return Some(false);
        }
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device).ok()?;
        let device = device?;
        let mut architecture = D3D12_FEATURE_DATA_ARCHITECTURE::default();
        device
            .CheckFeatureSupport(
                D3D12_FEATURE_ARCHITECTURE,
                &mut architecture as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<D3D12_FEATURE_DATA_ARCHITECTURE>() as u32,
            )
            .ok()?;
        Some(!architecture.UMA.as_bool())
    }
}

#[cfg(target_os = "linux")]
pub fn dedicated(choice: Option<GpuPreference>) -> Option<bool> {
    use ash::vk;

    // SAFETY: the loader is opened, one instance made, its devices read, and the instance destroyed
    // before the loader is dropped.
    let kinds: Vec<vk::PhysicalDeviceType> = unsafe {
        let entry = ash::Entry::load().ok()?;
        let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 0, 0));
        let instance = entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None).ok()?;
        let kinds = instance
            .enumerate_physical_devices()
            .unwrap_or_default()
            .into_iter()
            .map(|d| instance.get_physical_device_properties(d).device_type)
            .collect();
        instance.destroy_instance(None);
        kinds
    };
    pick(&kinds.iter().map(|&k| k == vk::PhysicalDeviceType::DISCRETE_GPU).collect::<Vec<_>>(),
        &kinds.iter().map(|&k| k == vk::PhysicalDeviceType::INTEGRATED_GPU).collect::<Vec<_>>(), choice)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn dedicated(_choice: Option<GpuPreference>) -> Option<bool> {
    None
}

/// Whether the card Dawn takes is dedicated, from the devices' types: a dedicated one when there is
/// one, or with low power an integrated one when there is one. No card of either type: `None`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pick(dedicated: &[bool], integrated: &[bool], choice: Option<GpuPreference>) -> Option<bool> {
    let (any_dedicated, any_integrated) = (dedicated.iter().any(|&d| d), integrated.iter().any(|&i| i));
    match choice {
        Some(GpuPreference::LowPower) if any_integrated => Some(false),
        _ if any_dedicated => Some(true),
        _ if any_integrated => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dawn_takes_the_dedicated_card_unless_low_power_finds_an_integrated_one() {
        // A laptop with both.
        assert_eq!(pick(&[true, false], &[false, true], None), Some(true));
        assert_eq!(pick(&[true, false], &[false, true], Some(GpuPreference::HighPerformance)), Some(true));
        assert_eq!(pick(&[true, false], &[false, true], Some(GpuPreference::LowPower)), Some(false));
        // Only an integrated card (an APU), or only a dedicated one.
        assert_eq!(pick(&[false], &[true], None), Some(false));
        assert_eq!(pick(&[true], &[false], Some(GpuPreference::LowPower)), Some(true));
        // Only a software device (llvmpipe), or none.
        assert_eq!(pick(&[false], &[false], None), None);
        assert_eq!(pick(&[], &[], None), None);
    }
}
