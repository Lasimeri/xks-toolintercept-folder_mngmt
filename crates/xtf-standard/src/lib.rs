//! The `xks-folder/1` standard: how an agent's folder and naming decisions
//! are asked of a Jev-like model the same way in every project.
//!
//! - [`roles`]: the fixed roles a directory plays, with their fixed wording.
//! - [`naming`]: naming styles, a rule (check and convert), never a question.
//! - [`manifest`]: the provided JSON, `xks-project.json`: the project, its
//!   purpose, its layout (role -> directories), its naming styles.
//! - [`projection`]: the projected JSON: one operation about to happen.
//! - [`catalog`]: every question the model is asked, with stable ids.
//! - [`policy`]: rules before the model, and answers into actions.
//! - [`decide`]: one decision against a System One server, logged.
//!
//! One decision (the decision server configured by `TYPESAFE_BASE_URL`,
//! `TYPESAFE_API_KEY`, `TYPESAFE_DEFAULT_MODEL`):
//!
//! ```no_run
//! use std::path::Path;
//! use xtf_standard::{Action, Decider, Manifest, Op, Projection};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let path = Path::new("my-project/src/thumbnails.rs");
//! let content = "//! Builds JPEG thumbnails for the sync preview.";
//! let root = Manifest::find_root(path).ok_or("no xks-project.json above the path")?;
//! let manifest = Manifest::load(&root)?;
//! let projection = Projection {
//!     op: Op::CreateFile,
//!     path: "src/thumbnails.rs".into(),
//!     from: None,
//!     summary: xtf_standard::projection::summary_of(content),
//!     tool: None,
//! };
//! let decision = Decider::new(xtf_standard::client(None)).decide(&manifest, &projection)?;
//! xtf_standard::decide::log(&root, &projection, &decision)?;
//! if decision.verdict.action == Action::Suggest {
//!     println!("the standard suggests {:?}", decision.verdict.suggested_path);
//! }
//! # Ok(())
//! # }
//! ```

pub mod catalog;
pub mod decide;
pub mod manifest;
pub mod naming;
pub mod policy;
pub mod projection;
pub mod roles;
pub mod scaffold;

pub use decide::{Decider, Decision};
pub use manifest::{MANIFEST_FILE, Manifest, Place, STANDARD};
pub use policy::{Action, Thresholds, Verdict};
pub use projection::{Op, Projection};
pub use roles::Role;

/// The decision server, configured as TypeSafe's SDKs are: `TYPESAFE_BASE_URL`
/// (default `jevbase serve` at http://127.0.0.1:8097), `TYPESAFE_API_KEY`,
/// `TYPESAFE_DEFAULT_MODEL` (default `jev-latest`); `base` overrides the URL.
/// Any System One server answers the standard's questions: `jevbase serve`,
/// Intel-Phi-Jev's `xks serve`, or the hosted Jev.
pub fn client(base: Option<&str>) -> jevbase::Client {
    let env = jevbase::Client::from_env();
    let Some(base) = base else {
        return env;
    };
    let c = jevbase::Client::new(base).with_model(env.model());
    match std::env::var("TYPESAFE_API_KEY") {
        Ok(k) if !k.trim().is_empty() => c.with_api_key(k.trim()),
        _ => c,
    }
}
