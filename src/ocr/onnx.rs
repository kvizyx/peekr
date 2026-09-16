//! ONNX Runtime session wrapper with memory settings suited for a long-running tray app.

use std::path::Path;

use anyhow::Result;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{RunOptions, Session};
use ort::value::TensorRef;

/// An ONNX model with a single float image input and a single float output.
pub struct OnnxModel {
    session: Session,
    run_options: RunOptions,
}

impl OnnxModel {
    pub fn load(path: &Path) -> ort::Result<Self> {
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(path)?;

        // Every selection has a different size, and the CPU memory arena keeps a block for each
        // new tensor shape: after a few full-screen captures the process held over a gigabyte.
        // Shrinking the arena after each run returns that memory without slowing inference down.
        let mut run_options = RunOptions::new()?;
        run_options.set("memory.enable_memory_arena_shrinkage", "cpu:0")?;

        Ok(Self { session, run_options })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Runs the model on a `[batch, channels, height, width]` tensor and hands the first output
    /// (shape and data) to `read`, which avoids copying large outputs.
    pub fn run<R>(&mut self, shape: [usize; 4], data: &[f32], read: impl FnOnce(&[i64], &[f32]) -> R) -> Result<R> {
        let input = TensorRef::from_array_view((shape, data))?;
        let outputs = self.session.run_with_options(ort::inputs![input], &self.run_options)?;
        let (output_shape, output) = outputs[0].try_extract_tensor::<f32>()?;

        Ok(read(output_shape, output))
    }
}
