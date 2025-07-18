# penumbra-operator

Deploy [Penumbra](https://penumbra.zone/) nodes on k8s,
via the controller pattern, implemented via [kube.rs](https://kube.rs).

## motivation

This tool was written as a development spike to represent Penumbra deployments as code,
as well as to learn more about k8s internals.

If you're interested in deploying Penumbra services on k8s, you should
use the well-supported [cosmos-operator].
**

# development

```
cargo check
```

to get started. There's also a `justfile` with helper scripts. 

[cosmos-operator]: https://github.com/strangelove-ventures/cosmos-operator/
