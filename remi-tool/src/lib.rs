#[cfg(feature = "tool-bash")]
pub mod bash;

#[cfg(feature = "tool-fs")]
pub mod fs;

#[cfg(feature = "tool-fs-virtual")]
pub mod vfs;

#[cfg(feature = "tool-bash-virtual")]
pub mod bashkit;

// Re-exports for convenience
#[cfg(feature = "tool-bash")]
pub use bash::BashTool;

#[cfg(feature = "tool-fs")]
pub use fs::FsTool;

#[cfg(feature = "tool-fs-virtual")]
pub use vfs::VirtualFsTool;

#[cfg(feature = "tool-bash-virtual")]
pub use bashkit::VirtualBashTool;
