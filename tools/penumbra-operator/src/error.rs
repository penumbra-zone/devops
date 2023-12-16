use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("k8s api returned error: {0}")]
    KubeError(#[from] kube::Error),

    #[error("failed to get lock on jawn")]
    ClientAccess,

    #[error("k8s api returned error: {0}")]
    // Type madness is straight out of `controller-rs` example, I swear.
    FinalizerError(#[source] Box<kube::runtime::finalizer::Error<Error>>),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;
