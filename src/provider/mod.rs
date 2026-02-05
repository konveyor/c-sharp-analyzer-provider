mod code_snip;
mod csharp;
mod dependency_resolution;
mod project;
pub(crate) mod sdk_detection;
pub(crate) mod target_framework;

pub use csharp::CSharpProvider;
pub use project::AnalysisMode;
pub use project::Project;
