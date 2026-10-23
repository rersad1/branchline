mod registry;
mod validation;

pub use registry::{NewProject, Registry, RegistryError};
pub use validation::{validate_registration, ProjectError, ResolvedRepository};
