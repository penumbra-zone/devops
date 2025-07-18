//! CustomResourceDefinitions for Penumbra concepts,
//! like [PenumbraNode] and [PenumbraNetwork].
// pub mod network;
// pub mod node;

mod network;
mod node;
mod resources;
pub use crate::crd::network::PenumbraNetwork;
pub use crate::crd::node::PenumbraNode;
