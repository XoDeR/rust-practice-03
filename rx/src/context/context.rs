use crate::{audio::AudioContext, filesystem::Filesystem, graphics::GraphicsContext};

/// Holds global resources
pub struct Context {
    pub fs: Filesystem,
    pub gfx: GraphicsContext,
    pub audio: AudioContext,
}
