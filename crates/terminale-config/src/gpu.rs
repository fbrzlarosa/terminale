//! GPU backend selection and power-preference configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which GPU backend wgpu should target. `Auto` lets wgpu pick the best
/// available API for the platform; the explicit variants force one API; and
/// `Software` requests a CPU fallback adapter (lavapipe / WARP), which lets
/// users disable hardware GPU acceleration entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GpuBackend {
    /// Let wgpu choose the best backend for this platform (default).
    Auto,
    /// Force the Vulkan backend (Linux / Windows / Android).
    Vulkan,
    /// Force the Direct3D 12 backend (Windows).
    Dx12,
    /// Force the Metal backend (macOS / iOS).
    Metal,
    /// Force the OpenGL / OpenGL ES backend (broad but slow).
    Gl,
    /// Disable hardware acceleration: request a CPU fallback adapter.
    Software,
}

impl Default for GpuBackend {
    fn default() -> Self {
        Self::Auto
    }
}

impl GpuBackend {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 6] {
        [
            Self::Auto,
            Self::Vulkan,
            Self::Dx12,
            Self::Metal,
            Self::Gl,
            Self::Software,
        ]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Vulkan => "Vulkan",
            Self::Dx12 => "Direct3D 12",
            Self::Metal => "Metal",
            Self::Gl => "OpenGL",
            Self::Software => "Software (disable GPU)",
        }
    }
}

/// Adapter power-preference hint passed to wgpu's adapter selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GpuPowerPreference {
    /// No preference — wgpu decides (default).
    Auto,
    /// Prefer the lowest-power adapter (typically an integrated GPU).
    Low,
    /// Prefer the highest-performance adapter (typically a discrete GPU).
    High,
}

impl Default for GpuPowerPreference {
    fn default() -> Self {
        Self::Auto
    }
}

impl GpuPowerPreference {
    /// All variants in display order — useful for UI dropdowns.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Auto, Self::Low, Self::High]
    }

    /// Human-readable label for UI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Low => "Low power",
            Self::High => "High performance",
        }
    }
}

/// GPU backend selection. Lets users force a specific graphics API or
/// disable hardware acceleration outright (`backend = "software"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct GpuConfig {
    /// Graphics API to target: `auto`, `vulkan`, `dx12`, `metal`, `gl`, or
    /// `software`. `software` requests a CPU fallback adapter so the GPU is
    /// effectively disabled. Defaults to `auto`.
    pub backend: GpuBackend,
    /// Adapter power preference: `auto`, `low`, or `high`. Defaults to `auto`.
    pub power_preference: GpuPowerPreference,
}
